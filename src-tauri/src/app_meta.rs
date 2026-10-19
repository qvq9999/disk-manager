//! 应用身份的唯一来源。
//!
//! 为什么单独一个模块：这些信息以前是散的 —— `Cargo.toml` 写着 `authors = ["you"]`、
//! `license = ""`、`repository = ""`（占位符从没填过），`LICENSE` 与 `tauri.conf.json`
//! 写的是 WorkBuddy，而仓库属主是 qvq9999。结果是同一个问题有四种答案。
//! 更直接的后果：AI 助手的提示词里**没有任何应用元信息**，用户问"这软件谁做的"
//! 时模型只能编造。
//!
//! 现在统一到这里：改署名只改这一个文件（外加构建配置里的 publisher）。
//!
//! ⚠️ 版本号必须走构建注入（`CARGO_PKG_VERSION`），不要在这里写死 ——
//! 写死之后 release.py 改了 Cargo.toml，程序内显示的还是旧版本。

/// 中文名（界面、AI 提示用）
pub const APP_NAME: &str = "存储管理 Disk Manager";
/// 英文名（安装包、任务栏用）
pub const APP_NAME_EN: &str = "Disk Manager";
/// 作者（对外署名 = GitHub 账号）
pub const AUTHOR: &str = "qvq9999";
/// 作者主页
pub const AUTHOR_URL: &str = "https://github.com/qvq9999";
/// 项目仓库
pub const REPO_URL: &str = "https://github.com/qvq9999/disk-manager";
/// 仓库的展示形式（回答里用短写法更像人话）
pub const REPO_LABEL: &str = "github.com/qvq9999/disk-manager";
/// 许可证
pub const LICENSE: &str = "MIT";
/// 版本：构建期注入
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// 锁住"单一身份源"不漂移。
    ///
    /// 这条测试的价值在于：改署名时只改 Cargo.toml 而忘了 app_meta（或反过来），
    /// 编译期就会失败，而不是等到用户问"作者是谁"时才发现 AI 与安装包说法不一致。
    #[test]
    fn 元信息与构建配置一致() {
        assert_eq!(env!("CARGO_PKG_AUTHORS"), AUTHOR, "Cargo.toml 的 authors 必须与 app_meta 一致");
        assert_eq!(
            env!("CARGO_PKG_REPOSITORY"),
            REPO_URL,
            "Cargo.toml 的 repository 必须与 app_meta 一致"
        );
        assert_eq!(env!("CARGO_PKG_LICENSE"), LICENSE, "Cargo.toml 的 license 必须与 app_meta 一致");
        assert_eq!(env!("CARGO_PKG_VERSION"), VERSION, "版本必须来自构建注入");
        assert!(AUTHOR_URL.contains(AUTHOR), "作者主页应指向同一个账号");
        assert!(REPO_URL.contains(AUTHOR), "仓库地址应位于同一个账号下");
    }
}
