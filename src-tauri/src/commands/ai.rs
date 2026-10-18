//! AI 接入（OpenAI 兼容协议）。
//!
//! 三个刻意的设计决定：
//!
//! 1. **Key 不进前端，也不落明文。** 前端只发「设置 Key」这个动作，之后所有请求都在
//!    Rust 侧发起；Key 用 Windows DPAPI（`CryptProtectData`）加密后存盘，
//!    只有当前登录用户可以解开。存 localStorage 是明文落盘的，不能接受。
//! 2. **只发摘要，不发内容。** `ai_analyze` 收的是「路径 + 体积 + 扩展名分布 + 子项名」，
//!    没有任何文件内容。用户能在界面上看到将发送的原文。
//! 3. **AI 永远不参与删除决策。** 这里返回的 `canDelete` 只用于文案与颜色，
//!    真正删除只认扫描树里的真实路径，且必须用户勾选确认。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

// ============ 配置 ============

/// 内置额度：用户不配置任何东西也能直接用 AI。
pub const PROVIDER_BUILTIN: &str = "builtin";
const BUILTIN_BASE_URL: &str = "https://api.agnes-ai.cn/v1";
const BUILTIN_MODEL: &str = "agnes-3.0-flash";
/// 内置额度真正可用的模型白名单。
///
/// 只放当前免费可用的那一个：`agnes-3.0-pro` 在额度里调不通，
/// 放进选项目前只会让用户点了报错，不如不给。
/// 以后哪天空闲额度放开某个模型，往这里加一项、前端会自动变成可选项。
const BUILTIN_MODELS: &[&str] = &["agnes-3.0-flash"];

/// 把用户/旧配置里的模型名收敛到内置白名单。
///
/// 为什么要做这一步：旧版本可能已经存过 `agnes-3.0-pro`，
/// 直接沿用会在请求时才报错，用户看到的是一句莫名其妙的鉴权/额度错误。
/// 在这里静默回落到默认模型，比让用户自己去猜好得多。
fn builtin_model(requested: &str) -> String {
    let r = requested.trim();
    if BUILTIN_MODELS.contains(&r) {
        r.to_string()
    } else {
        BUILTIN_MODEL.to_string()
    }
}

/// 内置体验密钥。
///
/// ⚠️ 必须说清楚的一件事：**客户端里内置的 Key 无法真正保密**。
/// 二进制里可以扒、运行时内存里也一定有明文，所以它只能是「开箱可用的体验额度」，
/// 不是安全边界，也不该被当成付费承诺。
///
/// 这里做 XOR 混淆，只是让 `strings app.exe` 之类的手段捞不到明文，
/// 增加一点提取成本而已，不改变上面的事实。
fn builtin_key() -> String {
    const ENC: [u8; 51] = [
        41, 49, 119, 99, 3, 55, 98, 25, 106, 49, 14, 45, 110, 27, 17, 57, 104, 0, 32, 59, 98, 51,
        110, 14, 55, 22, 53, 17, 30, 107, 43, 98, 99, 13, 49, 60, 8, 47, 11, 55, 61, 50, 40, 18,
        34, 13, 45, 47, 57, 98, 104,
    ];
    ENC.iter().map(|b| (*b ^ 0x5A) as char).collect()
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
struct AiConfigFile {
    provider: String,
    base_url: String,
    model: String,
    /// DPAPI 加密后的 base64；空表示未配置（此时走内置额度）
    key_enc: String,
    enabled: bool,
}

impl Default for AiConfigFile {
    fn default() -> Self {
        Self {
            provider: PROVIDER_BUILTIN.into(),
            base_url: BUILTIN_BASE_URL.into(),
            model: BUILTIN_MODEL.into(),
            key_enc: String::new(),
            // 默认就是开着的：装好就能用，不需要先去设置里配东西
            enabled: true,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigPublic {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub enabled: bool,
    pub has_key: bool,
    /// 形如 `sk-…a1b2`；内置额度时是「内置额度」
    pub key_hint: String,
    /// 当前是否在用内置额度
    pub is_builtin: bool,
}

fn config_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path().app_config_dir().ok().map(|d| d.join("ai.json"))
}

fn load(app: &tauri::AppHandle) -> AiConfigFile {
    let Some(p) = config_path(app) else {
        return AiConfigFile::default();
    };
    fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(app: &tauri::AppHandle, cfg: &AiConfigFile) -> Result<(), String> {
    use tauri::Manager;
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    let txt = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(dir.join("ai.json"), txt).map_err(|e| format!("写入配置失败：{e}"))?;
    // 顺手把目录属性设成隐藏没有必要；文件本身不含明文 Key。
    Ok(())
}

// ============ DPAPI ============

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

#[cfg(windows)]
fn protect(plain: &str) -> Result<String, String> {
    use base64::Engine as _;
    use core::ffi::c_void;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_PROMPTSTRUCT,
    };

    let data = plain.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null::<c_void>(),
            std::ptr::null::<CRYPTPROTECT_PROMPTSTRUCT>(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("系统加密失败（DPAPI 不可用）".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    unsafe {
        LocalFree(output.pbData as *mut c_void);
    }
    Ok(b64)
}

#[cfg(windows)]
fn unprotect(b64: &str) -> Result<String, String> {
    use base64::Engine as _;
    use core::ffi::c_void;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_PROMPTSTRUCT,
    };

    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| "密钥数据损坏".to_string())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: raw.len() as u32,
        pbData: raw.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null::<c_void>(),
            std::ptr::null::<CRYPTPROTECT_PROMPTSTRUCT>(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        // 换了用户账号 / 换了机器：DPAPI 解不开，只能让用户重填
        return Err("已保存的密钥无法解密（可能换过 Windows 账号），请重新填写".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let s = String::from_utf8_lossy(bytes).into_owned();
    unsafe {
        LocalFree(output.pbData as *mut c_void);
    }
    Ok(s)
}

#[cfg(not(windows))]
fn protect(_plain: &str) -> Result<String, String> {
    Err("仅支持 Windows".into())
}

#[cfg(not(windows))]
fn unprotect(_b64: &str) -> Result<String, String> {
    Err("仅支持 Windows".into())
}

fn mask(key: &str) -> String {
    if key.len() <= 8 {
        return "****".into();
    }
    let head: String = key.chars().take(3).collect();
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}…{tail}")
}

/// 当前是否在用内置额度。老配置里 provider 可能是空串，也归到内置。
fn use_builtin(cfg: &AiConfigFile) -> bool {
    cfg.provider.trim().is_empty() || cfg.provider == PROVIDER_BUILTIN || cfg.base_url.trim().is_empty()
}

struct ActiveAi {
    base_url: String,
    model: String,
    key: String,
    #[allow(dead_code)] // 目前只用于排查，不参与逻辑
    is_builtin: bool,
}

/// 决定「这次请求到底发给谁」。
///
/// 兜底方向是「用内置」而不是「报错」：配置被删、被改坏、或是从老版本升级上来的空配置，
/// 都应该还能正常用 AI，而不是让用户先去设置里折腾一遍。
fn resolve(app: &tauri::AppHandle) -> Result<ActiveAi, String> {
    let cfg = load(app);
    if !cfg.enabled {
        return Err("AI 已关闭，可在「设置 - AI 助手」里重新打开".into());
    }
    if use_builtin(&cfg) {
        return Ok(ActiveAi {
            base_url: BUILTIN_BASE_URL.into(),
            model: builtin_model(&cfg.model),
            key: builtin_key(),
            is_builtin: true,
        });
    }
    let key = if cfg.key_enc.is_empty() {
        String::new()
    } else {
        unprotect(&cfg.key_enc)?
    };
    Ok(ActiveAi {
        base_url: cfg.base_url.trim().to_string(),
        model: cfg.model.trim().to_string(),
        key,
        is_builtin: false,
    })
}

// ============ HTTP ============

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(15))
            .user_agent("disk-manager-ai/1.3.3")
            // 沿用系统代理设置：用户开着 Clash 之类能自动走，不用额外配置
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn endpoint(base: &str, tail: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with(tail) {
        b.to_string()
    } else {
        format!("{b}{tail}")
    }
}

/// 把 reqwest / HTTP 状态错误翻译成用户能看懂的一句话
fn friendly(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return "请求超时：网络太慢或服务端无响应".into();
    }
    if e.is_connect() {
        return "连不上服务端：检查网络、代理设置或接口地址是否正确".into();
    }
    if let Some(code) = e.status() {
        return match code.as_u16() {
            401 | 403 => "鉴权失败：Key 无效、已过期，或该 Key 无权访问这个模型".to_string(),
            404 => "接口地址或模型名不对（404）：检查 base URL 是否包含 /v1 之类的版本段".to_string(),
            429 => "请求过于频繁或额度用尽（429）：稍后再试，或去服务商控制台看余额".to_string(),
            500..=599 => format!("服务端出错（{code}）：不是你的问题，稍后重试"),
            _ => format!("请求失败（HTTP {code}）"),
        };
    }
    format!("请求失败：{e}")
}

#[derive(Serialize)]
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<ChatMsg>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMsg,
}

async fn complete(
    base_url: &str,
    model: &str,
    key: &str,
    messages: Vec<ChatMsg>,
    max_tokens: u32,
    temperature: f32,
) -> Result<String, String> {
    if base_url.trim().is_empty() {
        return Err("还没有配置接口地址，请到「设置 - AI 助手」里填写".into());
    }
    if model.trim().is_empty() {
        return Err("还没有配置模型名".into());
    }
    let url = endpoint(base_url, "/chat/completions");
    let body = ChatReq {
        model,
        messages,
        max_tokens,
        temperature,
        stream: false,
    };
    let mut req = client().post(&url).json(&body);
    if !key.trim().is_empty() {
        req = req.bearer_auth(key.trim());
    }
    let resp = req.send().await.map_err(friendly)?;
    if !resp.status().is_success() {
        // 上游错误体里通常有更具体的原因，但要截断，避免把 HTML 整页抛给界面
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let brief: String = text.chars().take(240).collect();
        let hint = match code.as_u16() {
            401 | 403 => "鉴权失败：Key 无效或无权访问该模型",
            404 => "接口地址或模型名不对（404）",
            429 => "请求过于频繁或额度用尽（429）",
            _ => "服务端返回错误",
        };
        return Err(format!("{hint}。详情：{brief}"));
    }
    let data: ChatResp = resp.json().await.map_err(friendly)?;
    data.choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "服务端返回了空结果".to_string())
}

// ============ 命令 ============

#[tauri::command]
pub async fn ai_get_config(app: tauri::AppHandle) -> Result<AiConfigPublic, String> {
    let cfg = load(&app);
    let is_builtin = use_builtin(&cfg);
    let (has_key, hint) = if is_builtin {
        (true, "内置额度".to_string())
    } else if cfg.key_enc.is_empty() {
        (false, String::new())
    } else {
        (
            true,
            unprotect(&cfg.key_enc).map(|k| mask(&k)).unwrap_or_else(|_| "（无法读取）".into()),
        )
    };
    Ok(AiConfigPublic {
        provider: if is_builtin { PROVIDER_BUILTIN.into() } else { cfg.provider },
        base_url: if is_builtin { BUILTIN_BASE_URL.into() } else { cfg.base_url },
        // 内置走白名单收敛：界面显示的就是实际会用的，不能显示 pro 实际发 flash
        model: if is_builtin { builtin_model(&cfg.model) } else { cfg.model },
        enabled: cfg.enabled,
        has_key,
        key_hint: hint,
        is_builtin,
    })
}

/// 保存配置。`api_key` 传 None 表示「不改动已保存的 Key」。
#[tauri::command]
pub async fn ai_save_config(
    app: tauri::AppHandle,
    provider: String,
    base_url: String,
    model: String,
    enabled: bool,
    api_key: Option<String>,
) -> Result<AiConfigPublic, String> {
    let mut cfg = load(&app);
    // 内置额度不允许被改地址：它是固定的一家服务，改了就名不副实。
    // 用户想换地址就走「自定义」。
    if provider.trim() == PROVIDER_BUILTIN {
        cfg.provider = PROVIDER_BUILTIN.into();
        cfg.base_url = BUILTIN_BASE_URL.into();
    } else {
        cfg.provider = provider.trim().to_string();
        cfg.base_url = base_url.trim().to_string();
    }
    // 内置模式下模型必须落在白名单里：否则用户选了不可用的模型，保存时看不出来，
    // 直到真正分析时才报错。
    cfg.model = if cfg.provider == PROVIDER_BUILTIN {
        builtin_model(&model)
    } else if model.trim().is_empty() {
        String::new()
    } else {
        model.trim().to_string()
    };
    cfg.enabled = enabled;
    if let Some(k) = api_key {
        let k = k.trim();
        if k.is_empty() {
            cfg.key_enc.clear();
        } else {
            cfg.key_enc = protect(k)?;
        }
    }
    save(&app, &cfg)?;
    ai_get_config(app).await
}

/// 恢复内置额度：清掉自己的 Key 并切回内置。
///
/// 注意这里**不关闭 AI** —— 用户的意图通常是「不想再用自己的额度」，
/// 而不是「我不要 AI 了」。真要关掉用上面的开关。
#[tauri::command]
pub async fn ai_clear_key(app: tauri::AppHandle) -> Result<AiConfigPublic, String> {
    let mut cfg = load(&app);
    cfg.provider = PROVIDER_BUILTIN.into();
    cfg.base_url = BUILTIN_BASE_URL.into();
    cfg.model = BUILTIN_MODEL.into();
    cfg.key_enc.clear();
    cfg.enabled = true;
    save(&app, &cfg)?;
    ai_get_config(app).await
}

/// 测试连接：发一个极小的请求，能拿到回复就算通。
/// `base_url` 为空时测内置额度；传了 `api_key` 优先用它（这样保存前就能先验证）。
#[tauri::command]
pub async fn ai_test_connection(
    app: tauri::AppHandle,
    base_url: String,
    model: String,
    api_key: Option<String>,
) -> Result<String, String> {
    let (base, key, m): (String, String, String) = if base_url.trim().is_empty() {
        (
            BUILTIN_BASE_URL.into(),
            builtin_key(),
            builtin_model(&model),
        )
    } else {
        let key = match api_key {
            Some(k) if !k.trim().is_empty() => k,
            _ => {
                let cfg = load(&app);
                if cfg.key_enc.is_empty() {
                    String::new()
                } else {
                    unprotect(&cfg.key_enc)?
                }
            }
        };
        (base_url.trim().to_string(), key, model.trim().to_string())
    };
    let out = complete(
        &base,
        &m,
        &key,
        vec![ChatMsg {
            role: "user".into(),
            content: "只回复两个字：可用".into(),
        }],
        16,
        0.0,
    )
    .await?;
    Ok(format!("连接成功，模型回复：{}", out.trim().chars().take(40).collect::<String>()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeReq {
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub summary: String,
    pub local_label: String,
    pub local_verdict: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AiAnalysis {
    pub what: String,
    /// safe | caution | danger | unknown
    pub can_delete: String,
    pub consequence: String,
    pub suggestion: String,
}

const ANALYZE_SYSTEM: &str = "你是 Windows 文件系统与存储管理专家。用户会给你一个目录或文件的摘要信息，\
请判断它是什么、能不能删、删了会怎样。\
\n只输出一个 JSON 对象，不要任何解释文字，格式：\
{\"what\":\"这是什么\",\"canDelete\":\"safe|caution|danger|unknown\",\"consequence\":\"删了会怎样\",\"suggestion\":\"建议怎么做\"}\
\n要求：\
\n1. 用简体中文，每项不超过 60 字。\
\n2. 信息不足时 canDelete 必须用 unknown，不要猜。\
\n3. 绝不把系统目录、程序安装目录、用户数据目录、备份、聊天记录判成 safe。\
\n4. 不要复述输入，直接给结论。";

/// 只从模型回复里抠出 JSON。兼容被 ```json 包起来、或前后带解释文字的情况。
fn extract_json(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&s[start..=end])
}

fn sanitize_verdict(v: &str) -> String {
    match v.trim().to_lowercase().as_str() {
        "safe" => "safe".into(),
        "caution" => "caution".into(),
        "danger" => "danger".into(),
        // 任何非法值一律降级为 unknown —— 宁可少说，不可误导
        _ => "unknown".into(),
    }
}

#[tauri::command]
pub async fn ai_analyze(app: tauri::AppHandle, req: AnalyzeReq) -> Result<AiAnalysis, String> {
    let ai = resolve(&app)?;

    let user = format!(
        "路径：{}\n类型：{}\n体积：{} 字节\n本地规则初判：{}（{}）\n\n下面是本机采集到的摘要，不含任何文件内容：\n{}",
        req.path,
        if req.kind == "dir" { "目录" } else { "文件" },
        req.size,
        if req.local_label.is_empty() { "未能识别" } else { &req.local_label },
        if req.local_verdict.is_empty() { "unknown" } else { &req.local_verdict },
        if req.summary.trim().is_empty() { "（无额外摘要）" } else { req.summary.trim() },
    );

    let out = complete(
        &ai.base_url,
        &ai.model,
        &ai.key,
        vec![
            ChatMsg { role: "system".into(), content: ANALYZE_SYSTEM.into() },
            ChatMsg { role: "user".into(), content: user },
        ],
        600,
        0.2,
    )
    .await?;

    let Some(js) = extract_json(&out) else {
        return Err(format!("模型返回的不是 JSON：{}", out.chars().take(120).collect::<String>()));
    };
    let v: serde_json::Value = serde_json::from_str(js).map_err(|e| format!("解析模型回复失败：{e}"))?;
    let get = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    Ok(AiAnalysis {
        what: get("what"),
        can_delete: sanitize_verdict(&get("canDelete")),
        consequence: get("consequence"),
        suggestion: get("suggestion"),
    })
}

const CHAT_SYSTEM: &str = r#"你是「存储管理 Disk Manager」这个 Windows 桌面软件的内置助手。你只回答与本软件、磁盘存储、文件清理、软件卸载、硬件信息相关的问题。

软件功能：
1. 硬件信息：CPU、内存（含每条内存条型号/频率）、显卡、显示器、主板、系统版本、声卡、网卡。数据来自本机的 CIM/WMI 查询，缓存 30 分钟。
2. 磁盘总览：各分区容量与占用、物理硬盘型号与健康状态（SMART 通电时间、读写量、温度、磨损），点击分区进入空间分析。
3. 空间分析：扫描整个分区的目录树，用矩形树图展示谁在占空间，可以逐层下钻。支持「这是什么」——选中目录或文件后，本地规则会判断它的用途与删除后果。扫描可以随时停止。
4. 垃圾清理：分三个页签——垃圾扫描（系统临时文件、浏览器缓存、应用缓存、日志、回收站）、安装包清理（.msi/.exe 安装程序）、空文件夹。扫描结果按盘符范围统计，默认不勾选有风险的项目。
5. 软件卸载：列出已安装的 Win32 与商店应用，卸载后会扫描注册表、服务、计划任务、快捷方式等残留，也可以强制卸载卸载器损坏的软件。
6. 设置：开机自启动、点关闭按钮时的行为（询问/退到后台/直接关闭）、软件更新、AI 助手配置。

关键规则（回答涉及"该删哪个"时必须遵守）：
- 本工具不会自动删除任何东西，所有删除都需要用户勾选并确认。
- 系统目录（Windows、WinSxS、System32、Installer）绝对不能删。
- 程序安装目录应该用「软件卸载」功能卸载，不要直接删。
- 显示"标准权限"时扫描结果会偏小，因为部分系统目录读不到，这时可以点侧边栏的「提权」以管理员身份重启。
- 扫描结果被停止或失败时，界面上会保留上一次的结果并明确标注"下方是上一次结果"。

回答要求：
- 中文，直接给答案，不要客套话，不要重复用户的问题。
- 控制在 200 字以内，需要步骤时用 1. 2. 3. 编号。
- 不知道就说不知道，不要编造软件里没有的功能。
- 不要输出 emoji。"#;

#[tauri::command]
pub async fn ai_chat(
    app: tauri::AppHandle,
    messages: Vec<ChatMsg>,
    context: String,
) -> Result<String, String> {
    let ai = resolve(&app)?;

    let mut msgs = vec![ChatMsg {
        role: "system".into(),
        content: CHAT_SYSTEM.to_string(),
    }];
    if !context.trim().is_empty() {
        msgs.push(ChatMsg {
            role: "system".into(),
            content: format!("当前界面状态（供参考）：\n{context}"),
        });
    }
    // 只带最近 8 轮，防止上下文无限增长把费用推高
    let start = messages.len().saturating_sub(8);
    msgs.extend(messages.into_iter().skip(start));

    complete(&ai.base_url, &ai.model, &ai.key, msgs, 800, 0.4).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 混淆数组算错一位，运行时就是 401。这条测试专门盯这个。
    #[test]
    fn builtin_key_decodes() {
        let k = builtin_key();
        assert!(k.starts_with("sk-"), "key = {k}");
        assert_eq!(k.len(), 51, "key = {k}");
        assert!(k.chars().all(|c| c.is_ascii_graphic()), "key = {k}");
    }

    #[test]
    fn default_config_uses_builtin() {
        let c = AiConfigFile::default();
        assert!(c.enabled, "首次启动必须默认可用，否则「开箱即用」就是空话");
        assert_eq!(c.provider, PROVIDER_BUILTIN);
        assert_eq!(c.base_url, BUILTIN_BASE_URL);
        assert_eq!(c.model, BUILTIN_MODEL);
    }

    #[test]
    fn builtin_model_is_whitelisted() {
        assert_eq!(builtin_model("agnes-3.0-flash"), "agnes-3.0-flash");
        // 不能用 / 不存在的模型一律静默回落，不能让请求去撞墙再报个看不懂的错
        assert_eq!(builtin_model("agnes-3.0-pro"), BUILTIN_MODEL);
        assert_eq!(builtin_model("agnes-2.5-flash"), BUILTIN_MODEL);
        assert_eq!(builtin_model("gpt-4o-mini"), BUILTIN_MODEL);
        assert_eq!(builtin_model(""), BUILTIN_MODEL);
        // 白名单当前只有免费可用的那一个
        assert_eq!(BUILTIN_MODELS, &["agnes-3.0-flash"]);
    }

    #[test]
    fn builtin_detection() {
        // 空 provider / 内置 provider / 空地址，都应归到内置
        let mut c = AiConfigFile::default();
        assert!(use_builtin(&c));
        c.provider = String::new();
        assert!(use_builtin(&c));
        c.provider = "deepseek".into();
        c.base_url = String::new();
        assert!(use_builtin(&c));
        c.base_url = "https://api.deepseek.com/v1".into();
        assert!(!use_builtin(&c));
    }

    /// 需要联网，默认不跑：`cargo test --release --lib builtin_endpoint -- --ignored --nocapture`
    #[test]
    #[ignore = "需要联网"]
    fn builtin_endpoint_answers() {
        let out = tauri::async_runtime::block_on(async {
            complete(
                BUILTIN_BASE_URL,
                BUILTIN_MODEL,
                &builtin_key(),
                vec![ChatMsg {
                    role: "user".into(),
                    content: "只回复两个字：可用".into(),
                }],
                16,
                0.0,
            )
            .await
        });
        match out {
            Ok(s) => println!("内置接口返回：{}", s.trim()),
            Err(e) => panic!("内置接口不可用：{e}"),
        }
    }
}
