// 临时诊断：用与应用相同的 winreg 读取链路，检查中文 DisplayName 是否被破坏
use winreg::enums::*;
use winreg::RegKey;

fn main() {
    let paths = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hive, path) in paths {
        let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else { continue };
        for key in root.enum_keys().flatten() {
            let Ok(sub) = root.open_subkey(&key) else { continue };
            let direct: String = sub.get_value("DisplayName").unwrap_or_default();
            if direct.is_empty() {
                continue;
            }
            // 只看含非 ASCII 的条目
            if direct.is_ascii() {
                continue;
            }
            let codes: Vec<String> = direct.chars().take(8).map(|c| format!("U+{:04X}", c as u32)).collect();
            let raw = sub.get_raw_value("DisplayName").ok();
            println!("KEY   : {key}");
            println!("  str : {direct:?}");
            println!("  code: {}", codes.join(" "));
            if let Some(r) = raw {
                let head: Vec<String> = r.bytes.iter().take(16).map(|b| format!("{b:02X}")).collect();
                println!("  raw : {} (len={})", head.join(" "), r.bytes.len());
            }
            println!();
        }
    }
}
