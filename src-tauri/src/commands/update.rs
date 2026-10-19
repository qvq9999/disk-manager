//! 自动更新：本模块只补「更新源」这一条信息，实际的检查/下载由 tauri-plugin-updater 完成。
//!
//! 为什么需要它：设置页要能区分「已是最新」和「压根没配更新源」——
//! 后者在 endpoint 还是 `OWNER/REPO` 占位符时，插件只会返回一个 404，
//! 前端拿到 404 无法判断是「仓库没发布」还是「你忘了改配置」。

/// ⚠️ 必须与 `tauri.conf.json` → `plugins.updater.endpoints[0]` 保持一致。
/// 这里只用于**判断是否已配置**，真正的请求由插件按配置的端点列表依次尝试。
///
/// 为什么第一位是镜像：GitHub 的 Release 资产会 302 到 `release-assets.githubusercontent.com`，
/// 该域名在部分网络下被静默丢包（实测直连与代理都超时 20s+），而镜像站是直接回源转发字节，
/// 1 秒内就能拿到。安装包本身还有签名校验，镜像即使被投毒也无法通过验证。
pub const ENDPOINT: &str =
    "https://ghproxy.net/https://github.com/qvq9999/disk-manager/releases/latest/download/latest.json";

/// 配置里是否还留着占位符（用户还没填自己的仓库）
pub const PLACEHOLDER: &str = "OWNER/REPO";

#[tauri::command]
pub fn update_endpoint() -> String {
    ENDPOINT.to_string()
}

// ---------------- 更新残留清理 ----------------
//
// 为什么需要手动清：tauri-plugin-updater 在 Windows 上用
// `tempfile::Builder::new().prefix("disk-manager-<版本>-updater-").tempdir()?.keep()`
// 建临时目录 —— **`.keep()` 明确表示不自动删除**；安装包 exe 也随之落在里面。
// 结果是每更新一个版本，`%TEMP%` 就多一个 `disk-manager-<版本>-updater-XXXX` 目录。
// 实测本机更新三次后留下了 1.2.0 / 1.3.0 / 1.3.1 三个目录。
//
// 清理放在应用启动时异步做一次：新版本启动时安装器早已退出，删除是安全的；
// 若仍被占用（极端情况）删除会失败，下次启动再试，绝不阻塞启动。

/// 目录/文件名是否属于更新器遗留
fn is_updater_leftover(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("disk-manager-") && (n.contains("-updater-") || n.contains("-installer"))
}

/// 在指定目录下清理更新遗留，返回清掉的条目数。
/// 抽成参数化形式是为了能在测试里用沙箱目录，不碰真实 `%TEMP%`。
pub fn cleanup_leftovers_in(dir: &std::path::Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0usize;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !is_updater_leftover(&name) {
            continue;
        }
        let p = e.path();
        // 目录用 remove_dir_all；被占用时返回 Err，跳过（下次启动再清）
        let ok = match e.file_type() {
            Ok(ft) if ft.is_dir() => std::fs::remove_dir_all(&p).is_ok(),
            Ok(_) => std::fs::remove_file(&p).is_ok(),
            Err(_) => false,
        };
        if ok {
            n += 1;
        }
    }
    n
}

/// 启动时调用：清掉 `%TEMP%` 下历史版本留下的更新目录
pub fn cleanup_update_leftovers() -> usize {
    let dir = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    cleanup_leftovers_in(&dir)
}

// ---------------- 安装位置记录 与 多余安装清理 ----------------
//
// 事故背景（v1.3.7）：
//   安装器靠 `Software\<publisher>\<product>` 的默认值记住上次安装目录。
//   v1.3.7 把 publisher 从 WorkBuddy 改成 qvq9999 后，这个键换了名字，旧记录读不到，
//   安装器回落到默认的 `C:\Program Files\disk-manager` —— 原本装在 D 盘的用户
//   于是被装成了两份：D 盘旧版还在，C 盘多一份新版。
//
// 两道修：
//   ① 每次启动写 `RunningFrom`，安装器（windows/installer-hooks.nsh）优先读它，
//      把更新装回同一目录，从源头不再产生第二份；
//   ② 安装器装完打一个 `CleanOthers` 标记，应用下次启动时把"另一份"清掉，
//      用于修复已经产生了两份的机器。

use std::path::{Path, PathBuf};
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
use winreg::RegKey;

/// ⚠️ 必须与 `windows/installer-hooks.nsh` 里的键名完全一致。
pub const META_KEY: &str = r"Software\qvq9999\disk-manager";
/// publisher 改名前的键（v1.3.6 及更早）。
const LEGACY_META_KEYS: &[&str] = &[r"Software\WorkBuddy\disk-manager"];
const UNINST_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\disk-manager";
const MAIN_EXE: &str = "app.exe";
const UNINST_EXE: &str = "uninstall.exe";

fn current_install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(|p| p.to_path_buf())
}

fn norm_path(s: &str) -> Option<PathBuf> {
    let t = s.trim().trim_matches('"').trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

fn read_str(root: &RegKey, key: &str, name: &str) -> Option<String> {
    root.open_subkey_with_flags(key, KEY_READ)
        .ok()?
        .get_value::<String, _>(name)
        .ok()
}

/// 启动时记录「从哪运行的」。
///
/// 只在同目录存在 uninstall.exe 时写：开发时 exe 在 `target\debug` 下，
/// 没有卸载器，不该被当成安装位置（写进去会让安装器把更新装进构建目录）。
pub fn write_running_from() -> bool {
    let Some(dir) = current_install_dir() else { return false };
    if !dir.join(UNINST_EXE).is_file() {
        return false;
    }
    let Ok((key, _)) = RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(META_KEY) else {
        return false;
    };
    key.set_value("RunningFrom", &dir.to_string_lossy().to_string())
        .is_ok()
}

/// 安装器装完会留下标记，读一次并清掉 —— 只有刚更新过才需要清理多余安装。
fn take_clean_flag() -> bool {
    let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(META_KEY, KEY_READ | KEY_WRITE)
    else {
        return false;
    };
    let on = key.get_value::<u32, _>("CleanOthers").map(|v| v == 1).unwrap_or(false);
    if on {
        let _ = key.delete_value("CleanOthers");
    }
    on
}

/// 所有可能藏着「另一份安装」的目录。
fn candidate_dirs() -> Vec<PathBuf> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut out: Vec<PathBuf> = Vec::new();

    // ① 位置记录（新键的 RunningFrom 最准，默认值是上一次的安装目录）
    for k in std::iter::once(META_KEY).chain(LEGACY_META_KEYS.iter().copied()) {
        if let Some(v) = read_str(&root, k, "RunningFrom") {
            if let Some(p) = norm_path(&v) {
                out.push(p);
            }
        }
        if let Some(v) = read_str(&root, k, "") {
            if let Some(p) = norm_path(&v) {
                out.push(p);
            }
        }
    }

    // ② 卸载项里登记的安装位置（带引号）
    if let Some(v) = read_str(&root, UNINST_KEY, "InstallLocation") {
        if let Some(p) = norm_path(&v) {
            out.push(p);
        }
    }

    // ③ 常见默认位置：安装器回落到默认路径时就是这几个之一
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            if !base.trim().is_empty() {
                out.push(PathBuf::from(base).join("disk-manager"));
            }
        }
    }
    out
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca.to_string_lossy().eq_ignore_ascii_case(&cb.to_string_lossy())
}

/// 对某个候选目录该怎么处理。抽成纯函数，好在沙箱目录里测。
#[derive(Debug, PartialEq)]
enum DirPlan {
    /// 不处理（附原因，仅用于日志）
    Skip(&'static str),
    /// 目录里只有本程序的文件 → 整个目录删掉
    RemoveAll,
    /// 还混着别的东西 → 只删本程序的两个文件，保留目录
    RemoveFiles(Vec<PathBuf>),
}

fn plan_for_dir(dir: &Path, current: &Path) -> DirPlan {
    if same_dir(dir, current) {
        return DirPlan::Skip("就是当前正在用的目录");
    }
    if !dir.is_dir() {
        return DirPlan::Skip("目录不存在");
    }
    let main = dir.join(MAIN_EXE);
    let uninst = dir.join(UNINST_EXE);
    if !main.is_file() || !uninst.is_file() {
        return DirPlan::Skip("缺少主程序或卸载器，不确认是本程序");
    }
    // 只有「目录里除了这两个文件没有别的」才敢整目录删；
    // 用户可能往安装目录里放过东西（日志、备份），那些不能碰。
    let mut unknown = false;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n != MAIN_EXE && n != UNINST_EXE {
                unknown = true;
                break;
            }
        }
    }
    if unknown {
        DirPlan::RemoveFiles(vec![main, uninst])
    } else {
        DirPlan::RemoveAll
    }
}

/// 注册表里登记的版本，是否就是当前正在跑的版本。
///
/// 为什么要这道闸：用户可能从「另一份旧安装」启动（两份并存时很常见）。
/// 那种情况下如果把「刚装好的新版」当成多余安装删掉，等于把更新结果毁了。
/// 版本对不上就什么都不做，标记留着，等从新版启动时再清。
fn running_version_matches_installed() -> bool {
    let Ok(key) =
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(UNINST_KEY, KEY_READ)
    else {
        return false;
    };
    let ver: String = key.get_value("DisplayVersion").unwrap_or_default();
    ver.trim() == crate::app_meta::VERSION
}

/// 清掉除当前目录之外的「另一份安装」。返回给日志用的说明。
///
/// 只会在安装器打过 `CleanOthers` 标记后动手，平时调用是零成本的空操作。
pub fn cleanup_other_installs() -> Vec<String> {
    let Some(current) = current_install_dir() else {
        return vec!["取不到当前程序路径，跳过清理".into()];
    };
    if !running_version_matches_installed() || !take_clean_flag() {
        return Vec::new();
    }

    let mut seen: Vec<PathBuf> = Vec::new();
    let mut log = Vec::new();
    for dir in candidate_dirs() {
        // 多个来源可能指向同一个目录
        if seen.iter().any(|s| same_dir(s, &dir)) {
            continue;
        }
        seen.push(dir.clone());

        match plan_for_dir(&dir, &current) {
            DirPlan::Skip(_) => {}
            DirPlan::RemoveAll => {
                let r = crate::commands::fsx::remove_dir(&dir);
                log.push(format!(
                    "{}：{}",
                    dir.display(),
                    if r.ok { "已删除旧安装" } else { "未能完全删除（可能被占用）" }
                ));
            }
            DirPlan::RemoveFiles(files) => {
                for f in files {
                    let r = crate::commands::fsx::remove_file(&f);
                    log.push(format!(
                        "{}：{}",
                        f.display(),
                        if r.ok { "已删除旧文件（目录里还有其它文件，已保留）" } else { "删除失败" }
                    ));
                }
            }
        }
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 更新源指向真实仓库() {
        // 必须是可匿名访问的地址：私有仓库的 Release 资产需要认证，
        // 客户端没有 token，自动更新会直接失效
        assert!(ENDPOINT.starts_with("https://"), "更新源必须是 https");
        assert!(ENDPOINT.ends_with("/latest.json"));
        assert!(
            ENDPOINT.contains("github.com/qvq9999/disk-manager/releases/"),
            "更新源必须指向真实仓库的 Release 资产"
        );
        // 已配置真实地址后不应再命中占位符，否则设置页会一直提示「未配置」
        assert!(
            !ENDPOINT.contains(PLACEHOLDER),
            "更新源仍是占位符，设置页会误报未配置"
        );
    }

    #[test]
    fn 占位符可被识别() {
        let placeholder_url = format!(
            "https://github.com/{PLACEHOLDER}/releases/latest/download/latest.json"
        );
        assert!(placeholder_url.contains(PLACEHOLDER));
    }

    #[test]
    fn 识别更新器遗留目录名() {
        assert!(is_updater_leftover("disk-manager-1.3.1-updater-ZgGuuK"));
        assert!(is_updater_leftover("disk-manager-1.3.1-installer.exe"));
        assert!(is_updater_leftover("Disk-Manager-1.2.0-Updater-AbCdEf"));
        // 无关目录不能误删
        assert!(!is_updater_leftover("disk-manager"));
        assert!(!is_updater_leftover("other-app-1.0.0-updater-x"));
        assert!(!is_updater_leftover("temp123"));
    }

    #[test]
    fn 清理只删更新遗留() {
        let base = std::env::temp_dir().join("__dm_updater_clean_test__");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("disk-manager-1.2.0-updater-Aaa")).unwrap();
        std::fs::create_dir_all(base.join("disk-manager-1.3.0-updater-Bbb")).unwrap();
        std::fs::write(base.join("disk-manager-1.3.0-installer.exe"), b"x").unwrap();
        std::fs::create_dir_all(base.join("keep-me")).unwrap();
        std::fs::write(base.join("unrelated.txt"), b"x").unwrap();

        let n = cleanup_leftovers_in(&base);
        assert_eq!(n, 3, "应清掉两个目录 + 一个安装包");
        assert!(!base.join("disk-manager-1.2.0-updater-Aaa").exists());
        assert!(!base.join("disk-manager-1.3.0-updater-Bbb").exists());
        assert!(!base.join("disk-manager-1.3.0-installer.exe").exists());
        assert!(base.join("keep-me").is_dir(), "无关目录必须保留");
        assert!(base.join("unrelated.txt").is_file(), "无关文件必须保留");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 真实拉一次更新清单，确认「更新源在这台机器上真的能通」。
    ///
    /// 默认跳过（需要联网）。手动跑：
    ///   cargo test --release --lib update::tests::更新源可访问 -- --include-ignored --nocapture
    ///
    /// 刻意 `no_proxy()`：程序从资源管理器启动时拿不到任何代理环境变量，
    /// 走环境代理跑通不代表用户那边能通。这条测的就是「裸连」。
    #[test]
    #[ignore = "需要联网"]
    fn 更新源可访问且返回合法清单() {
        let url = ENDPOINT.to_string();
        let body = tauri::async_runtime::block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .no_proxy()
                .user_agent("disk-manager/update-probe")
                .build()
                .expect("构建 HTTP 客户端失败");
            let resp = client.get(&url).send().await.expect("请求更新源失败");
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            assert!(status.is_success(), "更新源返回 {status}：{}", &text[..text.len().min(200)]);
            text
        });

        let v: serde_json::Value = serde_json::from_str(&body).expect("清单必须是 JSON");
        assert!(v.get("version").is_some(), "清单缺少 version 字段");
        let sig = &v["platforms"]["windows-x86_64"]["signature"];
        assert!(sig.is_string(), "清单缺少签名，自动更新会被拒绝");
        eprintln!(
            "更新源可用：version={} 签名长度={}",
            v["version"],
            sig.as_str().unwrap_or("").len()
        );
    }

    // ---- 多余安装清理：判定逻辑在沙箱目录里测，不碰真实安装 ----

    fn sandbox(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("__dm_install_plan_{tag}__"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join(MAIN_EXE), b"x").unwrap();
        std::fs::write(base.join(UNINST_EXE), b"x").unwrap();
        base
    }

    #[test]
    fn 计划_当前目录必须跳过() {
        let d = sandbox("self");
        assert_eq!(
            plan_for_dir(&d, &d),
            DirPlan::Skip("就是当前正在用的目录"),
            "绝不能把正在运行的这一份当成多余安装删掉"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 计划_缺卸载器时不认定为本程序() {
        let d = sandbox("nouninst");
        std::fs::remove_file(d.join(UNINST_EXE)).unwrap();
        assert!(matches!(
            plan_for_dir(&d, &std::env::temp_dir()),
            DirPlan::Skip(_)
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 计划_干净目录整目录删除() {
        let d = sandbox("clean");
        assert_eq!(plan_for_dir(&d, &std::env::temp_dir()), DirPlan::RemoveAll);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 计划_混有用户文件时只删程序文件() {
        let d = sandbox("mixed");
        std::fs::write(d.join("我的备份.txt"), b"x").unwrap();
        match plan_for_dir(&d, &std::env::temp_dir()) {
            DirPlan::RemoveFiles(f) => {
                assert_eq!(f.len(), 2, "只应删 app.exe 与 uninstall.exe");
                assert!(f.contains(&d.join(MAIN_EXE)));
                assert!(f.contains(&d.join(UNINST_EXE)));
                assert!(
                    !f.iter().any(|p| p.to_string_lossy().contains("备份")),
                    "用户放在安装目录里的文件不能删"
                );
            }
            other => panic!("应为只删程序文件，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 计划_不存在的目录跳过() {
        let ghost = std::env::temp_dir().join("__dm_install_plan_ghost__");
        let _ = std::fs::remove_dir_all(&ghost);
        assert!(matches!(
            plan_for_dir(&ghost, &std::env::temp_dir()),
            DirPlan::Skip(_)
        ));
    }
}
