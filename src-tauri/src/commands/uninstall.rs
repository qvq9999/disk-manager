//! 软件卸载中心：Win32 注册表卸载键 + 微软商店 AppX + 残留扫描 + 捆绑识别

use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use walkdir::WalkDir;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
use winreg::{HKEY, RegKey};

use base64::Engine as _;
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub install_date: String,
    pub size_bytes: u64,
    #[serde(rename = "type")]
    pub kind: String, // win32 | store
    pub bundle: bool,
    pub system: bool,
    /// 商店应用内联 Logo（data URL），win32 走 get_app_icon 懒加载
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_data: Option<String>,
}

#[derive(Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResidualItem {
    pub id: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String, // registry | folder | shortcut | startup | service
    pub size: u64,
    pub selected: bool,
    /// 残留内容明细（如「12 个文件 · 34.5 MB」「含 3 个子键」「指向 …」）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 目录快速统计（已不再用于列表体积，保留供扩展）
#[allow(dead_code)]
fn dir_size_fast(p: &PathBuf, max_depth: usize) -> u64 {
    WalkDir::new(p)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// 目录快速统计：(体积, 文件数)
fn dir_stats(p: &PathBuf, max_depth: usize) -> (u64, u64) {
    let mut size = 0u64;
    let mut count = 0u64;
    for e in WalkDir::new(p).max_depth(max_depth).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if let Ok(m) = e.metadata() {
            if m.is_file() {
                size += m.len();
                count += 1;
            }
        }
    }
    (size, count)
}

/// 读取注册表字符串：优先标准 UTF-16；若含替换字符（软件写入时编码错误，
/// 如"游戏加加"这类注册表名显示为乱码），则用原始字节按 GB18030 重新解码。
/// 清理注册表字符串里的「脏数据」：
/// ① 有些软件把一个 NUL 分隔的多串块整个塞进 REG_SZ（如 游戏加加 的 DisplayName 是
///    262 字节："游戏加加\0\0\0\0GamePlusPlusInterUninstallMutex\0GamePP.exe\0..."），
///    Python 在第一个 NUL 截断所以正常，但 winreg 会全量返回 → UI 显示成乱码。
///    这里统一截断到第一个 NUL。
/// ② 再去掉其它控制字符（保留制表符），并压缩连续空白。
fn sanitize_str(s: &str) -> String {
    let first = s.split('\0').next().unwrap_or("");
    let picked = if first.trim().is_empty() {
        // 首段为空时取第一个非空段（防止 \0 开头的脏值）
        s.split('\0').map(|p| p.trim()).find(|p| !p.is_empty()).unwrap_or(first)
    } else {
        first
    };
    picked
        .chars()
        .filter(|c| (*c as u32) >= 0x20 || *c == '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

fn reg_str(sub: &RegKey, value: &str) -> String {
    let direct = sub.get_value::<String, _>(value).unwrap_or_default();
    // 含替换符 → 说明原始字节不是合法 UTF-16，尝试按 GB18030 重新解码
    if direct.contains('\u{FFFD}') {
        if let Ok(raw) = sub.get_raw_value(value) {
            let (cow, _, _) = encoding_rs::GB18030.decode(&raw.bytes);
            let s = sanitize_str(cow.as_ref());
            if !s.is_empty() && !s.contains('\u{FFFD}') {
                return s;
            }
        }
    }
    sanitize_str(&direct)
}

/// 系统/公共厂商键名白名单：避免把 Microsoft、驱动厂商等误报为软件残留
fn is_whitelisted(name: &str) -> bool {
    const WL: &[&str] = &[
        "microsoft", "windows", "intel", "nvidia", "amd", "realtek", "mediatek", "qualcomm",
        "google", "mozilla", "apple", "oracle", "java", "vmware", "citrix", "adobe systems",
        "policies", "classes", "clients", "wow6432node", "windows nt", "system", "drivers",
        "odbc", "cryptography", "internet explorer", "microsoft corporation",
    ];
    let l = name.to_lowercase();
    WL.iter().any(|w| l == *w || l.starts_with(w))
}

fn fmt_date(ymd: &str) -> String {
    if ymd.len() == 8 && ymd.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &ymd[..4], &ymd[4..6], &ymd[6..])
    } else {
        ymd.to_string()
    }
}

/// 捆绑启发式：名字带助手/服务特征（结合同日安装判定）
fn is_bundle_name(name: &str) -> bool {
    let n = name.to_lowercase();
    ["助手", "helper", "assist", "toolbar", "推送", "热点", "update service", "升级"]
        .iter()
        .any(|k| n.contains(k))
}

fn is_system_store(name: &str) -> bool {
    ["Microsoft.Xbox", "Microsoft.WindowsStore", "Microsoft.WindowsCalculator", "Microsoft.WindowsPhotos"]
        .iter().any(|k| name.starts_with(k))
}

/// 以管理员身份重启本程序（ShellExecuteW runas）
#[tauri::command]
pub fn restart_as_admin() -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let wide_path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let rc = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            wide_path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    // ShellExecuteW 返回值 > 32 表示成功；用户取消 UAC 时返回 5（SE_ERR_ACCESSDENIED）
    if rc as isize > 32 {
        Ok("已请求管理员权限".into())
    } else {
        Err("已取消或不支持提权".into())
    }
}
/// Geek 式**强制卸载**：用于自带卸载器损坏的软件（例如报"卸载入口会话参数无效"）。
/// 做法：删除注册表记录的安装目录 → 删除卸载注册表键 → 清理由名称/线索匹配到的所有残留。
#[tauri::command]
pub async fn force_remove_app(id: String) -> Result<UninstallResult, String> {
    tauri::async_runtime::spawn_blocking(move || force_remove_app_impl(id))
        .await
        .map_err(|e| format!("强制卸载任务失败: {e}"))?
}

fn force_remove_app_impl(id: String) -> Result<UninstallResult, String> {
    let key_name = id.strip_prefix("win32:").ok_or("仅支持 Win32 软件的强制卸载")?;
    let _guard = APPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut freed = 0u64;
    let mut failed = 0u32;
    let mut name = String::new();
    let mut removed_dirs = 0u32;

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ | KEY_WRITE) else { continue };
            let Ok(sub) = root.open_subkey(key_name) else { continue };

            name = reg_str(&sub, "DisplayName");
            let loc: String = reg_str(&sub, "InstallLocation");
            let icon: String = reg_str(&sub, "DisplayIcon");
            let un: String = reg_str(&sub, "UninstallString");

            // ① 删除安装目录（InstallLocation / DisplayIcon / UninstallString 所在目录）
            for cand in [loc.as_str(), icon.as_str(), un.as_str()].into_iter().filter(|s| !s.is_empty()) {
                if let Some(dir) = derive_dir(cand) {
                    if dir.is_dir() && dir.to_string_lossy().len() > 6 {
                        let (sz, _) = dir_stats(&dir, 6);
                        match std::fs::remove_dir_all(&dir) {
                            Ok(()) => {
                                freed += sz;
                                removed_dirs += 1;
                            }
                            Err(_) => failed += 1,
                        }
                    }
                }
            }

            // ② 删除卸载注册表键
            let target = format!("{}\\{}", full_path, key_name);
            let _ = reg_delete_tree(hive, &target);
        }
    }

    // ③ 清理其余残留（快捷方式 / 启动项 / 同名目录 / Software 键）
    if !name.is_empty() {
        let items = scan_app_residuals_impl(name.clone());
        match clean_residuals_impl(items) {
            Ok((f2, fail2)) => {
                freed += f2;
                failed += fail2;
            }
            Err(_) => failed += 1,
        }
    }

    let still = is_still_installed(&name);
    Ok(UninstallResult {
        ok: !still,
        message: format!(
            "强制卸载完成：删除 {removed_dirs} 个程序目录并清理残留，释放 {}；{failed} 项被占用或权限不足跳过{}",
            fmt_bytes(freed),
            if still { "；注册表条目仍存在，可再次强制卸载" } else { "" }
        ),
        still_installed: still,
        pending: false,
    })
}

/// 从注册表路径/命令行推导目录
fn derive_dir(raw: &str) -> Option<PathBuf> {
    let cleaned = raw.trim().trim_matches('"');
    let first = cleaned.split(',').next().unwrap_or("").trim().trim_matches('"');
    let first = first.split_whitespace().next().unwrap_or(first);
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

/// 字节数可读格式（用于结果说明）
fn fmt_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n == 0 {
        return "0 B".to_string();
    }
    let i = ((n as f64).log2() / 10.0).floor().min(4.0) as usize;
    format!("{:.2} {}", n as f64 / 1024f64.powi(i as i32), U[i])
}

fn appx_logo_data_url(install_location: &str) -> Option<String> {
    if install_location.is_empty() {
        return None;
    }
    let manifest = std::fs::read_to_string(std::path::Path::new(install_location).join("AppxManifest.xml")).ok()?;
    let idx = manifest.find("Logo=\"")?;
    let rest = &manifest[idx + 6..];
    let end = rest.find('"')?;
    let rel = rest[..end].replace('/', "\\");
    if rel.is_empty() || !rel.to_lowercase().ends_with(".png") {
        return None;
    }
    let full = std::path::Path::new(install_location).join(&rel);
    let stem = full.with_extension("");
    let ext = full.extension()?.to_string_lossy().to_string();
    let stem_name = stem.file_name()?.to_string_lossy().to_string();
    let dir = full.parent()?;
    let mut candidates: Vec<PathBuf> = ["200", "125", "150", "100", "400"]
        .iter()
        .map(|s| dir.join(format!("{stem_name}.scale-{s}.{ext}")))
        .collect();
    candidates.push(full.clone());
    for c in candidates {
        if let Ok(bytes) = std::fs::read(&c) {
            return Some(format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ));
        }
    }
    None
}

fn list_store_apps() -> Vec<InstalledApp> {
    let mut apps = Vec::new();
    let script = format!(
        "{} Get-AppxPackage | Where-Object {{ -not $_.IsFramework }} | Select Name,PackageFullName,Publisher,Version,InstallLocation | ConvertTo-Json -Compress -Depth 2",
        crate::commands::PS_UTF8_PRELUDE
    );
    let out = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output();
    let Ok(o) = out else { return apps };
    let s = crate::commands::decode_out(&o.stdout);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { return apps };
    let arr: Vec<serde_json::Value> = match v {
        serde_json::Value::Array(a) => a,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => vec![],
    };
    for p in arr {
        let name = p.get("Name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let full = p.get("PackageFullName").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let loc = p.get("InstallLocation").and_then(|x| x.as_str()).unwrap_or("");
        let pb = PathBuf::from(loc);
        // 体积统计改为跳过（WindowsApps 全量遍历是卸载页加载慢的主因），显示为「—」
        let size = 0u64;
        let _ = &pb;
        let icon = appx_logo_data_url(loc);
        apps.push(InstalledApp {
            id: format!("store:{full}"),
            name,
            version: p.get("Version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            publisher: p.get("Publisher").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            install_date: String::new(),
            size_bytes: size,
            kind: "store".into(),
            bundle: false,
            system: is_system_store(&p.get("Name").and_then(|x| x.as_str()).unwrap_or("")),
            icon_data: icon,
        });
    }
    apps
}

const WIN32_UNINSTALL_PATHS: &[&str] = &[
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
];

// 全局串行锁：软件枚举/残留扫描都涉及大量注册表+文件 I/O，串行执行避免卸载页卡顿
static APPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Win32 软件列表（注册表枚举，快速返回；商店应用单独异步加载，提升首屏速度）
#[tauri::command]
pub async fn list_win32_apps() -> Vec<InstalledApp> {
    tauri::async_runtime::spawn_blocking(|| {
        let _guard = APPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(win32_apps_impl)).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// 微软商店应用列表（Get-AppxPackage，较慢；不统计包体积）
#[tauri::command]
pub async fn list_store_apps_only() -> Vec<InstalledApp> {
    tauri::async_runtime::spawn_blocking(|| {
        let _guard = APPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(list_store_apps)).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// 合并列表（兼容旧调用）
#[tauri::command]
pub async fn list_apps() -> Vec<InstalledApp> {
    tauri::async_runtime::spawn_blocking(|| {
        let _guard = APPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut apps = win32_apps_impl();
            apps.extend(list_store_apps());
            mark_bundles(&mut apps);
            apps.sort_by(|a, b| sort_key(&a.name).cmp(&sort_key(&b.name)));
            apps
        }))
        .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// 捆绑识别：同日安装 + 特征名
fn mark_bundles(apps: &mut [InstalledApp]) {
    let mut by_date: HashMap<String, usize> = HashMap::new();
    for a in apps.iter() {
        if !a.install_date.is_empty() {
            *by_date.entry(a.install_date.clone()).or_default() += 1;
        }
    }
    for a in apps.iter_mut() {
        if is_bundle_name(&a.name) && by_date.get(&a.install_date).copied().unwrap_or(0) > 1 {
            a.bundle = true;
        }
    }
}

fn win32_apps_impl() -> Vec<InstalledApp> {
    let mut apps: Vec<InstalledApp> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut hives: Vec<HKEY> = vec![HKEY_LOCAL_MACHINE];
    hives.push(HKEY_CURRENT_USER);
    for hive in hives {
        for path in WIN32_UNINSTALL_PATHS {
            // HKCU 的 Uninstall 键位于 Software\...；HKLM 位于 SOFTWARE\...
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) else { continue };
            for key in root.enum_keys().flatten() {
                if !seen_ids.insert(key.clone()) {
                    continue;
                }
                let Ok(sub) = root.open_subkey(&key) else { continue };
                let name: String = reg_str(&sub, "DisplayName");
                if name.is_empty() {
                    continue;
                }
                let sys_comp: u32 = sub.get_value("SystemComponent").unwrap_or(0);
                if sys_comp == 1 {
                    continue;
                }
                let est_kb: u32 = sub.get_value("EstimatedSize").unwrap_or(0);
                let date_raw: String = reg_str(&sub, "InstallDate");
                apps.push(InstalledApp {
                    id: format!("win32:{key}"),
                    name,
                    version: reg_str(&sub, "DisplayVersion"),
                    publisher: reg_str(&sub, "Publisher"),
                    install_date: fmt_date(&date_raw),
                    size_bytes: est_kb as u64 * 1024,
                    kind: "win32".into(),
                    bundle: false,
                    system: false,
                    icon_data: None,
                });
            }
        }
    }
    mark_bundles(&mut apps);
    // 按名称首字母排序（中文转拼音，便于按 A-Z 浏览）
    apps.sort_by(|a, b| sort_key(&a.name).cmp(&sort_key(&b.name)));
    apps
}

/// 排序键：ASCII 字母数字保留（小写），中文转拼音，其余字符（标点/空格）跳过
fn sort_key(name: &str) -> String {
    use pinyin::ToPinyin;
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if c.is_ascii() {
            // 空格/标点忽略
        } else if let Some(p) = c.to_pinyin() {
            s.push_str(p.plain());
        } else {
            s.push(c);
        }
    }
    s
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub ok: bool,
    pub message: String,
    /// 卸载后该软件是否仍存在于已安装列表（用于提示"卸载未完成"）
    pub still_installed: bool,
    /// 卸载程序仍在运行 / 需要用户在它的窗口里操作。
    /// 此时前端应改为轮询 `check_installed`，而不是继续等这个调用返回。
    #[serde(default)]
    pub pending: bool,
}

/// 取消「等待卸载完成」。
///
/// ⚠️ 语义是「不再等」，不是「杀掉卸载」。交互式卸载器（NSIS 向导、InstallShield）
/// 必须让用户把窗口里的流程走完，强行终止会留下装到一半的软件。
/// 只有静默模式（无窗口）超时/取消时才终止整个进程树。
static UNINSTALL_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub fn cancel_uninstall() {
    UNINSTALL_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 启动卸载（Win32 走原卸载器；Store 走 Remove-AppxPackage）——后台线程执行防卡界面
#[tauri::command]
pub async fn uninstall_app(id: String, silent_first: Option<bool>) -> Result<UninstallResult, String> {
    // 默认「交互式优先」：用户点卸载就是想看到那个卸载向导，
    // 静默只作为交互式不可用时的兜底；批量卸载则由前端显式要求静默优先。
    let silent_first = silent_first.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || uninstall_app_impl(id, silent_first))
        .await
        .map_err(|e| format!("卸载任务失败: {e}"))?
}

/// 卸载器所属的安装框架。
///
/// 借鉴 Bulk Crap Uninstaller（Apache-2.0）的做法：**不是凭文件名猜，而是读 exe 自身的特征**
/// 来判定框架 —— 这直接决定静默参数该传什么。传错的后果很实际：
/// NSIS 收到 Inno 的 `/VERYSILENT` 会当成未知参数，多数情况下弹窗或静默失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UninstallerKind {
    Msiexec,
    Inno,
    Nsis,
    Wise,
    InstallShield,
    Other,
}

fn detect_kind(exe: &str) -> UninstallerKind {
    let p = std::path::Path::new(exe);
    let Some(name) = p.file_name() else { return UninstallerKind::Other };
    let low = name.to_string_lossy().to_lowercase();

    if low == "msiexec.exe" {
        return UninstallerKind::Msiexec;
    }
    if low.contains("unwise") {
        return UninstallerKind::Wise;
    }
    // Inno Setup：文件名形如 unins000.exe，且**同目录必须有同名 .dat**（unins000.dat）
    // —— 这是 Inno 的固有结构，比单看文件名可靠得多
    if is_inno_filename(&low) {
        let dat = p.with_extension("dat");
        if dat.is_file() {
            return UninstallerKind::Inno;
        }
    }
    if low.contains("uninstall") && low.contains("installshield") {
        return UninstallerKind::InstallShield;
    }
    // NSIS：扫描 exe 头部找 "Nullsoft" 签名（BCU 也是这么判的）
    if has_nsis_signature(p) {
        return UninstallerKind::Nsis;
    }
    UninstallerKind::Other
}

fn is_inno_filename(low: &str) -> bool {
    // unins000 / unins001 …（Inno 的默认命名，三位数字后缀）
    let Some(idx) = low.find("unins") else { return false };
    let end = low.len().saturating_sub(4); // 去掉 ".exe"
    if idx + 5 >= end {
        return false;
    }
    let tail = &low[idx + 5..end];
    !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())
}

/// 读取 exe 开头找 "Nullsoft"：NSIS 的安装/卸载程序都会写入这个标记
fn has_nsis_signature(p: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    // 标记通常在 PE 头与资源之间，读前 2MB 足够；避免为超大文件做无谓 I/O
    let mut buf = vec![0u8; 2 * 1024 * 1024];
    let Ok(n) = f.read(&mut buf) else { return false };
    buf[..n].windows(8).any(|w| w.eq_ignore_ascii_case(b"Nullsoft"))
}

/// 展开命令行里的 %VAR%（cmd 会自动展开，但直接启动 exe 时不会）
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '%' {
            if let Some(end) = bytes[i + 1..].iter().position(|&c| c == '%') {
                let name: String = bytes[i + 1..i + 1 + end].iter().collect();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '(' || c == ')') {
                    if let Ok(v) = std::env::var(&name) {
                        out.push_str(&v);
                        i += end + 2;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 静默参数：按**实测判定出的框架**选择，而不是按文件名猜。
///
/// 借鉴 Bulk Crap Uninstaller：它先判定 UninstallerType，再据此给参数。
/// 我们仍保留一个备选序列 —— 判定不可能 100% 准确（比如被改名过的卸载器），
/// 但把「最可能正确」的那个放第一位，能显著减少无谓的重试。
fn silent_variants_for(exe: &str) -> Vec<&'static str> {
    let name = exe.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    if name.is_empty() {
        return Vec::new();
    }
    // 有些框架没有可靠的二进制特征，靠文件名兜底
    if name.contains("burn") || name.contains("bundle") {
        return vec!["/quiet /norestart"];
    }
    if name.contains("squirrel") || name == "update.exe" {
        return vec!["--uninstall -s"];
    }
    if name.contains("advancedinstaller") {
        return vec!["/exenoui /qn"];
    }
    if name.starts_with("setup") && !name.contains("uninst") {
        // InstallShield / 通用安装器
        return vec!["/s /x", "-s", "/silent"];
    }

    match detect_kind(exe) {
        UninstallerKind::Msiexec => vec!["/qn /norestart"],
        UninstallerKind::Inno => vec![
            "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
            "/SILENT /SUPPRESSMSGBOXES /NORESTART",
        ],
        UninstallerKind::Nsis => vec!["/S"],
        UninstallerKind::Wise => vec!["/S", "-uninstall"],
        UninstallerKind::InstallShield => vec!["/s /x", "/silent"],
        // 判定不出来：按 NSIS → Inno 的顺序试（统计上 NSIS 更常见）
        UninstallerKind::Other => {
            if name.contains("uninst") || name.contains("卸载") || name.contains("反安装") {
                vec!["/S", "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"]
            } else {
                Vec::new()
            }
        }
    }
}
///
/// 修正 MSI 卸载命令。
///
/// ⚠️ 一个经典坑：大量软件的 `UninstallString` 写的是 `MsiExec.exe /I{GUID}`
/// （甚至 `/I` 后面直接跟 GUID 无空格）。`/I` 的含义是「安装 / 修改」，
/// 直接执行会打开「修改安装」对话框而不是卸载 —— 这正是「点了卸载却弹出修复界面」的成因。
/// 正确做法是换成 `/X{GUID}`。
fn fix_msiexec(cmd: &str) -> String {
    let low = cmd.to_ascii_lowercase();
    if !low.contains("msiexec") {
        return cmd.to_string();
    }
    let mut out: Vec<String> = Vec::new();
    for tok in cmd.split_whitespace() {
        let bare = tok.trim_matches('"');
        let tlow = bare.to_ascii_lowercase();
        // /I{GUID} 或 /i{GUID} → /X{GUID}
        if tlow.starts_with("/i") && bare.len() > 2 && !tlow.starts_with("/i:") {
            out.push(format!("/X{}", &bare[2..]));
        } else {
            out.push(tok.to_string());
        }
    }
    out.join(" ")
}

/// 卸载完成后杀掉仍在运行、且可执行文件位于安装目录内的进程
/// （Geek/HiBit 必做项：进程不杀，安装目录就删不掉，残留扫描会一直报「文件被占用」）
/// 返回被杀掉的进程数
fn kill_processes_under(install_dir: &str) -> u32 {
    let dir = install_dir.trim().trim_matches('"');
    let dir = dir.split(',').next().unwrap_or("").trim();
    if dir.is_empty() {
        return 0;
    }
    let dir = dir.trim_end_matches('\\');
    let script = format!(
        "{} Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -and $_.ExecutablePath -like '{}\\*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; $_.Name }}",
        crate::commands::PS_UTF8_PRELUDE,
        dir.replace('\\', "\\\\").replace('\'', "''")
    );
    let Ok(out) = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    else {
        return 0;
    };
    if !out.status.success() {
        return 0;
    }
    let text = crate::commands::decode_out(&out.stdout);
    text.lines().filter(|l| !l.trim().is_empty()).count() as u32
}

fn uninstall_app_impl(id: String, silent_first: bool) -> Result<UninstallResult, String> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    // 每次卸载开始都清一次取消标志（串行锁保证同一时刻只有一个卸载在跑）
    UNINSTALL_CANCEL.store(false, Ordering::SeqCst);

    if let Some(full) = id.strip_prefix("store:") {
        let script = format!("{} Remove-AppxPackage -Package '{}'", crate::commands::PS_UTF8_PRELUDE, full);
        let out = crate::commands::silent_command("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .map_err(|e| e.to_string())?;
        return if out.status.success() {
            Ok(UninstallResult { ok: true, message: "商店应用已卸载".into(), still_installed: false, pending: false })
        } else {
            let msg = crate::commands::decode_out(&out.stderr);
            Ok(UninstallResult { ok: false, message: format!("卸载失败：{}", msg.trim()), still_installed: true, pending: false })
        };
    }
    let key_name = id.strip_prefix("win32:").ok_or("无效 id")?;
    let mut errs: Vec<String> = Vec::new();
    let budget = Instant::now() + Duration::from_secs(UNINSTALL_BUDGET_SECS);

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) else { continue };
            let Ok(sub) = root.open_subkey(key_name) else { continue };

            let quiet: String = reg_str(&sub, "QuietUninstallString");
            let normal: String = reg_str(&sub, "UninstallString");
            let icon: String = reg_str(&sub, "DisplayIcon");
            let loc: String = reg_str(&sub, "InstallLocation");
            let name: String = reg_str(&sub, "DisplayName");

            // 候选命令（按可靠性排序）：自带卸载器 → 静默串 → 常规串
            // MSI 的 UninstallString 常写成 `/I{GUID}`（修改安装）而非 `/X{GUID}`，先统一修正
            let mut order: Vec<String> = Vec::new();

            // 目录线索有三个来源，缺一不可：
            //   · InstallLocation —— 最直接，但很多软件（实测 Android Studio 等）干脆不写
            //   · DisplayIcon     —— 常指向主程序 exe，可反推目录
            //   · UninstallString —— 里面往往就是卸载器的完整路径，是最可靠的兜底线索
            let (normal_exe, _) = split_cmd(&normal);
            let (quiet_exe, _) = split_cmd(&quiet);
            for cand in [
                loc.as_str(),
                icon.as_str(),
                normal_exe.as_str(),
                quiet_exe.as_str(),
            ]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            {
                if let Some(exe) = find_own_uninstaller(cand) {
                    let s = exe.to_string_lossy().to_string();
                    if !order.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
                        order.push(s);
                    }
                }
            }
            for s in [quiet, normal].into_iter().filter(|s| !s.trim().is_empty()) {
                let fixed = fix_msiexec(&s);
                if !order.iter().any(|x| x.eq_ignore_ascii_case(&fixed)) {
                    order.push(fixed);
                }
            }
            if order.is_empty() {
                errs.push("该软件注册表中没有卸载命令".into());
                continue;
            }

            // 逐个尝试：某个候选启动后软件仍未消失（例如卸载器报"参数无效"自行退出），
            // 自动换下一个候选，直到成功或候选用尽
            let mut tried: Vec<String> = Vec::new();
            let mut done: Option<String> = None;
            // Geek/HiBit 做法：每个候选先试「静默参数」，不生效再退回交互式原命令
            'outer: for cmd_line in order {
                // 总预算用尽：不再换候选，否则「每个候选各等一次」会累积成几十分钟
                if Instant::now() > budget {
                    tried.push(format!("总耗时超过 {UNINSTALL_BUDGET_SECS} 秒，停止尝试其余方式"));
                    break 'outer;
                }
                if UNINSTALL_CANCEL.load(Ordering::SeqCst) {
                    return Ok(UninstallResult {
                        ok: false,
                        message: "已停止等待卸载完成".into(),
                        still_installed: is_still_installed(&name),
                        pending: true,
                    });
                }
                let mut variants: Vec<(String, bool)> = Vec::new();
                // 先解析出 exe 路径，静默参数按 exe 名判定（不能对整条命令行取末段）
                let (exe_path, args) = split_cmd(&cmd_line);
                if !exe_path.is_empty() {
                    let mut silent_vs: Vec<(String, bool)> = Vec::new();
                    for sa in silent_variants_for(&exe_path) {
                        let joined = if args.is_empty() {
                            format!("\"{}\" {sa}", exe_path)
                        } else {
                            format!("\"{}\" {} {sa}", exe_path, args)
                        };
                        if !silent_vs.iter().any(|(v, _)| v == &joined) {
                            silent_vs.push((joined, true));
                        }
                    }
                    if silent_first {
                        // 批量卸载：静默优先，交互式兜底（用户不在场，弹窗会卡住队列）
                        variants.extend(silent_vs);
                        variants.push((cmd_line.clone(), false));
                    } else {
                        // 单条卸载：**只走交互式**，绝不自动降级到静默。
                        // 用户在卸载向导里点了「取消」时，卸载器往往以非 0 退出码结束，
                        // 我们无法把它和「失败」区分开 —— 若此时自动改用静默参数重试，
                        // 就等于绕开用户的取消意愿把软件卸掉。卸载是破坏性操作，不能这样「重试」。
                        variants.push((cmd_line.clone(), false));
                    }
                } else {
                    variants.push((cmd_line.clone(), false));
                }
                for (vline, silent) in variants {
                    match launch_uninstaller(&vline, silent) {
                        LaunchOutcome::Exited(_code, msg) => {
                            // 等待软件真正从系统移除（卸载器退出后文件可能还在删除中）
                            if wait_until_removed(&name, &loc) {
                                let mode = if silent { "，静默模式" } else { "" };
                                done = Some(format!("卸载完成（{msg}{mode}）"));
                                break 'outer;
                            }
                            tried.push(format!("{} → {}", short_cmd(&vline), msg));
                        }
                        LaunchOutcome::StillRunning(msg) => {
                            // 卸载器（多半是交互向导）还在运行：立刻交还控制权。
                            // ⚠️ 不能杀进程 —— 用户可能正在向导里点下一步；
                            //    也不能继续试下一个候选 —— 那会同时开两个卸载器。
                            return Ok(UninstallResult {
                                ok: false,
                                message: msg,
                                still_installed: true,
                                pending: true,
                            });
                        }
                        LaunchOutcome::TimedOut(msg) => {
                            tried.push(format!("{} → {}", short_cmd(&vline), msg));
                        }
                        LaunchOutcome::Cancelled => {
                            return Ok(UninstallResult {
                                ok: false,
                                message: "已停止等待卸载完成".into(),
                                still_installed: is_still_installed(&name),
                                pending: true,
                            });
                        }
                        LaunchOutcome::Failed(e) => {
                            tried.push(format!("{} → 启动失败：{e}", short_cmd(&vline)));
                        }
                    }
                }
            }
            // 卸载收尾：结束仍驻留在安装目录内的进程，否则目录删不掉、残留扫描会一直报「被占用」
            // （仅在「确认已卸载」或「尝试已结束」时执行，不能碰正在卸载的进程）
            if done.is_some() {
                let killed = kill_processes_under(&loc);
                let extra = if killed > 0 { format!("，已结束 {killed} 个残留进程") } else { String::new() };
                return Ok(UninstallResult {
                    ok: true,
                    message: format!("{}{extra}", done.unwrap()),
                    still_installed: false,
                    pending: false,
                });
            }
            if Instant::now() > budget {
                return Ok(UninstallResult {
                    ok: false,
                    message: format!("卸载未在 {UNINSTALL_BUDGET_SECS} 秒内完成；已尝试：{}", tried.join("；")),
                    still_installed: is_still_installed(&name),
                    pending: true,
                });
            }
            return Ok(UninstallResult {
                ok: false,
                message: format!(
                    "已依次尝试 {} 种卸载方式，但该软件仍存在于系统中：{}",
                    tried.len(),
                    tried.join("；")
                ),
                still_installed: is_still_installed(&name),
                pending: false,
            });
        }
    }
    Ok(UninstallResult {
        ok: false,
        message: if errs.is_empty() {
            "未找到该软件的卸载程序（可能已被手动删除，可直接扫描残留）".into()
        } else {
            format!("卸载失败：{}", errs.join("；"))
        },
        still_installed: true,
        pending: false,
    })
}

/// 按 id 精确判断软件是否还装着（比按 DisplayName 全盘比对快得多）。
///
/// 前端在「卸载器仍在运行」时会轮询它，用来判断用户到底卸完没有。
#[tauri::command]
pub async fn check_installed(id: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || check_installed_impl(&id))
        .await
        .unwrap_or(true)
}

fn check_installed_impl(id: &str) -> bool {
    if let Some(full) = id.strip_prefix("store:") {
        // 商店应用：查 AppModel 仓库下的包目录（比 Get-AppxPackage 快一个数量级）
        let base = format!(
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages\{}",
            full
        );
        return RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(&base, KEY_READ)
            .is_ok();
    }
    let Some(key_name) = id.strip_prefix("win32:") else { return false };
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            if let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) {
                if root.open_subkey(key_name).is_ok() {
                    return true;
                }
            }
        }
    }
    false
}
/// 命令行的简短显示（用于结果说明）
fn short_cmd(cmd: &str) -> String {
    let c = cmd.trim();
    if c.chars().count() > 60 {
        format!("{}…", c.chars().take(60).collect::<String>())
    } else {
        c.to_string()
    }
}

/// 轮询等待软件真正从系统移除：注册表条目消失，或原安装目录已不存在
/// （卸载器有时先退出、文件随后才删完；也有卸载器保留注册表残留 → 目录消失即视为完成）
fn wait_until_removed(name: &str, install_loc: &str) -> bool {
    let started = std::time::Instant::now();
    let loc_path = {
        let raw = install_loc.trim().trim_matches('"');
        let first = raw.split(',').next().unwrap_or("").trim();
        if first.is_empty() { None } else { Some(PathBuf::from(first)) }
    };
    loop {
        let still = is_still_installed(name);
        let dir_gone = match &loc_path {
            Some(p) => !p.is_dir(),
            None => true, // 注册表没给安装目录时无法用目录判断
        };
        if !still || dir_gone {
            return true;
        }
        if started.elapsed() > std::time::Duration::from_secs(20) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
}

/// 卸载后校验该软件是否仍在注册表已安装列表中
fn is_still_installed(display_name: &str) -> bool {
    if display_name.is_empty() {
        return false;
    }
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            if let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) {
                for key in root.enum_keys().flatten() {
                    if let Ok(sub) = root.open_subkey(&key) {
                        let n: String = reg_str(&sub, "DisplayName");
                        if n == display_name {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// 在软件自身安装目录里寻找自带的卸载程序（优先级最高的卸载方式）。
///
/// Geek 之所以「几乎什么软件都能卸」，靠的不是一份固定的文件名清单，
/// 而是「注册表线索 + 目录扫描 + 命名语义匹配」三层兜底。这里按同样的思路实现：
///   ① 高频固定名（覆盖 Inno / NSIS / Wise 三大主流安装器的默认命名）
///   ② 语义匹配：文件名含 uninst / 卸载 / 反安装 的任意 exe
///   ③ 子目录兜底：`Uninstall\`、`_uninstall\`（Wise 与部分 InstallShield 的习惯）
///
/// ⚠️ 刻意**不把 `setup.exe` 当卸载器**：绝大多数情况下它是安装程序，
/// 误执行会给用户装一遍软件。它若真是卸载器，注册表的 UninstallString 会直接指向它。
fn find_own_uninstaller(hint: &str) -> Option<PathBuf> {
    let raw = hint.trim().trim_matches('"');
    let first = raw.split(',').next().unwrap_or(raw).trim();
    let p = std::path::Path::new(first);
    let dir = if p.is_file() { p.parent()?.to_path_buf() } else { p.to_path_buf() };
    if !dir.is_dir() {
        return None;
    }

    // ① 高频固定名
    const NAMES: &[&str] = &[
        "unins000.exe", "unins001.exe", "unins002.exe", // Inno Setup
        "uninstall.exe", "uninst.exe", "uninstaller.exe", "_uninst.exe", // NSIS / 通用
        "unwise.exe", "unwise32.exe", // Wise
        "卸载.exe", "反安装.exe", // 国产软件
    ];
    for n in NAMES {
        let c = dir.join(n);
        if c.is_file() {
            return Some(c);
        }
    }

    // ② 语义匹配（一层）
    if let Some(hit) = scan_for_uninstaller(&dir) {
        return Some(hit);
    }

    // ③ 卸载器专用子目录（再深入一层）
    for sub in ["Uninstall", "uninstall", "_uninstall", "Uninst", "卸载"] {
        let d = dir.join(sub);
        if d.is_dir() {
            if let Some(hit) = scan_for_uninstaller(&d) {
                return Some(hit);
            }
        }
    }
    None
}

/// 目录内按命名语义找卸载器，返回名字最短的（通常是主卸载器，
/// 诸如 `uninstall_helper.exe` 这类辅助程序名字更长）
fn scan_for_uninstaller(dir: &std::path::Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let low = name.to_lowercase();
        if !low.ends_with(".exe") {
            continue;
        }
        let semantic = low.contains("uninst")
            || low.contains("unwise")
            || name.contains("卸载")
            || name.contains("反安装");
        if !semantic {
            continue;
        }
        // 排除明显的日志/清单文件残留（虽然 .exe 很少见，仍保守过滤）
        if low.contains("uninstall.log") {
            continue;
        }
        if !e.path().is_file() {
            continue;
        }
        let len = name.chars().count();
        if best.as_ref().map(|(l, _)| len < *l).unwrap_or(true) {
            best = Some((len, e.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// 拆分命令行 → (exe, 参数)，兼容带引号/无引号/含空格路径
fn split_cmd(s: &str) -> (String, String) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    if std::path::Path::new(s).exists() {
        return (s.to_string(), String::new());
    }
    let parts: Vec<&str> = s.split_whitespace().collect();
    for i in (1..=parts.len()).rev() {
        let p = parts[..i].join(" ");
        if std::path::Path::new(&p).exists() {
            return (p, parts[i..].join(" "));
        }
    }
    (parts.first().copied().unwrap_or("").to_string(), parts.get(1..).map(|r| r.join(" ")).unwrap_or_default())
}

/// 一次启动卸载器的结果。
///
/// 之所以要区分这么细：交互式卸载器和静默卸载器的「正确等待方式」完全不同 ——
/// 静默的没窗口，等它退出即可；交互式的一点开就弹向导等人点，
/// **此时长期阻塞只会让整个界面看起来卡死**，正确做法是立刻交还控制权、让前端去轮询。
enum LaunchOutcome {
    /// 卸载器已退出（拿到退出码）
    Exited(i32, String),
    /// 卸载器仍在运行（多半是弹了交互向导）：我们不再等，也不终止它
    StillRunning(String),
    /// 静默模式超时，已终止整个进程树
    TimedOut(String),
    /// 用户点了「停止等待」
    Cancelled,
    /// 根本没启动起来
    Failed(String),
}

/// 静默卸载的单次等待上限。
/// 超过这个时间基本可以断定卡住了（正常静默卸载在 5~60 秒内结束）
const SILENT_WAIT_SECS: u64 = 120;
/// 交互式卸载器只观察这么久：之后它肯定在等用户点按钮，继续等没有意义
const INTERACTIVE_WATCH_SECS: u64 = 20;
/// 整次卸载（跨多个候选命令）的总预算，防止「每个候选各超时一次」累积成几十分钟
const UNINSTALL_BUDGET_SECS: u64 = 180;

/// 终止进程树（只用于静默模式：没有窗口可让用户操作，卡住就是真卡住）
fn kill_tree(pid: u32) {
    let _ = crate::commands::silent_command("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

/// 启动卸载程序并按模式等待，返回结果。
///
/// 启动策略见下方注释：优先直接执行 exe，cmd 只作为「命令不带路径」时的回退。
fn launch_uninstaller(cmd_line: &str, silent: bool) -> LaunchOutcome {
    use std::os::windows::process::CommandExt;

    let cmd_line = cmd_line.trim();
    if cmd_line.is_empty() {
        return LaunchOutcome::Failed("卸载命令为空".into());
    }
    let wait_secs = if silent { SILENT_WAIT_SECS } else { INTERACTIVE_WATCH_SECS };
    let expanded = expand_env(cmd_line);
    let (exe, args) = split_cmd(&expanded);

    // ① 首选：**直接启动 exe，不经过 cmd**
    //
    // 借鉴 Bulk Crap Uninstaller 的做法：它自己解析命令行后用 Process.Start 启动，
    // 从不经过 cmd。原因有三：
    //   · cmd 会二次解析 `& | ^ < >`，路径或参数里含这些字符时会截断甚至执行到别的命令
    //   · cmd 对「带路径的引号」有一堆历史遗留规则（实测 args() 写法直接退出码 1）
    //   · 多一层 cmd 就多一个可能失败的进程，出错时难以归因
    // 直接启动时参数用 raw_arg 原样传递，由卸载器自己按标准 C 规则解析。
    if !exe.is_empty() && std::path::Path::new(&exe).is_file() {
        let mut c = crate::commands::silent_command(&exe);
        if !args.trim().is_empty() {
            c.raw_arg(&args);
        }
        if let Ok(child) = c.spawn() {
            return wait_child(child, wait_secs, silent);
        }
    }

    // ② 回退：cmd /C（用于 `MsiExec.exe` 这类不带路径、需要 PATH 解析的命令）
    //
    // ⚠️ 必须用 raw_arg 并且给整条命令再套一对引号，不能用 args(["/C", line])：
    //   Rust 的 args() 按 MSVC 规则把参数内部 `"` 转义成 `\"`，cmd 不认这种转义，
    //   于是 `"C:\Program Files\App\unins000.exe" /S` 会被拆错并报「不是内部或外部命令」。
    let mut c = crate::commands::silent_command("cmd");
    c.raw_arg("/C");
    c.raw_arg(format!("\"{expanded}\""));
    match c.spawn() {
        Ok(child) => wait_child(child, wait_secs, silent),
        Err(_) => launch_via_shellexecute(&expanded),
    }
}

/// 轮询等待子进程，期间响应取消
fn wait_child(mut child: std::process::Child, wait_secs: u64, silent: bool) -> LaunchOutcome {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let pid = child.id();
    let started = Instant::now();
    loop {
        if UNINSTALL_CANCEL.load(Ordering::SeqCst) {
            // 静默模式没有窗口，卡住就是真卡住 → 连子进程一起终止
            if silent {
                kill_tree(pid);
                let _ = child.wait();
            }
            return LaunchOutcome::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(st)) => {
                let code = st.code().unwrap_or(-1);
                let msg = if code == 0 {
                    "卸载程序已完成".to_string()
                } else if code == 3010 {
                    "卸载完成（需重启系统生效）".to_string()
                } else {
                    format!("卸载程序已退出（退出码 {code}）")
                };
                return LaunchOutcome::Exited(code, msg);
            }
            Ok(None) => {
                if started.elapsed() > Duration::from_secs(wait_secs) {
                    if silent {
                        kill_tree(pid);
                        let _ = child.wait();
                        return LaunchOutcome::TimedOut(format!(
                            "静默卸载超过 {wait_secs} 秒未结束，已终止"
                        ));
                    }
                    // 交互式：**不终止**，交还控制权让用户在向导里操作
                    return LaunchOutcome::StillRunning(format!(
                        "卸载程序已启动，正在等待你在卸载向导中完成操作（已观察 {wait_secs} 秒）"
                    ));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return LaunchOutcome::Failed(format!("等待卸载进程失败：{e}")),
        }
    }
}

/// 最后兜底：ShellExecuteW 异步启动（兼容需提权/特殊启动方式的卸载器）。
/// 拿不到进程句柄，一律按「仍在运行」交给前端轮询。
fn launch_via_shellexecute(cmd_line: &str) -> LaunchOutcome {
    use std::os::windows::ffi::OsStrExt;

    let (exe, args) = split_cmd(cmd_line);
    if exe.is_empty() {
        return LaunchOutcome::Failed("无法解析卸载命令".into());
    }
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let exe_w: Vec<u16> = std::ffi::OsStr::new(&exe).encode_wide().chain(Some(0)).collect();
    let args_w: Vec<u16> = if args.is_empty() {
        vec![0]
    } else {
        args.encode_utf16().chain(Some(0)).collect()
    };
    let rc = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            exe_w.as_ptr(),
            args_w.as_ptr(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    if rc as isize > 32 {
        LaunchOutcome::StillRunning("已启动卸载程序（ShellExecute 异步模式）".into())
    } else if rc as isize == 5 {
        LaunchOutcome::Failed("用户取消了 UAC 提权".into())
    } else {
        LaunchOutcome::Failed(format!("卸载程序启动失败（ShellExecute 错误码 {}）", rc as isize))
    }
}

/// 递归删除注册表键（winreg 无 delete_tree，自行实现）
fn reg_delete_tree(hive: HKEY, path: &str) -> Result<(), String> {
    let (parent_path, target) = path.rsplit_once('\\').ok_or("路径无效")?;
    let parent = RegKey::predef(hive)
        .open_subkey_with_flags(parent_path, KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;
    if let Ok(sub) = parent.open_subkey_with_flags(target, KEY_READ) {
        for k in sub.enum_keys().flatten() {
            let _ = sub.delete_subkey(&k); // 先删一层子键（残留在常规软件键下通常只有 1-2 层）
        }
    }
    parent.delete_subkey(target).map_err(|e| e.to_string())
}

/// Geek 式残留扫描：注册表 + 文件目录
#[tauri::command]
pub async fn scan_app_residuals(app_name: String) -> Vec<ResidualItem> {
    tauri::async_runtime::spawn_blocking(move || {
        // 残留扫描含目录遍历，走串行锁避免与软件枚举互相拖慢
        let _guard = APPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan_app_residuals_impl(app_name)))
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// 从注册表该软件的条目中提取线索：安装目录路径 + 目录名关键词
/// （Geek/Revo 的关键做法：软件目录名往往与显示名不同，如「网易云音乐」目录是 CloudMusic）
fn app_clues(app_name: &str) -> (Vec<String>, Vec<PathBuf>) {
    let mut keys: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) else { continue };
            for key in root.enum_keys().flatten() {
                let Ok(sub) = root.open_subkey(&key) else { continue };
                let n: String = reg_str(&sub, "DisplayName");
                if n.is_empty() || !(n.eq_ignore_ascii_case(app_name) || n.contains(app_name) || app_name.contains(&n)) {
                    continue;
                }
                let loc: String = reg_str(&sub, "InstallLocation");
                let un: String = reg_str(&sub, "UninstallString");
                let icon: String = reg_str(&sub, "DisplayIcon");
                let pubr: String = reg_str(&sub, "Publisher");
                for raw in [loc.clone(), un, icon, pubr] {
                    if raw.trim().is_empty() {
                        continue;
                    }
                    let cleaned = raw.trim().trim_matches('"');
                    let first = cleaned.split(',').next().unwrap_or("").trim().trim_matches('"');
                    let p = PathBuf::from(first.split_whitespace().next().unwrap_or(first));
                    let dir = if p.is_file() { p.parent().map(|x| x.to_path_buf()) } else { Some(p.clone()) };
                    if let Some(d) = dir {
                        if let Some(name) = d.file_name() {
                            let s = name.to_string_lossy().to_lowercase();
                            if s.len() >= 3 { keys.push(s); }
                        }
                        // 上一级目录名（如 ...\Netease\CloudMusic → netease）
                        if let Some(par) = d.parent().and_then(|x| x.file_name()) {
                            let s = par.to_string_lossy().to_lowercase();
                            if s.len() >= 3 && s != "program files" && s != "program files (x86)" && s != "appdata" && s != "local" && s != "roaming" {
                                keys.push(s);
                            }
                        }
                        if d.is_dir() && !dirs.contains(&d) {
                            dirs.push(d);
                        }
                    }
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    (keys, dirs)
}

/// 扫描计划任务（Geek/HiBit 必扫项：很多软件卸载后计划任务仍在，会自己把自己拉起来）
fn scan_scheduled_tasks(needles: &[String]) -> Vec<String> {
    let script = format!(
        "{} Get-ScheduledTask -ErrorAction SilentlyContinue | ForEach-Object {{ \"$($_.TaskPath)$($_.TaskName)\" }}",
        crate::commands::PS_UTF8_PRELUDE
    );
    let Ok(out) = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    else {
        return Vec::new();
    };
    let text = crate::commands::decode_out(&out.stdout);
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.as_str() != "\\")
        .filter(|l| {
            let low = l.to_lowercase();
            needles.iter().any(|k| low.contains(k.as_str()))
        })
        .take(40)
        .collect()
}

/// 扫描防火墙规则（软件常留下入站 / 出站规则）
fn scan_firewall_rules(needles: &[String]) -> Vec<String> {
    let script = format!(
        "{} Get-NetFirewallRule -ErrorAction SilentlyContinue | ForEach-Object {{ $_.DisplayName }}",
        crate::commands::PS_UTF8_PRELUDE
    );
    let Ok(out) = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    else {
        return Vec::new();
    };
    let text = crate::commands::decode_out(&out.stdout);
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let low = l.to_lowercase();
            needles.iter().any(|k| low.contains(k.as_str()))
        })
        .map(|l| l.to_string())
        .take(40)
        .collect()
}

/// 扫描 PATH 环境变量中指向该软件的目录段（Geek/HiBit 会摘掉这类残留段）
fn scan_path_env_entries(needles: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (hive, path, tag) in [
        (HKEY_CURRENT_USER, r"Environment", "HKCU"),
        (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "HKLM"),
    ] {
        let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else { continue };
        let Ok(v) = k.get_value::<String, _>("Path") else { continue };
        for seg in v.split(';') {
            let s = seg.trim();
            if s.is_empty() {
                continue;
            }
            let low = s.to_lowercase();
            if needles.iter().any(|n| low.contains(n.as_str())) {
                let s = s.to_string();
                if !out.iter().any(|(_, e)| *e == s) {
                    out.push((tag.to_string(), s));
                }
            }
        }
    }
    out
}

/// 卸载残留扫描入口
fn scan_app_residuals_impl(app_name: String) -> Vec<ResidualItem> {
    let mut out: Vec<ResidualItem> = Vec::new();
    let mut n = 0;

    // ---- 关键词：显示名 + 首词 + 中文前 4 字 + 注册表线索（安装目录名等）----
    let full = app_name.to_lowercase();
    let mut needles: Vec<String> = vec![full.clone()];
    let first = app_name.split([' ', '-', '_']).next().unwrap_or(&app_name).to_lowercase();
    if first.len() >= 3 && first != full {
        needles.push(first);
    }
    if !app_name.is_ascii() && app_name.chars().count() > 4 {
        needles.push(app_name.chars().take(4).collect::<String>().to_lowercase());
    }
    let (extra_keys, install_dirs) = app_clues(&app_name);
    needles.extend(extra_keys);
    needles.sort();
    needles.dedup();
    let needles: Vec<String> = needles.into_iter().filter(|k| !k.is_empty() && !is_whitelisted(k)).collect();

    let hit = |s: &str| {
        let l = s.to_lowercase();
        needles.iter().any(|k| l.contains(k.as_str()))
    };

    // ---- ① 注册表线索中的安装目录：仍存在则直接列为高置信度残留 ----
    for d in &install_dirs {
        if d.is_dir() {
            let (size, files) = dir_stats(d, 4);
            n += 1;
            out.push(ResidualItem {
                id: format!("r{n}"),
                path: d.to_string_lossy().to_string(),
                kind: "folder".into(),
                size,
                selected: true,
                detail: Some(format!("原安装目录（注册表记录） · {files} 个文件")),
            });
        }
    }

    // ---- ② 注册表：HKCU\Software 与 HKLM\SOFTWARE 下的同名子键 ----
    for (hive, root_path, tag) in [
        (HKEY_CURRENT_USER, "Software", "HKCU"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE", "HKLM"),
    ] {
        if let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(root_path, KEY_READ) {
            for key in root.enum_keys().flatten() {
                // 命中关键词且不在系统/驱动厂商白名单内，避免误报
                if hit(&key) && !is_whitelisted(&key) {
                    let subs = root
                        .open_subkey_with_flags(&key, KEY_READ)
                        .map(|k| k.enum_keys().flatten().count())
                        .unwrap_or(0);
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: format!("{tag}\\{root_path}\\{key}"),
                        kind: "registry".into(),
                        size: 0,
                        selected: true,
                        detail: Some(if subs > 0 {
                            format!("软件配置注册表键 · 含 {subs} 个子键")
                        } else {
                            "软件配置注册表键".into()
                        }),
                    });
                }
            }
        }
    }

    // ② 开机启动项（HKCU/HKLM Run + Wow6432Node Run）
    for (hive, path, tag) in [
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run", "HKCU"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "HKLM"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run", "HKLM(x86)"),
    ] {
        if let Ok(run) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) {
            for (vname, _) in run.enum_values().flatten() {
                if hit(&vname) {
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: format!("{tag}\\Run\\{vname}"),
                        kind: "startup".into(),
                        size: 0,
                        selected: true,
                        detail: Some(format!("开机自启动项：{vname}")),
                    });
                }
            }
        }
    }

    // ③ 服务（HKLM\SYSTEM\CurrentControlSet\Services 下名称含关键词，跳过微软自带）
    if let Ok(svc) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Services", KEY_READ)
    {
        for key in svc.enum_keys().flatten() {
            if hit(&key) && !is_whitelisted(&key) {
                n += 1;
                out.push(ResidualItem {
                    id: format!("r{n}"),
                    path: format!("HKLM\\SYSTEM\\CurrentControlSet\\Services\\{key}"),
                    kind: "service".into(),
                    size: 0,
                    selected: false, // 服务残留默认不勾选（需管理员且需人工确认）
                    detail: Some("后台服务键（清理需管理员权限）".into()),
                });
            }
        }
    }

    // ④ 快捷方式（开始菜单 / 公共桌面 / 当前用户桌面）
    let mut lnk_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        lnk_dirs.push(PathBuf::from(&a).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        lnk_dirs.push(PathBuf::from(&p).join("Desktop"));
        lnk_dirs.push(PathBuf::from(&p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(u) = std::env::var("USERPROFILE") {
        lnk_dirs.push(PathBuf::from(&u).join("Desktop"));
    }
    for dir in lnk_dirs {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                if fname.to_lowercase().ends_with(".lnk") && hit(fname.trim_end_matches(".lnk")) {
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: e.path().to_string_lossy().to_string(),
                        kind: "shortcut".into(),
                        size: 0,
                        selected: true,
                        detail: Some("快捷方式（开始菜单 / 桌面）".into()),
                    });
                }
            }
        }
    }

    // ⑤ 计划任务（Geek/HiBit 必扫：卸载后计划任务常在，会自己把自己重新拉起来）
    for task in scan_scheduled_tasks(&needles) {
        n += 1;
        out.push(ResidualItem {
            id: format!("r{n}"),
            path: format!("计划任务：{task}"),
            kind: "task".into(),
            size: 0,
            selected: true,
            detail: Some("计划任务（卸载后仍可能自动拉起该软件）".into()),
        });
    }

    // ⑥ 防火墙规则（软件常留下入站 / 出站规则）
    for fr in scan_firewall_rules(&needles) {
        n += 1;
        out.push(ResidualItem {
            id: format!("r{n}"),
            path: format!("防火墙规则：{fr}"),
            kind: "firewall".into(),
            size: 0,
            selected: true,
            detail: Some("Windows Defender 防火墙规则".into()),
        });
    }

    // ⑦ PATH 环境变量中的目录段（卸载后忘了摘的路径）
    for (tag, seg) in scan_path_env_entries(&needles) {
        n += 1;
        out.push(ResidualItem {
            id: format!("r{n}"),
            path: format!("{tag}\\Environment\\Path 片段：{seg}"),
            kind: "pathenv".into(),
            size: 0,
            selected: true,
            detail: Some("PATH 环境变量残留目录段（应从 Path 中移除）".into()),
        });
    }

    // ⑧ 启动文件夹（Geek 会扫 Startup 与中文「启动」目录）
    let mut su_dirs: Vec<PathBuf> = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        if let Ok(v) = std::env::var(var) {
            let base = PathBuf::from(v);
            su_dirs.push(base.join(r"Microsoft\Windows\Start Menu\Programs\Startup"));
            su_dirs.push(base.join(r"Microsoft\Windows\Start Menu\Programs\启动"));
        }
    }
    for dir in su_dirs {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                if !fname.is_empty() && hit(&fname) {
                    let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: e.path().to_string_lossy().to_string(),
                        kind: "startupfile".into(),
                        size: sz,
                        selected: true,
                        detail: Some("开机启动文件夹内的残留项".into()),
                    });
                }
            }
        }
    }

    // ⑨ COM / 文件关联类注册表键（HKLM 与 WOW6432Node 的 Classes\<key>）
    for (root_path, tag) in [("SOFTWARE\\Classes", "HKLM"), ("SOFTWARE\\WOW6432Node\\Classes", "HKLM(x86)")] {
        if let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(root_path, KEY_READ) {
            for key in root.enum_keys().flatten() {
                // 只取顶层且像 ProgID 的键（xxx.yyy / xxx.yyy.1），避开海量 CLSID
                let dots = key.matches('.').count();
                let looks_progid = (1..=3).contains(&dots)
                    && key.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
                    && !key.starts_with("CLSID")
                    && !key.starts_with("TypeLib")
                    && !key.starts_with("Interface")
                    && !key.starts_with("AppID");
                if looks_progid && hit(&key) && !is_whitelisted(&key) {
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: format!("{tag}\\{root_path}\\{key}"),
                        kind: "class".into(),
                        size: 0,
                        selected: true,
                        detail: Some("COM / 文件关联注册表键".into()),
                    });
                }
            }
        }
    }

    // 文件目录（Geek 做法：Program Files / AppData 递归两层，一层会漏掉装在子文件夹里的软件）
    let mut base_dirs: Vec<(String, PathBuf)> = vec![
        ("C:\\Program Files".into(), PathBuf::from("C:\\Program Files")),
        ("C:\\Program Files (x86)".into(), PathBuf::from("C:\\Program Files (x86)")),
    ];
    if let Ok(appdata) = std::env::var("APPDATA") {
        base_dirs.push(("%AppData%".into(), PathBuf::from(appdata)));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        base_dirs.push(("%LocalAppData%".into(), PathBuf::from(local)));
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        base_dirs.push(("%ProgramData%".into(), PathBuf::from(pd)));
    }
    for (tag, base) in base_dirs {
        if let Ok(rd) = std::fs::read_dir(&base) {
            for entry in rd.flatten() {
                let dn = entry.file_name().to_string_lossy().to_string();
                if hit(&dn) {
                    let (size, files) = dir_stats(&entry.path(), 3);
                    n += 1;
                    out.push(ResidualItem {
                        id: format!("r{n}"),
                        path: format!("{tag}\\{dn}"),
                        kind: "folder".into(),
                        size,
                        selected: true,
                        detail: Some(format!("残留目录 · {files} 个文件")),
                    });
                }
            }
        }
    }
    out
}

/// 清理选中残留：目录/快捷方式直接删除，注册表键与启动项直接删，服务键需管理员
/// 返回 (释放字节数, 失败项数)；单项失败跳过并继续，不会卡住整批
#[tauri::command]
pub async fn clean_residuals(items: Vec<ResidualItem>) -> Result<(u64, u32), String> {
    tauri::async_runtime::spawn_blocking(move || clean_residuals_impl(items))
        .await
        .map_err(|e| format!("残留清理任务失败: {e}"))?
}

fn clean_residuals_impl(items: Vec<ResidualItem>) -> Result<(u64, u32), String> {
    let mut freed = 0u64;
    let mut failed = 0u32;
    for it in &items {
        if !it.selected {
            continue;
        }
        match it.kind.as_str() {
            "registry" => {
                let (hive, rest) = if it.path.starts_with("HKCU") {
                    (HKEY_CURRENT_USER, it.path.trim_start_matches("HKCU\\"))
                } else {
                    (HKEY_LOCAL_MACHINE, it.path.trim_start_matches("HKLM\\"))
                };
                if reg_delete_tree(hive, rest).is_err() {
                    failed += 1;
                }
            }
            "startup" => {
                // path 格式：HKCU\Run\<值名> | HKLM\Run\<值名> | HKLM(x86)\Run\<值名>
                let hive = if it.path.starts_with("HKCU") { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
                let Some((_, value_name)) = it.path.rsplit_once('\\') else { failed += 1; continue };
                let open_path = if it.path.starts_with("HKCU") {
                    r"Software\Microsoft\Windows\CurrentVersion\Run"
                } else if it.path.contains("(x86)") {
                    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run"
                } else {
                    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run"
                };
                let ok = RegKey::predef(hive)
                    .open_subkey_with_flags(open_path, KEY_READ | KEY_WRITE)
                    .and_then(|run| run.delete_value(value_name))
                    .is_ok();
                if !ok { failed += 1; }
            }
            "service" => {
                if reg_delete_tree(HKEY_LOCAL_MACHINE, it.path.trim_start_matches("HKLM\\")).is_err() {
                    failed += 1; // 通常是缺少管理员权限
                }
            }
            "shortcut" => {
                let p = PathBuf::from(&it.path);
                if p.is_file() {
                    let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
                    match std::fs::remove_file(&p) {
                        Ok(()) => freed += sz,
                        Err(_) => failed += 1,
                    }
                }
            }
            "folder" => {
                match resolve_base_dir(&it.path) {
                    Some(real) => {
                        let (size, _) = dir_stats(&real, 4);
                        // 直接删除（残留清理追求彻底与速度，不占回收站）
                        match std::fs::remove_dir_all(&real) {
                            Ok(()) => freed += size,
                            Err(_) => failed += 1, // 文件被占用或无权限
                        }
                    }
                    None => failed += 1,
                }
            }
            // 计划任务：Unregister-ScheduledTask（卸载后残留的计划任务会自己把软件拉起来）
            "task" => {
                let Some((_, raw)) = it.path.split_once("计划任务：") else { failed += 1; continue };
                let full = raw.trim();
                let (tpath, tname) = match full.rsplit_once('\\') {
                    Some((p, n)) => (p, n),
                    None => ("\\", full),
                };
                let script = format!(
                    "{} Unregister-ScheduledTask -TaskName '{}' -TaskPath '{}' -Confirm:$false -ErrorAction SilentlyContinue",
                    crate::commands::PS_UTF8_PRELUDE,
                    tname.replace('\'', "''"),
                    tpath.replace('\'', "''")
                );
                let ok = crate::commands::silent_command("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if !ok { failed += 1 }
            }
            // 防火墙规则：Remove-NetFirewallRule
            "firewall" => {
                let Some((_, raw)) = it.path.split_once("防火墙规则：") else { failed += 1; continue };
                let script = format!(
                    "{} Remove-NetFirewallRule -DisplayName '{}' -ErrorAction SilentlyContinue",
                    crate::commands::PS_UTF8_PRELUDE,
                    raw.trim().replace('\'', "''")
                );
                let ok = crate::commands::silent_command("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if !ok { failed += 1 }
            }
            // PATH 环境变量残留段：从 Path 值里摘掉这一段（保留其余段）
            "pathenv" => {
                let Some((_, raw)) = it.path.split_once("Path 片段：") else { failed += 1; continue };
                let seg = raw.trim().to_string();
                let (hive, env_path) = if it.path.starts_with("HKCU") {
                    (HKEY_CURRENT_USER, "Environment")
                } else {
                    (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
                };
                let ok = match RegKey::predef(hive).open_subkey_with_flags(env_path, KEY_READ | KEY_WRITE) {
                    Ok(k) => {
                        let cur: String = k.get_value("Path").unwrap_or_default();
                        let parts: Vec<&str> = cur.split(';').collect();
                        let kept: Vec<&str> = parts
                            .iter()
                            .copied()
                            .filter(|s| !s.trim().eq_ignore_ascii_case(&seg))
                            .collect();
                        if kept.len() == parts.len() {
                            false // 没找到该段，无需改写
                        } else {
                            k.set_value("Path", &kept.join(";")).is_ok()
                        }
                    }
                    Err(_) => false,
                };
                if !ok { failed += 1 }
            }
            // COM / 文件关联类注册表键
            "class" => {
                let rest = it
                    .path
                    .trim_start_matches("HKLM(x86)")
                    .trim_start_matches("HKLM")
                    .trim_start_matches("HKCU")
                    .trim_start_matches('\\')
                    .replace("SOFTWARE\\WOW6432Node\\", "SOFTWARE\\");
                if rest.is_empty() || reg_delete_tree(HKEY_LOCAL_MACHINE, &rest).is_err() {
                    failed += 1
                }
            }
            // 启动文件夹内的残留（文件或子目录）
            "startupfile" => {
                let p = PathBuf::from(&it.path);
                if p.is_file() {
                    let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
                    match std::fs::remove_file(&p) {
                        Ok(()) => freed += sz,
                        Err(_) => failed += 1,
                    }
                } else if p.is_dir() {
                    let (sz, _) = dir_stats(&p, 2);
                    match std::fs::remove_dir_all(&p) {
                        Ok(()) => freed += sz,
                        Err(_) => failed += 1,
                    }
                } else {
                    failed += 1
                }
            }
            _ => {}
        }
    }
    Ok((freed, failed))
}

fn resolve_base_dir(shown: &str) -> Option<PathBuf> {
    let name = shown.rsplit('\\').next()?;
    for var in ["APPDATA", "LOCALAPPDATA", "ProgramData"] {
        if shown.starts_with('%') {
            if let Ok(v) = std::env::var(var) {
                let candidate = PathBuf::from(&v).join(name);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }
    let candidate = PathBuf::from("C:\\Program Files").join(name);
    if candidate.is_dir() {
        return Some(candidate);
    }
    let candidate = PathBuf::from("C:\\Program Files (x86)").join(name);
    candidate.is_dir().then_some(candidate)
}

// ---------------- 软件图标提取（DisplayIcon → HICON → PNG dataURL） ----------------

/// 解析 DisplayIcon 值为 (图标文件路径, 索引)
fn parse_display_icon(v: &str) -> Option<(String, i32)> {
    let v = v.trim().trim_matches('"');
    if v.is_empty() {
        return None;
    }
    // 形如 `C:\x\app.exe,-1` 或 `C:\x\icon.ico`
    if let Some((path, idx)) = v.rsplit_once(',') {
        if idx.trim().parse::<i32>().is_ok() && path.to_lowercase().ends_with(".exe") || path.to_lowercase().ends_with(".dll") || path.to_lowercase().ends_with(".ico") {
            let p = path.trim().trim_matches('"').to_string();
            if std::path::Path::new(&p).exists() {
                return Some((p, idx.trim().parse::<i32>().unwrap_or(0)));
            }
            return None;
        }
    }
    if v.to_lowercase().ends_with(".exe") || v.to_lowercase().ends_with(".dll") || v.to_lowercase().ends_with(".ico") {
        if std::path::Path::new(v).exists() {
            return Some((v.to_string(), 0));
        }
    }
    None
}

/// HICON → PNG 字节（32bpp BGRA → RGBA）
fn icon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    unsafe {
        let mut info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut info) == 0 {
            return None;
        }
        let mut bm: BITMAP = std::mem::zeroed();
        let ok = GetObjectW(
            info.hbmColor as _,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut _ as *mut core::ffi::c_void,
        );
        if ok == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            DeleteObject(info.hbmMask as _);
            if !info.hbmColor.is_null() {
                DeleteObject(info.hbmColor as _);
            }
            return None;
        }
        let w = bm.bmWidth;
        let h = bm.bmHeight.abs();
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0; // BI_RGB
        let hdc = GetDC(std::ptr::null_mut());
        let got = GetDIBits(
            hdc,
            info.hbmColor,
            0,
            h as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(std::ptr::null_mut(), hdc);
        DeleteObject(info.hbmMask as _);
        DeleteObject(info.hbmColor as _);
        if got == 0 {
            return None;
        }
        // BGRA → RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, buf)?;
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
        Some(out)
    }
}

/// 提取某软件图标，返回 data URL（失败/商店应用返回 None，前端回落通用图标）
#[tauri::command]
pub async fn get_app_icon(id: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || get_app_icon_impl(id))
        .await
        .unwrap_or(None)
}

fn get_app_icon_impl(id: String) -> Option<String> {
    let key_name = id.strip_prefix("win32:")?;
    let mut display_icon = String::new();
    let mut uninstall_str = String::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for path in WIN32_UNINSTALL_PATHS {
            let full_path = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", path.trim_start_matches("SOFTWARE\\"))
            } else {
                path.to_string()
            };
            if let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(&full_path, KEY_READ) {
                if let Ok(sub) = root.open_subkey(key_name) {
                    if display_icon.is_empty() {
                        display_icon = sub.get_value::<String, _>("DisplayIcon").unwrap_or_default();
                    }
                    if uninstall_str.is_empty() {
                        uninstall_str = sub.get_value::<String, _>("UninstallString").unwrap_or_default();
                    }
                    break;
                }
            }
        }
        if !display_icon.is_empty() {
            break;
        }
    }
    // 提取尝试顺序：DisplayIcon → UninstallString（NSIS 卸载器 exe 也有图标可提）
    let mut source = display_icon;
    if parse_display_icon(&source).is_none() {
        source = uninstall_str;
    }
    let Some((path, index)) = parse_display_icon(&source) else { return None };
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut hicon_large: HICON = std::ptr::null_mut();
    let mut hicon_small: HICON = std::ptr::null_mut();
    let hr = unsafe {
        SHDefExtractIconW(
            wide.as_ptr(),
            index,
            0,
            &mut hicon_large,
            &mut hicon_small,
            48 | (32 << 16),
        )
    };
    if hr != 0 || hicon_large.is_null() {
        if !hicon_small.is_null() {
            unsafe { DestroyIcon(hicon_small); }
        }
        return None;
    }
    let png = icon_to_png(hicon_large);
    unsafe { DestroyIcon(hicon_large); }
    if !hicon_small.is_null() {
        unsafe { DestroyIcon(hicon_small); }
    }
    png.map(|bytes| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 游戏加加的脏显示名应被截断() {
        // 实测注册表原值：262 字节，NUL 分隔的多串块被塞进了 REG_SZ
        let raw = "游戏加加\0\0\0\0GamePlusPlusInterUninstallMutex\0GamePP.exe\0\0GamePPLite.exe\0\0TGPPMainForm\0";
        assert_eq!(sanitize_str(raw), "游戏加加");
    }

    #[test]
    fn 正常字符串保持不变() {
        assert_eq!(sanitize_str("Microsoft Visual Studio Code"), "Microsoft Visual Studio Code");
        assert_eq!(sanitize_str("  网易云音乐  "), "网易云音乐");
    }

    #[test]
    fn 首段为空时取第一个非空段() {
        assert_eq!(sanitize_str("\0\0RealName\0junk"), "RealName");
    }

    #[test]
    fn 控制字符被清除() {
        assert_eq!(sanitize_str("名字\u{1}\u{2}尾巴"), "名字尾巴");
    }

    #[test]
    fn 识别_inno_卸载器命名() {
        // Inno 的默认命名：unins + 三位数字
        assert!(is_inno_filename("unins000.exe"));
        assert!(is_inno_filename("unins001.exe"));
        // NSIS 的 uninstall.exe 不能被误判成 Inno（否则静默参数会传错）
        assert!(!is_inno_filename("uninstall.exe"));
        assert!(!is_inno_filename("uninst.exe"));
        // 边界：长度不足时不能 panic（曾在这里踩过切片越界）
        assert!(!is_inno_filename("unins"));
        assert!(!is_inno_filename("unins000"));
        assert!(!is_inno_filename(""));
    }

    #[test]
    fn 展开环境变量() {
        // 能展开的
        let got = expand_env(r"%WINDIR%\system32\sdbinst.exe");
        assert!(got.to_lowercase().contains("system32"), "应展开 %WINDIR%，实际 {got}");
        assert!(!got.contains('%'), "不应残留 % 号");
        // 不存在的变量原样保留，不做半截替换
        assert_eq!(expand_env("%NOT_A_REAL_VAR_XYZ%\\a.exe"), "%NOT_A_REAL_VAR_XYZ%\\a.exe");
        // 无变量的字符串原样返回
        assert_eq!(expand_env(r"C:\a\b.exe"), r"C:\a\b.exe");
        // 单个 % 不应 panic
        assert_eq!(expand_env("100%"), "100%");
    }

    #[test]
    fn msi_的命令行被改成卸载() {
        // /I 是「安装/修改」，必须换成 /X，否则打开的是修复界面
        assert_eq!(
            fix_msiexec("MsiExec.exe /I{010792BA-551A-3AC0-A7EF-0FAB4156C382}"),
            "MsiExec.exe /X{010792BA-551A-3AC0-A7EF-0FAB4156C382}"
        );
        // 已经是 /X 的保持不变
        let already = "MsiExec.exe /X{0664B665-622E-490B-87CB-45EF0B52DA46}";
        assert_eq!(fix_msiexec(already), already);
        // 非 msiexec 的命令一律不动
        assert_eq!(fix_msiexec(r#""C:\a\unins000.exe" /S"#), r#""C:\a\unins000.exe" /S"#);
    }
}
