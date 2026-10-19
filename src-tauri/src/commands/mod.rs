pub mod ai;
pub mod ai_knowledge;
pub mod app_cache;
pub mod clean;
pub mod clean_rules;
pub mod disk;
pub mod dupes;
pub mod emptydir;
pub mod fsx;
pub mod hardware;
pub mod identify;
pub mod installer;
pub mod mft;
pub mod residual;
pub mod scan;
pub mod settings;
pub mod smart;
pub mod uninstall;
pub mod update;
pub mod volumes;

/// 创建不弹终端黑窗的子进程命令（CREATE_NO_WINDOW）
/// 卸载向导等 GUI 程序的窗口不受影响，只是父进程 cmd/powershell 不闪黑框
pub fn silent_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 子进程输出解码：PowerShell 在中文 Windows 下可能输出 GBK/ANSI，
/// 直接按 UTF-8 解析会导致中文变方块（曾出现「专业版」显示为乱码）。
/// 策略：先严格 UTF-8，失败则按 GB18030 解码。
pub fn decode_out(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::GB18030.decode(bytes);
            cow.into_owned()
        }
    }
}

/// PowerShell 脚本统一前置：把控制台输出编码切到 UTF-8（配合 decode_out 双保险）
pub const PS_UTF8_PRELUDE: &str = "$ErrorActionPreference='SilentlyContinue'; \
try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}; \
$OutputEncoding=[System.Text.Encoding]::UTF8; ";
