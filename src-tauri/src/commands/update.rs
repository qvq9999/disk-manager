//! 自动更新：本模块只补「更新源」这一条信息，实际的检查/下载由 tauri-plugin-updater 完成。
//!
//! 为什么需要它：设置页要能区分「已是最新」和「压根没配更新源」——
//! 后者在 endpoint 还是 `OWNER/REPO` 占位符时，插件只会返回一个 404，
//! 前端拿到 404 无法判断是「仓库没发布」还是「你忘了改配置」。

/// ⚠️ 必须与 `tauri.conf.json` → `plugins.updater.endpoints[0]` 保持一致。
/// 这里只用于**展示与判断是否已配置**，真正的请求由插件按配置发起。
pub const ENDPOINT: &str =
    "https://github.com/OWNER/REPO/releases/latest/download/latest.json";

/// 配置里是否还留着占位符（用户还没填自己的仓库）
pub const PLACEHOLDER: &str = "OWNER/REPO";

#[tauri::command]
pub fn update_endpoint() -> String {
    ENDPOINT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 占位符检测() {
        assert!(ENDPOINT.contains(PLACEHOLDER), "默认配置应被识别为未配置");
        // 用户替换成自己的仓库后不应再命中
        let real = ENDPOINT.replace(PLACEHOLDER, "octocat/disk-manager");
        assert!(!real.contains(PLACEHOLDER));
        assert!(real.starts_with("https://github.com/"));
    }
}
