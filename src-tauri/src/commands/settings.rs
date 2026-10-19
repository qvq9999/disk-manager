//! 应用设置：关闭窗口的行为、开机自启动。
//!
//! 为什么这份设置在 Rust 侧而不是前端 localStorage：
//! 按下 ✕ 的那一刻必须**同步地**决定「放行关闭」还是「拦下来」。
//! 如果让 Rust 先拦下再问前端，一次 IPC 往返期间窗口已经关了，逻辑就永远走不到；
//! 所以关闭行为由 Rust 持有，前端设置页只是这份状态的读写界面。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 每次关闭都询问（默认）
pub const CLOSE_ASK: &str = "ask";
/// 隐藏窗口、继续在托盘后台运行
pub const CLOSE_BACKGROUND: &str = "background";
/// 直接退出进程
pub const CLOSE_EXIT: &str = "exit";

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_NAME: &str = "DiskManager";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// "ask" | "background" | "exit"
    pub close_action: String,
    /// 进入目录后自动分析这一层（默认关闭：浏览目录会持续消耗额度，应由用户主动开启）
    pub auto_analyze_layer: bool,
    /// 允许把当前页面的数据摘要作为 AI 上下文（关掉则只传页面名）
    pub allow_page_context: bool,
    /// 是否已阅读并同意免责声明。首次启动为 false → 前端自动弹出一次。
    /// 放在这里而不是 localStorage：清理 WebView 数据不会让声明重新弹出，
    /// 卸载重装（配置文件一并删除）才会。
    pub disclaimer_accepted: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_action: CLOSE_ASK.to_string(),
            // 自动分析默认关：每进一个目录就发一次请求，等于让程序替用户花钱。
            // 「让 AI 读取当前页面」只影响上下文长度、不额外发请求，仍默认开。
            auto_analyze_layer: false,
            allow_page_context: true,
            disclaimer_accepted: false,
        }
    }
}

/// 当前关闭行为（热路径：每次点 ✕ 都要读，放内存里）
static CLOSE_ACTION: Mutex<String> = Mutex::new(String::new());
static SETTINGS_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// 进入目录自动分析这一层（默认关）
static AUTO_ANALYZE: AtomicBool = AtomicBool::new(false);
/// 允许把页面数据摘要作为 AI 上下文
static ALLOW_CTX: AtomicBool = AtomicBool::new(true);
/// 免责声明是否已确认（true = 不必再自动弹出）
static DISCLAIMER_ACCEPTED: AtomicBool = AtomicBool::new(false);
/// 托盘是否创建成功。「退到后台」在无托盘时等于把程序藏进任务管理器，必须禁用
static TRAY_READY: AtomicBool = AtomicBool::new(false);

pub fn set_tray_ready(v: bool) {
    TRAY_READY.store(v, Ordering::Release);
}

pub fn tray_ready() -> bool {
    TRAY_READY.load(Ordering::Acquire)
}

/// 启动时调用一次：确定配置文件位置并载入上次的选择
pub fn init(app: &tauri::AppHandle) {
    use tauri::Manager;
    let path = app.path().app_config_dir().ok().map(|d| d.join("settings.json"));
    let mut action = String::new();
    if let Some(p) = &path {
        if let Ok(txt) = std::fs::read_to_string(p) {
            if let Ok(s) = serde_json::from_str::<AppSettings>(&txt) {
                action = s.close_action;
                AUTO_ANALYZE.store(s.auto_analyze_layer, Ordering::Release);
                ALLOW_CTX.store(s.allow_page_context, Ordering::Release);
                DISCLAIMER_ACCEPTED.store(s.disclaimer_accepted, Ordering::Release);
            }
        }
    }
    if !is_known(&action) {
        action = CLOSE_ASK.to_string();
    }
    if let Ok(mut g) = CLOSE_ACTION.lock() {
        *g = action;
    }
    if let Ok(mut g) = SETTINGS_PATH.lock() {
        *g = path;
    }
}

fn is_known(a: &str) -> bool {
    a == CLOSE_ASK || a == CLOSE_BACKGROUND || a == CLOSE_EXIT
}

/// 配置文件所在目录：硬件缓存等「旁挂数据」复用同一位置，
/// 避免在用户目录里再散落一个自建路径。
pub fn config_dir() -> Option<PathBuf> {
    SETTINGS_PATH
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
}

pub fn close_action() -> String {
    CLOSE_ACTION
        .lock()
        .map(|g| g.clone())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| CLOSE_ASK.to_string())
}

fn persist() {
    let path = SETTINGS_PATH.lock().ok().and_then(|g| g.clone());
    let Some(path) = path else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 必须写全字段：只写 close_action 会把另外两个开关重置回默认值
    let body = AppSettings {
        close_action: close_action(),
        auto_analyze_layer: AUTO_ANALYZE.load(Ordering::Relaxed),
        allow_page_context: ALLOW_CTX.load(Ordering::Relaxed),
        disclaimer_accepted: DISCLAIMER_ACCEPTED.load(Ordering::Relaxed),
    };
    if let Ok(txt) = serde_json::to_string_pretty(&body) {
        if let Err(e) = std::fs::write(&path, txt) {
            log::warn!("写入设置失败: {e}");
        }
    }
}

pub fn set_close_action(action: &str) -> Result<(), String> {
    if !is_known(action) {
        return Err(format!("未知的关闭行为: {action}"));
    }
    // 「退到后台」依赖托盘：没有托盘就等于把窗口藏没了，直接拒绝
    if action == CLOSE_BACKGROUND && !tray_ready() {
        return Err("托盘图标未能创建，无法退到后台".into());
    }
    if let Ok(mut g) = CLOSE_ACTION.lock() {
        *g = action.to_string();
    }
    persist();
    Ok(())
}

// ---------------- 开机自启动（HKCU ...\Run） ----------------

pub fn autostart_enabled() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_READ)
        .ok()
        .and_then(|k| k.get_value::<String, _>(AUTOSTART_NAME).ok())
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| format!("打开自启动注册表键失败：{e}"))?;

    if enabled {
        let exe = std::env::current_exe().map_err(|e| format!("获取程序路径失败：{e}"))?;
        // 路径必须加引号：Program Files 下的空格会让系统把路径截断
        let val = format!("\"{}\"", exe.display());
        key.set_value(AUTOSTART_NAME, &val)
            .map_err(|e| format!("写入自启动项失败：{e}"))?;
    } else {
        match key.delete_value(AUTOSTART_NAME) {
            Ok(_) => {}
            // 本来就不存在 = 已经是关闭状态，不算失败
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("移除自启动项失败：{e}")),
        }
    }
    Ok(())
}

// ---------------- 命令 ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub close_action: String,
    pub autostart: bool,
    pub tray_available: bool,
    pub app_version: String,
    /// 当前程序路径（设置页展示「自启动将指向哪个文件」，开发态能看到 target\debug）
    pub exe_path: String,
    /// 进入目录后自动分析这一层
    pub auto_analyze_layer: bool,
    /// 允许把当前页面数据摘要作为 AI 上下文
    pub allow_page_context: bool,
    /// 免责声明是否已确认（false 时首次进入会自动弹出）
    pub disclaimer_accepted: bool,
}

#[tauri::command]
pub fn get_app_settings() -> SettingsView {
    SettingsView {
        close_action: close_action(),
        autostart: autostart_enabled(),
        tray_available: tray_ready(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        exe_path: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        auto_analyze_layer: AUTO_ANALYZE.load(Ordering::Acquire),
        allow_page_context: ALLOW_CTX.load(Ordering::Acquire),
        disclaimer_accepted: DISCLAIMER_ACCEPTED.load(Ordering::Acquire),
    }
}

/// 用户在弹窗里读完并点击「我已阅读并同意」后调用。
/// 只在第一次真正落盘；重复调用不产生额外写入。
#[tauri::command]
pub fn accept_disclaimer() -> Result<bool, String> {
    if DISCLAIMER_ACCEPTED.swap(true, Ordering::Release) {
        return Ok(true);
    }
    persist();
    Ok(DISCLAIMER_ACCEPTED.load(Ordering::Acquire))
}

/// 更新 AI 相关的两个开关。传 None 表示该项不变。
#[tauri::command]
pub fn update_prefs(
    auto_analyze_layer: Option<bool>,
    allow_page_context: Option<bool>,
) -> Result<SettingsView, String> {
    if let Some(v) = auto_analyze_layer {
        AUTO_ANALYZE.store(v, Ordering::Release);
    }
    if let Some(v) = allow_page_context {
        ALLOW_CTX.store(v, Ordering::Release);
    }
    persist();
    Ok(get_app_settings())
}

#[tauri::command]
pub fn update_close_action(action: String) -> Result<(), String> {
    set_close_action(&action)
}

#[tauri::command]
pub fn update_autostart(enabled: bool) -> Result<bool, String> {
    set_autostart(enabled)?;
    // 回读注册表，把真实结果告诉前端（而不是假设写入成功）
    Ok(autostart_enabled())
}

/// 关闭弹窗里用户做出选择后调用
#[tauri::command]
pub fn resolve_close(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    action: String,
    remember: bool,
) -> Result<(), String> {
    if remember {
        // 记住失败不阻断关闭动作（例如选了后台但托盘不可用，仍应正常退出）
        let _ = set_close_action(&action);
    }
    if action == CLOSE_BACKGROUND && tray_ready() {
        window.hide().map_err(|e| format!("隐藏窗口失败：{e}"))?;
        Ok(())
    } else {
        app.exit(0);
        Ok(())
    }
}

/// 从托盘/其他入口把主窗口调回前台
#[tauri::command]
pub fn show_main_window(app: tauri::AppHandle) {
    show_main(&app);
}

pub fn show_main(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 老版本写下的 settings.json 里没有 disclaimerAccepted。
    /// 如果反序列化整体失败，init() 会退回默认值 —— 用户的关闭行为与两个 AI 开关会被静默重置。
    /// 这条测试锁的是「升级不丢配置」。
    #[test]
    fn old_settings_without_disclaimer_still_loads() {
        let txt = r#"{"closeAction":"exit","autoAnalyzeLayer":false,"allowPageContext":false}"#;
        let s: AppSettings = serde_json::from_str(txt).expect("旧配置必须能解析");
        assert_eq!(s.close_action, "exit");
        assert!(!s.auto_analyze_layer);
        assert!(!s.allow_page_context);
        assert!(!s.disclaimer_accepted, "缺该字段时应视为未确认，升级后弹一次");
    }

    /// 确认状态必须能落盘并原样读回，否则每次启动都会重新弹窗
    #[test]
    fn disclaimer_flag_roundtrip() {
        let s = AppSettings {
            close_action: CLOSE_ASK.to_string(),
            auto_analyze_layer: true,
            allow_page_context: true,
            disclaimer_accepted: true,
        };
        let txt = serde_json::to_string(&s).unwrap();
        assert!(txt.contains("disclaimerAccepted"), "字段名必须是 camelCase：{txt}");
        let back: AppSettings = serde_json::from_str(&txt).unwrap();
        assert!(back.disclaimer_accepted);
    }
}
