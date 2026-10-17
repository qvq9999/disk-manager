//! 自动更新：本模块只补「更新源」这一条信息，实际的检查/下载由 tauri-plugin-updater 完成。
//!
//! 为什么需要它：设置页要能区分「已是最新」和「压根没配更新源」——
//! 后者在 endpoint 还是 `OWNER/REPO` 占位符时，插件只会返回一个 404，
//! 前端拿到 404 无法判断是「仓库没发布」还是「你忘了改配置」。

/// ⚠️ 必须与 `tauri.conf.json` → `plugins.updater.endpoints[0]` 保持一致。
/// 这里只用于**展示与判断是否已配置**，真正的请求由插件按配置发起。
pub const ENDPOINT: &str =
    "https://github.com/qvq9999/disk-manager/releases/latest/download/latest.json";

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
    fn 更新源指向真实仓库() {
        // 必须是可匿名访问的地址：私有仓库的 Release 资产需要认证，
        // 客户端没有 token，自动更新会直接失效
        assert!(ENDPOINT.starts_with("https://github.com/"));
        assert!(ENDPOINT.ends_with("/latest.json"));
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
}
