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
}
