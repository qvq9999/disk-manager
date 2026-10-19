//! 卸载残留：线索快照 → 扫描 → 清理
//!
//! 为什么要重写（v1.3.6）：
//! ① **线索在卸载前就丢了**。旧实现靠「用显示名反查 `Uninstall\<key>`」拿安装目录，
//!    但扫描发生在卸载**完成之后** —— 键已被卸载器删除，`install_dirs` 必然为空，
//!    只剩「目录名包含软件名」这一条路。本机实测这类目录只占全部应用目录的 1/5 左右。
//!    → 现在在**卸载之前**把整份线索快照写入 `app_config_dir/residual-clues.json`。
//! ② **只扫一层**。旧的目录扫描对 5 个根只做 `read_dir` 顶层；实测 Program Files 顶层 29 个
//!    目录里 23 个是厂商容器（`Adobe\`、`AMD\`），真正的产品在下一层。
//!    → 现在按「厂商\产品」两层枚举，并补上 `%LocalAppData%\Programs`、LocalLow、点目录。
//! ③ **注册表只扫顶层键**。实测 `HKCU\Software` 95 个顶层键里 71 个只是厂商容器。
//!    → 现在两层枚举，并补 WOW6432Node（显式 32/64 视图）、Classes、Uninstall 残项、App Paths。
//! ④ **匹配只有一个 contains**。→ 改成带权重的评分，并按置信度分级（低置信只列不勾）。
//!
//! 安全边界（删除前必过）：
//!   · 只认 id 里编码的**绝对路径**，不按名字猜路径（旧实现 `resolve_base_dir` 会猜错：
//!     装在 D 盘的残留一律失败；`%LocalAppData%\X` 与 `%AppData%\X` 同名时会删到 Roaming 那个）；
//!   · 路径必须在允许根之下、不是根本身、名字不在系统白名单；
//!   · 命中已安装软件线索的目录一律拒绝（防止扫描后被重装）。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, KEY_WRITE,
};
use winreg::{HKEY, RegKey};

use super::fsx;
use super::uninstall::{dir_stats, is_whitelisted, reg_str};

// ============================ 数据结构 ============================

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResidualItem {
    /// `"{kind}|{payload}"`。payload 是**真实目标**（绝对路径 / 带视图的注册表路径），
    /// 清理端只认它，绝不再从展示文案里反推。
    pub id: String,
    /// 展示用文案（可带 `%AppData%` 这类友好前缀）
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 匹配置信度 0~100。>=60 默认勾选；40~59 只列出、默认不勾。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    /// 清理后回填：已登记「重启后删除」
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reboot: Option<bool>,
    /// 清理后回填：失败原因
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CleanItemResult {
    pub path: String,
    pub ok: bool,
    pub freed: u64,
    pub pending_reboot: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub freed: u64,
    pub ok: u32,
    pub failed: u32,
    pub pending_reboot: u32,
    pub items: Vec<CleanItemResult>,
}

// ============================ 线索快照 ============================

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub(crate) struct Clue {
    id: String,
    name: String,
    publisher: String,
    install_location: String,
    display_icon: String,
    uninstall_string: String,
    /// 卸载注册表键名（MSI 是 {GUID}，其余常等于产品目录名）
    key: String,
    /// 快照时间（Unix 秒），用于过期清理
    at: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const CLUE_TTL_SECS: u64 = 30 * 24 * 3600;

fn clues_path(dir: &Path) -> PathBuf {
    dir.join("residual-clues.json")
}

fn load_clues(dir: &Path) -> Vec<Clue> {
    let p = clues_path(dir);
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        entries: Vec<Clue>,
    }
    let mut v = serde_json::from_str::<File>(&text).map(|f| f.entries).unwrap_or_default();
    let now = now_secs();
    v.retain(|c| now.saturating_sub(c.at) < CLUE_TTL_SECS);
    v
}

/// 写入/更新一条线索。整体重写文件：条数上限 300，超出按时间淘汰最旧的。
fn save_clue(dir: &Path, clue: Clue) {
    if clue.id.is_empty() || clue.name.is_empty() {
        return;
    }
    let mut all = load_clues(dir);
    all.retain(|c| c.id != clue.id);
    all.push(Clue { at: if clue.at == 0 { now_secs() } else { clue.at }, ..clue });
    if all.len() > 300 {
        all.sort_by(|a, b| b.at.cmp(&a.at));
        all.truncate(300);
    }
    #[derive(Serialize)]
    struct File<'a> {
        entries: &'a [Clue],
    }
    if let Ok(text) = serde_json::to_string(&File { entries: &all }) {
        let dir_ok = std::fs::create_dir_all(dir).is_ok();
        if dir_ok {
            // 先写临时文件再改名，避免进程被杀时留下半个 JSON
            let tmp = clues_path(dir).with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, clues_path(dir));
            }
        }
    }
}

/// 从**已打开的**卸载注册表键抓取线索（在卸载动作之前调用，这是本模块的关键）。
pub fn clue_from_key(sub: &RegKey, app_id: &str, key_name: &str) -> Clue {
    Clue {
        id: app_id.to_string(),
        name: reg_str(sub, "DisplayName"),
        publisher: reg_str(sub, "Publisher"),
        install_location: reg_str(sub, "InstallLocation"),
        display_icon: reg_str(sub, "DisplayIcon"),
        uninstall_string: reg_str(sub, "UninstallString"),
        key: key_name.to_string(),
        at: now_secs(),
    }
}

/// 在卸载之前快照线索。`dir` 为应用配置目录（拿不到就跳过，扫描端会退回纯名称匹配）。
pub fn snapshot_before_uninstall(dir: Option<&Path>, clue: Clue) {
    if let Some(d) = dir {
        save_clue(d, clue);
    }
}

// ============================ 匹配评分 ============================

#[derive(Clone)]
struct Needle {
    text: String,
    weight: f32,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// 归一化：小写、去掉括号内容、只保留字母数字与汉字。
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '（' | '[' | '【' => depth += 1,
            ')' | '）' | ']' | '】' => depth = (depth - 1).max(0),
            _ => {
                if depth == 0 {
                    for lc in c.to_lowercase() {
                        if lc.is_alphanumeric() {
                            out.push(lc);
                        }
                    }
                }
            }
        }
    }
    out
}

/// 拆词：ASCII 字母数字连成一段，每个汉字单独成词。
fn tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(c.to_string());
        } else if c.is_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 双字组 Dice 系数，用于拼写差异（`HBuilder X` vs `HBuilderX`、`Photoshop 2024` vs `Photoshop`）。
fn dice(a: &str, b: &str) -> f32 {
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    if ca.len() < 2 || cb.len() < 2 {
        return if a == b { 1.0 } else { 0.0 };
    }
    let mut ga: Vec<String> = Vec::new();
    for w in ca.windows(2) {
        ga.push(w.iter().collect());
    }
    let mut gb: Vec<String> = Vec::new();
    for w in cb.windows(2) {
        gb.push(w.iter().collect());
    }
    let set_b: HashSet<&String> = gb.iter().collect();
    let inter = ga.iter().filter(|g| set_b.contains(g)).count();
    (2 * inter) as f32 / (ga.len() + gb.len()) as f32
}

/// 单个 needle 对目标名的匹配质量。
fn quality(hay: &str, needle: &str) -> u8 {
    let n = needle.chars().count();
    if n < 2 {
        return 0;
    }
    let q = if hay == needle {
        100
    } else if hay.starts_with(needle) || hay.ends_with(needle) {
        85
    } else if hay.contains(needle) {
        // 短词包含容易误报（"qq" 命中一切），只在够长时给高分
        if n >= 4 { 70 } else { 55 }
    } else {
        let toks = tokens(needle);
        let meaningful: Vec<&String> = toks.iter().filter(|t| t.chars().count() >= 2).collect();
        if meaningful.len() >= 2 && meaningful.iter().all(|t| hay.contains(t.as_str())) {
            62
        } else {
            let d = dice(hay, needle);
            if d >= 0.78 {
                48
            } else if d >= 0.62 {
                42
            } else {
                0
            }
        }
    };
    // 纯 ASCII 且不足 3 字符：再准也不给"默认勾选"级别的分（"QQ"、"7z" 这类短名靠其它线索）
    if n < 3 && needle.chars().all(|c| c.is_ascii()) {
        return q.min(55);
    }
    q
}

fn score_of(name: &str, needles: &[Needle]) -> u8 {
    let h = norm(name);
    if h.is_empty() {
        return 0;
    }
    needles
        .iter()
        .map(|n| (quality(&h, &n.text) as f32 * n.weight).min(100.0) as u8)
        .max()
        .unwrap_or(0)
}

fn push_needle(v: &mut Vec<Needle>, text: &str, weight: f32) {
    let t = norm(text);
    let n = t.chars().count();
    if n < 2 {
        return;
    }
    // 纯 ASCII 短词（<3）几乎必然误报
    if n < 3 && t.chars().all(|c| c.is_ascii()) {
        return;
    }
    if v.iter().any(|x| x.text == t) {
        return;
    }
    v.push(Needle { text: t, weight });
}

/// 从「软件名 + 快照线索」构建匹配词表。权重决定同等匹配质量下的优先级。
fn build_needles(app_name: &str, clue: Option<&Clue>) -> Vec<Needle> {
    let mut v: Vec<Needle> = Vec::new();
    push_needle(&mut v, app_name, 1.0);
    // 首词（"Microsoft Visual Studio Code" → "microsoft" 太泛，长度>=4 才收）
    if let Some(first) = app_name.split([' ', '-', '_', '·']).next() {
        if first != app_name && first.chars().count() >= 4 {
            push_needle(&mut v, first, 0.9);
        }
    }
    if let Some(c) = clue {
        push_needle(&mut v, &c.name, 1.0);
        push_needle(&mut v, &c.publisher, 0.8);
        // 键名：MSI 的 {GUID} 无意义，其余常等于产品目录名
        if !c.key.starts_with('{') {
            push_needle(&mut v, &c.key, 0.95);
        }
        for raw in [&c.install_location, &c.display_icon, &c.uninstall_string] {
            if raw.trim().is_empty() {
                continue;
            }
            if let Some(dir) = dir_of(raw) {
                if let Some(n) = dir.file_name() {
                    push_needle(&mut v, &n.to_string_lossy(), 0.95);
                }
                if let Some(p) = dir.parent().and_then(|x| x.file_name()) {
                    push_needle(&mut v, &p.to_string_lossy(), 0.7);
                }
                for f in walk_names(&dir, 2, 40) {
                    let stem = f.rsplit_once('.').map(|(a, _)| a).unwrap_or(&f);
                    if stem.to_lowercase().contains("uninst") {
                        continue;
                    }
                    push_needle(&mut v, stem, 0.75);
                }
            }
        }
    }
    v
}

fn dir_of(raw: &str) -> Option<PathBuf> {
    let cleaned = raw.trim().trim_matches('"');
    let first = cleaned.split(',').next().unwrap_or("").trim().trim_matches('"');
    let first = first.split_whitespace().next().unwrap_or(first).trim_matches('"');
    if first.is_empty() {
        return None;
    }
    let p = PathBuf::from(first);
    if p.is_dir() {
        Some(p)
    } else if p.is_file() {
        p.parent().map(|x| x.to_path_buf())
    } else {
        None
    }
}

/// 取目录内前 n 个文件名（浅层），用于补充线索
fn walk_names(dir: &Path, depth: usize, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if out.len() >= max {
            break;
        }
        if e.file_type().is_file() {
            if let Some(n) = e.path().file_name() {
                out.push(n.to_string_lossy().to_string());
            }
        }
    }
    out
}

// ============================ 目录扫描 ============================

/// 共享容器：这些目录里放的是"很多软件共用的东西"，绝不能整体当作某一个软件的残留。
const CONTAINERS: &[&str] = &[
    "common files", "windowsapps", "modifiablewindowsapps", "microsoft", "packages", "temp",
    "tempstate", "virtualstore", "programs", "windows", "system32", "syswow64", "$recycle.bin",
    "application data", "desktop", "documents", "downloads", "start menu", "templates",
    "internet explorer", "common", "connecteddevicesplatform", "comms", "d3dscache", "publishers",
];

fn is_container(name: &str) -> bool {
    let l = name.to_lowercase();
    CONTAINERS.iter().any(|c| l == *c || l == format!("{c}\\").as_str())
}

struct Root {
    tag: &'static str,
    path: PathBuf,
    depth: usize,
    /// 只接受以 `.` 开头的目录（`%USERPROFILE%` 下的点目录）
    dot_only: bool,
}

fn roots() -> Vec<Root> {
    let mut v: Vec<Root> = Vec::new();
    let mut add = |tag: &'static str, var: &str, suffix: Option<&str>, depth: usize, dot_only: bool| {
        if let Ok(base) = std::env::var(var) {
            if base.trim().is_empty() {
                return;
            }
            let mut p = PathBuf::from(base);
            if let Some(s) = suffix {
                p = p.join(s);
            }
            if p.is_dir() {
                v.push(Root { tag, path: p, depth, dot_only });
            }
        }
    };
    add("C:\\Program Files", "ProgramFiles", None, 2, false);
    add("C:\\Program Files (x86)", "ProgramFiles(x86)", None, 2, false);
    add("%AppData%", "APPDATA", None, 2, false);
    add("%LocalAppData%", "LOCALAPPDATA", None, 2, false);
    // Electron / Squirrel 系默认装在这里，旧实现完全没扫
    add("%LocalAppData%\\Programs", "LOCALAPPDATA", Some("Programs"), 1, false);
    add("%LocalAppData%Low", "USERPROFILE", Some(r"AppData\LocalLow"), 1, false);
    add("%ProgramData%", "ProgramData", None, 2, false);
    add("%UserProfile%", "USERPROFILE", None, 1, true);
    v
}

fn allowed_roots_canon() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for (var, suffix) in [
        ("ProgramFiles", None),
        ("ProgramFiles(x86)", None),
        ("APPDATA", None),
        ("LOCALAPPDATA", None),
        ("LOCALAPPDATA", Some("Programs")),
        ("ProgramData", None),
        ("USERPROFILE", None),
    ] {
        if let Ok(base) = std::env::var(var) {
            let mut p = PathBuf::from(base);
            if let Some(s) = suffix {
                p = p.join(s);
            }
            if let Ok(c) = std::fs::canonicalize(&p) {
                v.push(c);
            }
        }
    }
    // 快捷方式 / 程序组所在位置：它们不在上面任何根的「直接子目录」层，
    // 必须单独列为允许根，否则开始菜单里的残留永远删不掉。
    for (var, suffix) in [
        ("APPDATA", Some(r"Microsoft\Windows\Start Menu\Programs")),
        ("ProgramData", Some(r"Microsoft\Windows\Start Menu\Programs")),
        ("USERPROFILE", Some("Desktop")),
        ("PUBLIC", Some("Desktop")),
    ] {
        if let Ok(base) = std::env::var(var) {
            let mut p = PathBuf::from(base);
            if let Some(s) = suffix {
                p = p.join(s);
            }
            if let Ok(c) = std::fs::canonicalize(&p) {
                v.push(c);
            }
        }
    }
    v
}

/// 删除前的最后一道闸。路径必须真实存在、位于允许根之下（最多两层）、不是根本身、
/// 名字既不在系统白名单也不属于共享容器。
///
/// 这道闸在**清理时重新执行**，不信任前端传回的路径：
/// 扫描与清理是两次调用，中间软件可能被重装、目录可能被替换成 junction。
fn path_is_safe(p: &Path, roots: &[PathBuf]) -> bool {
    if !p.is_absolute() {
        return false;
    }
    let Ok(canon) = std::fs::canonicalize(p) else { return false };
    let ok = roots.iter().any(|r| {
        if canon == *r {
            return false;
        }
        let Some(par) = canon.parent() else { return false };
        if par == r.as_path() {
            return true;
        }
        // 厂商\产品 这种两层结构
        par.starts_with(r) && canon.components().count() == r.components().count() + 2
    });
    if !ok {
        return false;
    }
    let name = canon.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.chars().count() < 3 {
        return false;
    }
    if is_whitelisted(&name) || is_container(&name) {
        return false;
    }
    true
}

fn scan_dirs(needles: &[Needle], out: &mut Vec<ResidualItem>) {
    let mut listed: Vec<PathBuf> = Vec::new();
    let mut push_dir = |p: &Path, tag: &str, conf: u8, detail: &str, out: &mut Vec<ResidualItem>| {
        if p.exists() && !listed.iter().any(|x| x == p) {
            let (size, files) = dir_stats(&p.to_path_buf(), 4);
            listed.push(p.to_path_buf());
            out.push(ResidualItem {
                id: format!("folder|{}", p.to_string_lossy()),
                path: format!("{tag}"),
                kind: "folder".into(),
                size,
                selected: conf >= 60,
                detail: Some(if detail.is_empty() {
                    format!("{files} 个文件 · 匹配度 {conf}%")
                } else {
                    format!("{detail} · {files} 个文件 · 匹配度 {conf}%")
                }),
                confidence: Some(conf),
                pending_reboot: None,
                reason: None,
            });
        }
    };

    for root in roots() {
        let Ok(rd) = std::fs::read_dir(&root.path) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let raw_name = e.file_name().to_string_lossy().to_string();
            if p.is_symlink() {
                continue;
            }
            if root.dot_only && !raw_name.starts_with('.') {
                continue;
            }
            if is_container(&raw_name) {
                continue;
            }
            if root.dot_only && root.depth == 1 {
                // 点目录只按高置信收录
                let conf = score_of(&raw_name, needles);
                if conf >= 60 {
                    push_dir(&p, &format!("{}\\{}", root.tag, raw_name), conf, "", out);
                }
                continue;
            }
            let conf = score_of(&raw_name, needles);
            if conf >= 60 {
                push_dir(&p, &format!("{}\\{}", root.tag, raw_name), conf, "", out);
                continue;
            }
            if root.depth < 2 {
                continue;
            }
            // 第二层：厂商\产品（Adobe\Photoshop、aha_doctor\Doubao）
            let Ok(rd2) = std::fs::read_dir(&p) else { continue };
            let mut sub_n = 0;
            for e2 in rd2.flatten() {
                sub_n += 1;
                if sub_n > 120 {
                    break;
                }
                let p2 = e2.path();
                if !p2.is_dir() || p2.is_symlink() {
                    continue;
                }
                let n2 = e2.file_name().to_string_lossy().to_string();
                if is_container(&n2) {
                    continue;
                }
                let c2 = score_of(&n2, needles);
                if c2 >= 40 {
                    let detail = format!("位于 {} 下", raw_name);
                    push_dir(
                        &p2,
                        &format!("{}\\{}\\{}", root.tag, raw_name, n2),
                        c2,
                        &detail,
                        out,
                    );
                }
            }
        }
    }
}

// ============================ 注册表扫描 ============================

fn open(hive: HKEY, path: &str, view: u32) -> Option<RegKey> {
    RegKey::predef(hive)
        .open_subkey_with_flags(path, KEY_READ | view)
        .ok()
}

fn push_reg(out: &mut Vec<ResidualItem>, payload: String, shown: String, kind: &str, conf: u8, detail: String) {
    out.push(ResidualItem {
        id: format!("{kind}|{payload}"),
        path: shown,
        kind: kind.into(),
        size: 0,
        selected: conf >= 60,
        detail: Some(format!("{detail} · 匹配度 {conf}%")),
        confidence: Some(conf),
        pending_reboot: None,
        reason: None,
    });
}

/// 注册表：两层枚举 + 更多位置
fn scan_registry(needles: &[Needle], out: &mut Vec<ResidualItem>) {
    // ① Software 及其下一层（两个 32/64 视图）
    for (hive, hive_tag, view) in [
        (HKEY_CURRENT_USER, "HKCU", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM32", KEY_WOW64_32KEY),
    ] {
        let Some(root) = open(hive, "Software", view) else { continue };
        for key in root.enum_keys().flatten() {
            if is_whitelisted(&key) {
                continue;
            }
            let conf = score_of(&key, needles);
            if conf >= 60 {
                let subs = root
                    .open_subkey_with_flags(&key, KEY_READ | view)
                    .map(|k| k.enum_keys().flatten().count())
                    .unwrap_or(0);
                push_reg(
                    out,
                    format!("{hive_tag}\\Software\\{key}"),
                    format!("{hive_tag}\\Software\\{key}"),
                    "registry",
                    conf,
                    format!("软件配置键 · {subs} 个子键"),
                );
                continue;
            }
            // 第二层：Vendor\Product
            let Ok(vendor) = root.open_subkey_with_flags(&key, KEY_READ | view) else { continue };
            let mut n = 0;
            for prod in vendor.enum_keys().flatten() {
                n += 1;
                if n > 150 {
                    break;
                }
                if is_whitelisted(&prod) {
                    continue;
                }
                let c2 = score_of(&prod, needles);
                if c2 >= 40 {
                    push_reg(
                        out,
                        format!("{hive_tag}\\Software\\{key}\\{prod}"),
                        format!("{hive_tag}\\Software\\{key}\\{prod}"),
                        "registry",
                        c2,
                        format!("位于 {} 下的配置键", key),
                    );
                }
            }
        }
    }

    // ② Classes：ProgID / App Paths
    for (hive, hive_tag, view) in [
        (HKEY_CURRENT_USER, "HKCU", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM32", KEY_WOW64_32KEY),
    ] {
        if let Some(cls) = open(hive, r"Software\Classes", view) {
            for key in cls.enum_keys().flatten() {
                let dots = key.matches('.').count();
                let looks_progid = (1..=3).contains(&dots)
                    && key.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
                    && !key.starts_with("CLSID")
                    && !key.starts_with("TypeLib")
                    && !key.starts_with("Interface")
                    && !key.starts_with("AppID");
                if !looks_progid || is_whitelisted(&key) {
                    continue;
                }
                let conf = score_of(&key, needles);
                if conf >= 60 {
                    push_reg(
                        out,
                        format!("{hive_tag}\\Software\\Classes\\{key}"),
                        format!("{hive_tag}\\Classes\\{key}"),
                        "class",
                        conf,
                        "COM / 文件关联键".into(),
                    );
                }
            }
        }
        if let Some(ap) = open(hive, r"Software\Microsoft\Windows\CurrentVersion\App Paths", view) {
            for k in ap.enum_keys().flatten() {
                let conf = score_of(&k, needles);
                if conf >= 60 {
                    push_reg(
                        out,
                        format!("{hive_tag}\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{k}"),
                        format!("{hive_tag}\\App Paths\\{k}"),
                        "registry",
                        conf,
                        "程序注册路径（App Paths）".into(),
                    );
                }
            }
        }
    }

    // ③ 卸载登记残项：卸载器自己留下的 Uninstall 键（最常见的残留之一）
    for (hive, hive_tag, view) in [
        (HKEY_CURRENT_USER, "HKCU", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM32", KEY_WOW64_32KEY),
    ] {
        let path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
        let Some(root) = open(hive, path, view) else { continue };
        for key in root.enum_keys().flatten() {
            let Ok(sub) = root.open_subkey_with_flags(&key, KEY_READ | view) else { continue };
            let dn = reg_str(&sub, "DisplayName");
            // GUID 键名本身没有语义，只有 DisplayName 可用来匹配
            let conf = score_of(&dn, needles).max(if key.starts_with('{') {
                0
            } else {
                score_of(&key, needles)
            });
            if conf < 60 {
                continue;
            }
            let label = if dn.is_empty() { key.clone() } else { dn.clone() };
            push_reg(
                out,
                format!("{hive_tag}\\{path}\\{key}"),
                format!("{hive_tag}\\Uninstall\\{label}"),
                "uninstallkey",
                conf,
                "卸载登记项（软件已移除但登记还在）".into(),
            );
        }
    }

    // ④ 服务 + 事件日志源
    if let Some(svc) = open(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services", KEY_WOW64_64KEY) {
        for key in svc.enum_keys().flatten() {
            if is_whitelisted(&key) {
                continue;
            }
            let conf = score_of(&key, needles);
            if conf >= 60 {
                push_reg(
                    out,
                    format!("HKLM\\SYSTEM\\CurrentControlSet\\Services\\{key}"),
                    format!("HKLM\\Services\\{key}"),
                    "service",
                    conf,
                    "后台服务键（清理需管理员权限）".into(),
                );
            }
        }
    }
    if let Some(el) = open(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Services\EventLog\Application",
        KEY_WOW64_64KEY,
    ) {
        for key in el.enum_keys().flatten() {
            let conf = score_of(&key, needles);
            if conf >= 60 {
                push_reg(
                    out,
                    format!("HKLM\\SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\{key}"),
                    format!("HKLM\\EventLog\\{key}"),
                    "registry",
                    conf.min(70),
                    "事件日志源残留".into(),
                );
            }
        }
    }

    // ⑤ 开机启动项（含 WOW6432Node 与 RunOnce）
    for (hive, tag, path, view) in [
        (HKEY_CURRENT_USER, "HKCU", r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce", KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, "HKLM32", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", KEY_WOW64_32KEY),
    ] {
        let Some(run) = open(hive, path, view) else { continue };
        for (vname, _) in run.enum_values().flatten() {
            let conf = score_of(&vname, needles);
            if conf >= 60 {
                out.push(ResidualItem {
                    id: format!("startup|{tag}|{path}|{vname}"),
                    path: format!("{tag}\\Run\\{vname}"),
                    kind: "startup".into(),
                    size: 0,
                    selected: true,
                    detail: Some(format!("开机自启动项：{vname}")),
                    confidence: Some(conf),
                    pending_reboot: None,
                    reason: None,
                });
            }
        }
    }

    // ⑥ PATH 环境变量中的残留段
    for (hive, path, tag) in [
        (HKEY_CURRENT_USER, r"Environment", "HKCU"),
        (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "HKLM"),
    ] {
        let Some(k) = open(hive, path, KEY_WOW64_64KEY) else { continue };
        let Ok(v) = k.get_value::<String, _>("Path") else { continue };
        for seg in v.split(';') {
            let s = seg.trim();
            if s.is_empty() {
                continue;
            }
            let conf = score_of(s, needles);
            if conf >= 60 && !out.iter().any(|i| i.kind == "pathenv" && i.id.ends_with(s)) {
                out.push(ResidualItem {
                    id: format!("pathenv|{tag}|{s}"),
                    path: format!("{tag}\\Path 片段：{s}"),
                    kind: "pathenv".into(),
                    size: 0,
                    selected: true,
                    detail: Some("PATH 环境变量残留目录段".into()),
                    confidence: Some(conf),
                    pending_reboot: None,
                    reason: None,
                });
            }
        }
    }
}

// ============================ 文件型残留（快捷方式 / 启动文件夹） ============================

fn scan_shortcuts(needles: &[Needle], out: &mut Vec<ResidualItem>) {
    // (标签, 目录, 是否允许把「子目录」也当作残留)
    //
    // 只有开始菜单的「程序组文件夹」允许整目录删除（里面只放快捷方式）。
    // 桌面 / 启动文件夹上的目录**一律不动** —— 那更可能是用户自己的文件夹，
    // 名字碰巧和软件名相似，删了就是丢用户资料。
    let mut dirs: Vec<(&str, PathBuf, bool)> = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        let base = PathBuf::from(&a).join(r"Microsoft\Windows\Start Menu\Programs");
        dirs.push(("开始菜单", base.clone(), true));
        dirs.push(("启动文件夹", base.join("Startup"), false));
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        dirs.push(("公共桌面", PathBuf::from(&p).join("Desktop"), false));
        dirs.push((
            "公共开始菜单",
            PathBuf::from(&p).join(r"Microsoft\Windows\Start Menu\Programs"),
            true,
        ));
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        dirs.push((
            "程序数据开始菜单",
            PathBuf::from(&pd).join(r"Microsoft\Windows\Start Menu\Programs"),
            true,
        ));
    }
    if let Ok(u) = std::env::var("USERPROFILE") {
        dirs.push(("桌面", PathBuf::from(&u).join("Desktop"), false));
    }
    for (tag, dir, allow_dir) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            let low = fname.to_lowercase();
            let is_lnk = low.ends_with(".lnk") || low.ends_with(".url");
            let is_dir = e.path().is_dir();
            if !is_lnk && !(is_dir && allow_dir) {
                continue;
            }
            let stem = if is_lnk {
                fname.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(fname.clone())
            } else {
                fname.clone()
            };
            let conf = score_of(&stem, needles);
            if conf < 60 {
                continue;
            }
            let (size, files) = if is_dir {
                dir_stats(&e.path(), 4)
            } else {
                (e.metadata().map(|m| m.len()).unwrap_or(0), 1)
            };
            out.push(ResidualItem {
                id: format!("{}|{}", if is_dir { "folder" } else { "shortcut" }, e.path().to_string_lossy()),
                path: e.path().to_string_lossy().to_string(),
                kind: if is_dir { "folder".into() } else { "shortcut".into() },
                size,
                selected: true,
                detail: Some(format!("{} · {} 个文件 · 匹配度 {conf}%", tag, files)),
                confidence: Some(conf),
                pending_reboot: None,
                reason: None,
            });
        }
    }
}

// ============================ 系统项（计划任务 / 防火墙） ============================

/// 计划任务：直接读 `TaskCache\Tree` 注册表树。
///
/// 为什么不用 `Get-ScheduledTask`：实测单次 2056ms（其中约 2s 是 PowerShell 自身的启动开销），
/// 而这段代码每次卸载后都要跑一遍。注册表遍历只要几毫秒，结果还更完整
/// （PowerShell 会过滤掉部分系统任务，做残留检测恰恰需要看到它们）。
fn scan_tasks(needles: &[Needle], out: &mut Vec<(String, u8)>) {
    fn walk(root: &RegKey, prefix: &str, depth: usize, needles: &[Needle], out: &mut Vec<(String, u8)>) {
        if depth > 5 || out.len() >= 40 {
            return;
        }
        for key in root.enum_keys().flatten() {
            let path = format!("{prefix}\\{key}");
            let conf = score_of(&key, needles);
            if conf >= 60 {
                out.push((path.clone(), conf));
            }
            // 继续下钻：任务树里目录与任务混在同一层，目录还会再分一层
            if let Ok(sub) = root.open_subkey(&key) {
                walk(&sub, &path, depth + 1, needles, out);
            }
        }
    }

    let Some(tree) = open(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree",
        KEY_WOW64_64KEY,
    ) else {
        return;
    };
    walk(&tree, "", 0, needles, out);
}

/// 防火墙规则：读 `FirewallPolicy\FirewallRules` 的注册表值（比 Get-NetFirewallRule 快 200 倍）。
///
/// 值的格式是 `v2.30|Action=Allow|Dir=In|App=C:\x\y.exe|Name=xxx|Desc=显示名|...`。
/// 匹配两个字段：`Desc`（界面显示名）与 `App`（程序路径 —— 比名字可靠得多，
/// 名字可以是任意值，路径一定指向安装目录）。
fn scan_firewall(needles: &[Needle], out: &mut Vec<(String, String, u8)>) {
    let Some(key) = open(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules",
        KEY_WOW64_64KEY,
    ) else {
        return;
    };
    for (vname, _) in key.enum_values().flatten() {
        if out.len() >= 40 {
            break;
        }
        let Ok(raw) = key.get_raw_value(&vname) else { continue };
        if raw.bytes.is_empty() {
            continue;
        }
        // 值都是 REG_SZ（UTF-16LE）；个别老系统为 ANSI，统一按 UTF-16 解，失败再退 GB18030
        let text = {
            let mut units: Vec<u16> = Vec::with_capacity(raw.bytes.len() / 2);
            let mut i = 0;
            while i + 1 < raw.bytes.len() {
                units.push(u16::from_le_bytes([raw.bytes[i], raw.bytes[i + 1]]));
                i += 2;
            }
            String::from_utf16_lossy(&units)
        };
        let text = text.trim_end_matches('\0');
        let mut name = String::new();
        let mut desc = String::new();
        let mut app = String::new();
        for field in text.split('|') {
            if let Some(v) = field.strip_prefix("Name=") {
                name = v.to_string();
            } else if let Some(v) = field.strip_prefix("Desc=") {
                desc = v.to_string();
            } else if let Some(v) = field.strip_prefix("App=") {
                app = v.to_string();
            }
        }
        let label = if desc.is_empty() { name.clone() } else { desc.clone() };
        if label.is_empty() {
            continue;
        }
        let conf = score_of(&label, needles).max(score_of(&app, needles));
        if conf >= 60 {
            out.push((vname.clone(), label, conf));
        }
    }
}

/// PowerShell 兜底：注册表方式取不到任何结果时（极少数系统）才走这条老路
fn scan_system_items_ps(needles: &[Needle]) -> (Vec<(String, u8)>, Vec<(String, u8)>) {
    if needles.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let script = format!(
        "{} Get-ScheduledTask | ForEach-Object {{ \"T`t$($_.TaskPath)$($_.TaskName)\" }}; \
         Get-NetFirewallRule | ForEach-Object {{ \"F`t$($_.DisplayName)\" }}",
        crate::commands::PS_UTF8_PRELUDE
    );
    let Ok(out) = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    else {
        return (Vec::new(), Vec::new());
    };
    let text = crate::commands::decode_out(&out.stdout);
    let mut tasks = Vec::new();
    let mut fws = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((tag, rest)) = line.split_once('\t') else { continue };
        let rest = rest.trim();
        if rest.is_empty() || rest == "\\" {
            continue;
        }
        let conf = score_of(rest, needles);
        if conf < 60 {
            continue;
        }
        match tag {
            "T" => {
                if tasks.len() < 40 {
                    tasks.push((rest.to_string(), conf))
                }
            }
            "F" => {
                if fws.len() < 40 {
                    fws.push((rest.to_string(), conf))
                }
            }
            _ => {}
        }
    }
    (tasks, fws)
}

// ============================ 扫描入口 ============================

fn scan_impl(app_name: String, app_id: Option<String>, app_dir: Option<PathBuf>) -> Vec<ResidualItem> {
    let mut items: Vec<ResidualItem> = Vec::new();

    let clue = app_dir
        .as_deref()
        .map(load_clues)
        .unwrap_or_default()
        .into_iter()
        .find(|c| {
            app_id.as_deref().map(|id| c.id == id).unwrap_or(false)
                || c.name.eq_ignore_ascii_case(&app_name)
        });

    let needles = build_needles(&app_name, clue.as_ref());
    if needles.is_empty() {
        return items;
    }

    // ① 快照里的原安装目录：最高置信，直接可删（这也是旧实现几乎拿不到的一类）
    if let Some(c) = &clue {
        for raw in [&c.install_location, &c.display_icon, &c.uninstall_string] {
            if raw.trim().is_empty() {
                continue;
            }
            let Some(d) = dir_of(raw) else { continue };
            let Ok(canon) = std::fs::canonicalize(&d) else { continue };
            if !path_is_safe(&canon, &allowed_roots_canon()) {
                continue;
            }
            if items.iter().any(|i| i.id == format!("folder|{}", canon.to_string_lossy())) {
                continue;
            }
            let (size, files) = dir_stats(&canon, 4);
            items.push(ResidualItem {
                id: format!("folder|{}", canon.to_string_lossy()),
                path: canon.to_string_lossy().to_string(),
                kind: "folder".into(),
                size,
                selected: true,
                detail: Some(format!("原安装目录（卸载前快照） · {files} 个文件")),
                confidence: Some(100),
                pending_reboot: None,
                reason: None,
            });
        }
    }

    // ② 目录 / 注册表 / 快捷方式（覆盖 5 个根的两层结构与全部常用注册表位置）
    scan_dirs(&needles, &mut items);
    scan_registry(&needles, &mut items);
    scan_shortcuts(&needles, &mut items);

    // ③ 计划任务与防火墙都直接读注册表（毫秒级）；
    //    只有两者都为空时才退回 PowerShell —— 正常机器上不会走到那一步。
    let mut tasks: Vec<(String, u8)> = Vec::new();
    let mut fws: Vec<(String, String, u8)> = Vec::new();
    scan_tasks(&needles, &mut tasks);
    scan_firewall(&needles, &mut fws);
    if tasks.is_empty() && fws.is_empty() {
        let (t, f) = scan_system_items_ps(&needles);
        tasks = t;
        fws = f.into_iter().map(|(n, c)| (n.clone(), n, c)).collect();
    }
    for (t, conf) in tasks {
        items.push(ResidualItem {
            id: format!("task|{t}"),
            path: format!("计划任务：{t}"),
            kind: "task".into(),
            size: 0,
            selected: true,
            detail: Some(format!("卸载后仍可能自动拉起该软件 · 匹配度 {conf}%")),
            confidence: Some(conf),
            pending_reboot: None,
            reason: None,
        });
    }
    for (rname, label, conf) in fws {
        items.push(ResidualItem {
            // payload 带规则名（唯一）+ 显示名（删除时的兜底）
            id: format!("firewall|{rname}|{label}"),
            path: format!("防火墙规则：{label}"),
            kind: "firewall".into(),
            size: 0,
            selected: true,
            detail: Some(format!("Windows Defender 防火墙规则 · 匹配度 {conf}%")),
            confidence: Some(conf),
            pending_reboot: None,
            reason: None,
        });
    }

    // 排序：置信度高 → 体积大 优先，让用户先看到该看的
    items.sort_by(|a, b| {
        b.confidence
            .unwrap_or(0)
            .cmp(&a.confidence.unwrap_or(0))
            .then(b.size.cmp(&a.size))
    });
    items
}

#[tauri::command]
pub async fn scan_app_residuals(
    app: tauri::AppHandle,
    app_name: String,
    app_id: Option<String>,
) -> Vec<ResidualItem> {
    use tauri::Manager;
    let dir = app.path().app_config_dir().ok();
    tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan_impl(app_name, app_id, dir)))
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// 内部调用入口（强制卸载等场景，不经过 IPC）
pub fn scan_for(app_name: &str, app_id: Option<&str>, dir: Option<&Path>) -> Vec<ResidualItem> {
    scan_impl(app_name.to_string(), app_id.map(|s| s.to_string()), dir.map(|p| p.to_path_buf()))
}

/// 卸载前主动快照（前端在调用卸载前调用；即使不调用，`uninstall_app` 内部也会做一次）
#[tauri::command]
pub async fn prepare_app_removal(app: tauri::AppHandle, id: String) -> bool {
    use tauri::Manager;
    let Some(dir) = app.path().app_config_dir().ok() else { return false };
    tauri::async_runtime::spawn_blocking(move || snapshot_by_id(&dir, &id))
        .await
        .unwrap_or(false)
}

/// 按 id 直接读注册表抓线索（用于强制卸载与前端主动预备）
pub fn snapshot_by_id(dir: &Path, id: &str) -> bool {
    let Some(key_name) = id.strip_prefix("win32:") else { return false };
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ] {
            let full = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            let Some(root) = open(hive, &full, KEY_WOW64_64KEY) else { continue };
            let Ok(sub) = root.open_subkey(key_name) else { continue };
            let clue = clue_from_key(&sub, id, key_name);
            if clue.name.is_empty() {
                continue;
            }
            save_clue(dir, clue);
            return true;
        }
    }
    false
}

// ============================ 清理 ============================

/// 递归删注册表键（含子键与值）。
///
/// 旧实现只删**一层**子键，两层嵌套的键必然删不掉；winreg 的 `delete_subkey_all`
/// 底层就是 `RegDeleteTreeW`，一次递归删干净。仍然失败时退回 `reg.exe delete /f`，
/// 它能覆盖一部分 winreg 处理不了的 ACL 场景。
fn reg_delete(hive: HKEY, view: u32, path: &str) -> Result<(), String> {
    let Some((parent_path, target)) = path.rsplit_once('\\') else {
        return Err("注册表路径无效".into());
    };
    let attempt = || -> Result<(), u32> {
        let parent = RegKey::predef(hive)
            .open_subkey_with_flags(parent_path, KEY_READ | KEY_WRITE | view)
            .map_err(|e| e.raw_os_error().unwrap_or(5) as u32)?;
        if let Ok(sub) = parent.open_subkey_with_flags(target, KEY_READ | KEY_WRITE | view) {
            sub.delete_subkey_all("").map_err(|e| e.raw_os_error().unwrap_or(5) as u32)?;
        }
        parent.delete_subkey(target).map_err(|e| e.raw_os_error().unwrap_or(5) as u32)
    };
    match attempt() {
        Ok(()) => Ok(()),
        Err(code) => {
            // 退回 reg.exe：对部分 ACL 受限的键更宽容
            let root = format!(
                "{}\\{path}",
                if hive == HKEY_CURRENT_USER { "HKCU" } else { "HKLM" }
            );
            let mut cmd = crate::commands::silent_command("reg");
            cmd.args(["delete", &root, "/f"]);
            cmd.arg(if view == KEY_WOW64_32KEY { "/reg:32" } else { "/reg:64" });
            let ok = cmd.output().map(|o| o.status.success()).unwrap_or(false);
            if ok {
                Ok(())
            } else {
                Err(match code {
                    5 => "权限拒绝（注册表键的安全描述符不允许修改）".into(),
                    2 | 3 => "键已不存在".to_string(),
                    c => format!("删除失败（WinError {c}）"),
                })
            }
        }
    }
}

/// 解析 id 里编码的注册表目标：`HKLM\...` / `HKLM32\...` / `HKCU\...`。
/// 32/64 视图必须在**扫描时**就确定下来并在清理时原样使用 ——
/// 旧实现把 `WOW6432Node` 从字符串里替换掉再去删，实际删的是 64 位视图的同名键。
fn reg_delete_payload(payload: &str) -> Result<(), String> {
    if let Some(rest) = payload.strip_prefix("HKLM32\\") {
        reg_delete(HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, rest)
    } else if let Some(rest) = payload.strip_prefix("HKLM\\") {
        reg_delete(HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, rest)
    } else if let Some(rest) = payload.strip_prefix("HKCU\\") {
        reg_delete(HKEY_CURRENT_USER, KEY_WOW64_64KEY, rest)
    } else {
        Err("注册表路径缺少根键前缀".into())
    }
}

fn delete_startup_value(hive_tag: &str, key_path: &str, value: &str) -> Result<(), String> {
    let (hive, view) = if hive_tag.starts_with("HKCU") {
        (HKEY_CURRENT_USER, KEY_WOW64_64KEY)
    } else if hive_tag.contains("32") {
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY)
    } else {
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY)
    };
    let k = RegKey::predef(hive)
        .open_subkey_with_flags(key_path, KEY_READ | KEY_WRITE | view)
        .map_err(|_| "无法打开启动项键（权限不足）".to_string())?;
    k.delete_value(value).map_err(|e| format!("删除启动项失败：{e}"))
}

fn remove_path_env(hive_tag: &str, seg: &str) -> Result<(), String> {
    let (hive, path) = if hive_tag.starts_with("HKCU") {
        (HKEY_CURRENT_USER, r"Environment")
    } else {
        (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    };
    let k = RegKey::predef(hive)
        .open_subkey_with_flags(path, KEY_READ | KEY_WRITE)
        .map_err(|_| "无法打开环境变量键（权限不足）".to_string())?;
    let cur: String = k.get_value("Path").unwrap_or_default();
    let parts: Vec<&str> = cur.split(';').collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|s| !s.trim().eq_ignore_ascii_case(seg))
        .collect();
    if kept.len() == parts.len() {
        return Ok(());
    }
    k.set_value("Path", &kept.join(";")).map_err(|e| format!("写入 Path 失败：{e}"))
}

fn run_ps(script: String) -> Result<(), String> {
    let ok = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("系统命令执行失败（可能需要管理员权限）".into())
    }
}

#[tauri::command]
pub async fn clean_residuals(
    items: Vec<ResidualItem>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<CleanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let report = clean_impl(&items, Some(&|done, total, freed, cur| {
            let _ = on_progress.send(serde_json::json!({
                "done": done, "total": total, "freed": freed, "current": cur,
            }));
        }));
        Ok(report)
    })
    .await
    .map_err(|e| format!("残留清理任务失败: {e}"))?
}

/// 内部调用入口（强制卸载）
pub fn clean_items(items: &[ResidualItem]) -> CleanReport {
    clean_impl(items, None)
}

/// 供 uninstall 模块直接删注册表（强制卸载时用）
pub fn delete_reg_path(hive_tag: &str, path: &str) -> Result<(), String> {
    reg_delete_payload(&format!("{hive_tag}\\{path}"))
}

fn clean_impl(
    items: &[ResidualItem],
    on_progress: Option<&dyn Fn(u32, u32, u64, &str)>,
) -> CleanReport {
    let mut report = CleanReport::default();
    let roots = allowed_roots_canon();
    let selected: Vec<&ResidualItem> = items.iter().filter(|i| i.selected).collect();
    let total = selected.len() as u32;

    for (idx, it) in selected.iter().enumerate() {
        if let Some(cb) = on_progress {
            cb(idx as u32, total, report.freed, &it.path);
        }
        let (kind, payload) = it.id.split_once('|').unwrap_or((it.kind.as_str(), it.path.as_str()));
        let mut res = CleanItemResult {
            path: it.path.clone(),
            ok: false,
            freed: 0,
            pending_reboot: false,
            reason: None,
        };
        match kind {
            "folder" | "shortcut" | "startupfile" => {
                let p = PathBuf::from(payload);
                let apply = |r: fsx::DelResult, res: &mut CleanItemResult| {
                    res.ok = r.ok;
                    res.freed = r.freed;
                    res.pending_reboot = r.pending_reboot;
                    // 成功但改过权限时也留一句，便于用户理解"为什么这次删掉了"
                    res.reason = if r.ok && r.acl_fixed {
                        Some("已取得所有权后删除".into())
                    } else {
                        r.reason
                    };
                };
                if p.symlink_metadata().is_err() {
                    // 已不存在（可能上一轮已删掉）：算成功，不报"权限不足"误导用户
                    res.ok = true;
                } else if p.is_file() {
                    // 单文件只允许删快捷方式（.lnk/.url）：防止伪造 id 删掉任意文件
                    let is_lnk = p
                        .extension()
                        .map(|e| {
                            let e = e.to_string_lossy().to_lowercase();
                            e == "lnk" || e == "url"
                        })
                        .unwrap_or(false);
                    if !is_lnk {
                        res.reason = Some("只允许删除快捷方式文件".into());
                    } else {
                        let r = fsx::remove_path(&p);
                        apply(r, &mut res);
                    }
                } else if !path_is_safe(&p, &roots) {
                    res.reason = Some("路径不在允许删除的范围内，已拒绝".into());
                } else {
                    let r = fsx::remove_path(&p);
                    apply(r, &mut res);
                }
            }
            "registry" | "class" | "service" | "uninstallkey" => match reg_delete_payload(payload) {
                Ok(()) => res.ok = true,
                Err(e) => res.reason = Some(e),
            },
            "startup" => {
                // payload: HKCU|Software\...\Run|Value
                let parts: Vec<&str> = payload.split('|').collect();
                if parts.len() != 3 {
                    res.reason = Some("启动项格式无效".into());
                } else {
                    match delete_startup_value(parts[0], parts[1], parts[2]) {
                        Ok(()) => res.ok = true,
                        Err(e) => res.reason = Some(e),
                    }
                }
            }
            "pathenv" => {
                let parts: Vec<&str> = payload.splitn(2, '|').collect();
                if parts.len() != 2 {
                    res.reason = Some("PATH 片段格式无效".into());
                } else {
                    match remove_path_env(parts[0], parts[1]) {
                        Ok(()) => res.ok = true,
                        Err(e) => res.reason = Some(e),
                    }
                }
            }
            "task" => {
                let (tpath, tname) = match payload.rsplit_once('\\') {
                    Some((p, n)) => (p, n),
                    None => ("\\", payload),
                };
                let script = format!(
                    "{} Unregister-ScheduledTask -TaskName '{}' -TaskPath '{}' -Confirm:$false -ErrorAction Stop",
                    crate::commands::PS_UTF8_PRELUDE,
                    tname.replace('\'', "''"),
                    tpath.replace('\'', "''")
                );
                match run_ps(script) {
                    Ok(()) => res.ok = true,
                    Err(e) => res.reason = Some(e),
                }
            }
            "firewall" => {
                // payload: 规则名（唯一）| 显示名（兜底）
                let (rname, label) = payload.split_once('|').unwrap_or((payload, payload));
                let esc = |s: &str| s.replace('\'', "''");
                let script = format!(
                    "{} $ok=$false; try {{ Remove-NetFirewallRule -Name '{}' -ErrorAction Stop; $ok=$true }} catch {{}}; \
                     if (-not $ok) {{ Remove-NetFirewallRule -DisplayName '{}' -ErrorAction Stop }}",
                    crate::commands::PS_UTF8_PRELUDE,
                    esc(rname),
                    esc(label)
                );
                match run_ps(script) {
                    Ok(()) => res.ok = true,
                    Err(e) => res.reason = Some(e),
                }
            }
            _ => res.reason = Some("未知的残留类型".into()),
        }

        if res.ok {
            report.ok += 1;
        } else {
            report.failed += 1;
            if res.pending_reboot {
                report.pending_reboot += 1;
            }
        }
        report.freed += res.freed;
        report.items.push(res);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 归一化去掉括号与版本噪音() {
        assert_eq!(norm("Photoshop 2024（64 位）"), "photoshop2024");
        assert_eq!(norm("WeChat [3.9.0]"), "wechat");
        assert_eq!(norm("Visual Studio Code"), "visualstudiocode");
    }

    #[test]
    fn 精确与前缀比包含得分高() {
        let h = norm("cloudmusic");
        assert_eq!(quality(&h, "cloudmusic"), 100);
        assert_eq!(quality(&h, "cloud"), 85);
        assert_eq!(quality(&h, "music"), 85);
        assert!(quality(&h, "oudmus") < 90);
    }

    #[test]
    fn 短词包含只给低分避免误报() {
        // "qq" 这类词几乎能命中一切，只给 55（低于默认勾选阈值 60）
        let h = norm("qqprotect");
        assert_eq!(quality(&h, "qq"), 55);
    }

    #[test]
    fn 分词全含可命中厂商加产品写法() {
        let h = norm("visualstudiocode");
        assert_eq!(quality(&h, "visual studio"), 62);
    }

    #[test]
    fn 相似度能兜住拼写差异() {
        let h = norm("hbuilderx");
        assert!(quality(&h, "hbuilder") >= 40, "应能识别近似的产品名");
    }

    #[test]
    fn 词表会滤掉无意义的短词() {
        let mut v = Vec::new();
        push_needle(&mut v, "QQ", 1.0); // 纯 ASCII 两字符
        push_needle(&mut v, "微信", 1.0); // 两汉字，允许
        assert!(v.iter().any(|n| n.text == "微信"));
        assert!(!v.iter().any(|n| n.text == "qq"));
    }

    #[test]
    fn 共享容器目录不会被当成残留() {
        for c in ["Common Files", "WindowsApps", "Packages", "Microsoft", "Temp", "VirtualStore"] {
            assert!(is_container(c), "{c} 应被识别为共享容器");
        }
        assert!(!is_container("Adobe"));
        assert!(!is_container("CloudMusic"));
    }

    #[test]
    fn 危险路径被安全闸拒绝() {
        let roots = allowed_roots_canon();
        assert!(!path_is_safe(Path::new(r"C:\Windows"), &roots));
        assert!(!path_is_safe(Path::new(r"C:\"), &roots));
        assert!(!path_is_safe(Path::new("relative\\path"), &roots));
    }

    /// 实机诊断：`DM_RESIDUAL_NAME="某软件名" cargo test --lib 诊断残留扫描 -- --ignored --nocapture`
    /// 只打印结果，不做任何删除，用于核对"到底能不能扫出来"以及各阶段耗时。
    #[test]
    #[ignore]
    fn 诊断残留扫描() {
        let name = std::env::var("DM_RESIDUAL_NAME").unwrap_or_else(|_| "网易云音乐".to_string());
        let t0 = std::time::Instant::now();
        let needles = build_needles(&name, None);
        let t1 = t0.elapsed().as_millis();
        let mut items: Vec<ResidualItem> = Vec::new();
        scan_dirs(&needles, &mut items);
        let t2 = t0.elapsed().as_millis();
        scan_registry(&needles, &mut items);
        let t3 = t0.elapsed().as_millis();
        scan_shortcuts(&needles, &mut items);
        let t4 = t0.elapsed().as_millis();
        let mut tasks: Vec<(String, u8)> = Vec::new();
        let mut fws: Vec<(String, String, u8)> = Vec::new();
        scan_tasks(&needles, &mut tasks);
        scan_firewall(&needles, &mut fws);
        let t5 = t0.elapsed().as_millis();
        println!(
            "\n=== 诊断：{name} · 词表{:?} · 建词表{}ms ===\n目录 {}ms(共{}ms) · 注册表 {}ms(共{}ms) · 快捷方式 {}ms(共{}ms) · 系统项 {}ms(共{}ms)",
            needles.iter().map(|n| n.text.clone()).collect::<Vec<_>>(),
            t1, t2, t2, t3 - t2, t3, t4 - t3, t4, t5 - t4, t5
        );
        println!("目录/注册表/快捷方式命中 {} 项，计划任务 {} 个，防火墙 {} 条", items.len(), tasks.len(), fws.len());
        for i in &items {
            println!("[{:>3}%] {:<12} {:>10}  {}", i.confidence.unwrap_or(0), i.kind, i.size, i.path);
        }
        for (t, c) in &tasks {
            println!("[{:>3}%] task         {:>10}  {t}", c, 0);
        }
        for (rname, label, c) in &fws {
            println!("[{:>3}%] firewall     {:>10}  {label}（规则名 {rname}）", c, 0);
        }    }
}
