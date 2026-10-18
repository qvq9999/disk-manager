//! 安装包专项清理
//!
//! 目标：把「用户下载过的安装包」找出来，让用户自己挑要删的。
//!
//! 识别策略刻意保守（误删一个安装包用户要重新下载几百 MB，代价远大于漏报）：
//!   ① `.msi` / `.msix` / `.appx` / `.msu` —— 本身就是安装包格式，直接认定
//!   ② `.exe` —— 只有文件名含 setup / install / 安装 这类强特征，且确实是 PE 文件才认定
//!   ③ 一律排除 `unins*`（卸载器，删了软件就卸不干净）、系统目录、程序安装目录里的
//!      `Installer` / `Package Cache` / `WindowsApps`（这些是修复/卸载用的官方缓存）
//!
//! 不做「读文件内容猜是不是安装器」：全盘几十万个 exe 逐个读头部，I/O 代价远超收益，
//! 而安装器文件名几乎都带 setup/install 特征。漏掉的少数（如纯版本号命名）宁可不算。
//!
//! 删除端与扫描端使用同一套判定（`is_installer`），id 编码真实绝对路径，删除前重新校验。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::WalkDir;

/// 用户中止扫描的固定错误码（前端据此区分「停止」与「真失败」）
pub const INSTALLER_CANCELLED: &str = "INSTALLER_CANCELLED";

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallerItem {
    /// 真实绝对路径（删除端据此校验，不编码索引）
    pub path: String,
    pub name: String,
    pub size: u64,
    /// 文件修改时间（Unix 秒）
    pub mtime: u64,
    /// MSI / MSIX / APPX / MSU / EXE
    pub kind: String,
}

/// 这些目录整枝跳过：要么是系统保护，要么是官方安装缓存（删了会破坏卸载/修复）
const SKIP_DIRS: &[&str] = &[
    "windows",
    "winsxs",
    "system32",
    "syswow64",
    "system volume information",
    "$recycle.bin",
    "recycler",
    "windowsapps",
    "recovery",
    "boot",
    "efi",
    "config.msi",
    "installer",       // %WINDIR%\Installer：MSI 卸载/修复缓存
    "package cache",   // Visual Studio / VC++ 运行库安装缓存
    "node_modules",
    ".git",
    "winsxs_backup",
];

fn skip_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    SKIP_DIRS.iter().any(|s| *s == n)
}

/// 是否是 PE 文件（读前 2 字节，成本可忽略）
fn is_pe(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut b = [0u8; 2];
    f.read_exact(&mut b).is_ok() && &b == b"MZ"
}

/// 判定一个文件是否是安装包，返回类型标签。
/// 扫描与删除共用此函数 —— 两边判定不一致就会出现「显示可删、实际删不掉」。
pub fn is_installer(p: &Path) -> Option<&'static str> {
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "msi" => return Some("MSI"),
        "msix" => return Some("MSIX"),
        "appx" => return Some("APPX"),
        "msu" => return Some("MSU"),
        "exe" => {}
        _ => return None,
    }

    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    // 卸载器不是安装包：删了会导致软件无法正常卸载
    if name.contains("unins") {
        return None;
    }
    let looks_like_installer = name.contains("setup")
        || name.contains("install")
        || name.contains("安装");
    if !looks_like_installer {
        return None;
    }
    // 名字像还不够，必须确实是可执行文件（避免把 `.exe` 结尾的文本/占位文件算进来）
    if !is_pe(p) {
        return None;
    }
    Some("EXE")
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 扫描所选盘符下的安装包。`on_progress(pct, 当前路径)` 节流上报。
pub fn scan<F: FnMut(u32, &str)>(drives: &[char], mut on_progress: F) -> Vec<InstallerItem> {
    reset_cancel();
    let mut out: Vec<InstallerItem> = Vec::new();
    let total = drives.len().max(1);

    for (i, &letter) in drives.iter().enumerate() {
        let root = PathBuf::from(format!("{letter}:\\"));
        if !root.is_dir() {
            continue;
        }
        let base = (i as u32 * 100 / total as u32) as u32;
        let span = 100 / total as u32;
        let mut seen = 0u32;

        for e in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // filter_entry 只对目录生效：命中黑名单的整枝不再进入
                if e.file_type().is_dir() {
                    let name = e.file_name().to_string_lossy();
                    !skip_dir(&name)
                } else {
                    true
                }
            })
            .filter_map(|e| e.ok())
        {
            if cancelled() {
                break;
            }
            if e.file_type().is_dir() {
                continue;
            }
            seen += 1;
            // 每 512 个文件报一次进度：既不刷爆前端，也不让计数成为瓶颈
            if seen % 512 == 0 {
                on_progress(base + span / 2, &e.path().to_string_lossy());
            }
            let path = e.path();
            let Some(kind) = is_installer(path) else { continue };
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            // 小于 64KB 的「安装包」几乎都是壳/占位文件，清了没收益还容易误判
            if size < 64 * 1024 {
                continue;
            }
            out.push(InstallerItem {
                path: path.to_string_lossy().to_string(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                size,
                mtime: mtime_secs(path),
                kind: kind.to_string(),
            });
        }
        on_progress(base + span, &format!("{letter}: 扫描完成"));
        if cancelled() {
            break;
        }
    }

    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

/// 删除端安全闸：绝对路径 + 仍是文件 + 仍被认定为安装包 + 不在受保护目录里
pub fn path_allowed(p: &Path) -> bool {
    if !p.is_absolute() || !p.is_file() {
        return false;
    }
    if is_installer(p).is_none() {
        return false;
    }
    // 路径任意一段命中黑名单即拒绝（防止 id 被篡改成系统目录里的文件）
    !p.components()
        .any(|c| skip_dir(&c.as_os_str().to_string_lossy()))
}

/// 删除勾选的安装包，返回 (释放字节, 失败数)
pub fn delete(paths: &[PathBuf], on_file: &mut dyn FnMut(u64, &str)) -> (u64, u32) {
    let mut freed = 0u64;
    let mut failed = 0u32;
    for p in paths {
        if !path_allowed(p) {
            failed += 1;
            continue;
        }
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(p) {
            Ok(()) => {
                freed += size;
                on_file(freed, &p.to_string_lossy());
            }
            Err(_) => failed += 1,
        }
    }
    (freed, failed)
}

// ---------------- Tauri 命令 ----------------

/// 扫描所选盘符下的安装包。用户停止时返回固定错误码 `INSTALLER_CANCELLED`。
#[tauri::command]
pub async fn scan_installers(
    scope: Option<super::clean::CleanScope>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<Vec<InstallerItem>, String> {
    let drives = scope.map(|s| s.drives).unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let letters = super::clean::effective_drives(&drives);
        let items = scan(&letters, |pct, cur| {
            let _ = on_progress.send(serde_json::json!({ "pct": pct, "current": cur }));
        });
        if cancelled() {
            Err(INSTALLER_CANCELLED.to_string())
        } else {
            Ok(items)
        }
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

#[tauri::command]
pub fn cancel_installer_scan() {
    request_cancel();
}

/// 删除勾选的安装包。`paths` 是真实绝对路径，删除端逐个重新校验。
#[tauri::command]
pub async fn delete_installers(
    paths: Vec<String>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<(u64, u32), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let list: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let total: u64 = list
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        delete(&list, &mut |freed, cur| {
            let pct = if total > 0 {
                ((freed * 100 / total) as u32).min(99)
            } else {
                0
            };
            let _ = on_progress.send(serde_json::json!({
                "pct": pct, "freed": freed, "current": cur
            }));
        })
    })
    .await
    .map_err(|e| format!("清理任务失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 卸载器不算安装包() {
        assert!(is_installer(Path::new(r"C:\Downloads\WeChatSetup.exe")).is_none());
        // 文件不存在 → is_pe 为 false → 不算（名字像但内容不是）
        assert!(is_installer(Path::new(r"C:\x\unins000.exe")).is_none());
    }

    #[test]
    fn 系统目录被跳过() {
        for d in ["Windows", "WinSxS", "$Recycle.Bin", "Installer", "Package Cache", "node_modules"] {
            assert!(skip_dir(d), "{d} 必须跳过");
        }
        assert!(!skip_dir("Downloads"));
        assert!(!skip_dir("我的安装包"));
    }

    /// 真实文件判定：构造最小 PE 头（MZ）验证 exe 分支
    #[test]
    fn exe_需文件名特征且为真实PE() {
        let dir = std::env::temp_dir().join("__dm_installer_test__");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let pe = dir.join("FooSetup.exe");
        std::fs::write(&pe, b"MZ\x90\x00rest-of-file").unwrap();
        assert_eq!(is_installer(&pe), Some("EXE"), "setup + 真 PE 应认定");

        let not_pe = dir.join("BarSetup.exe");
        std::fs::write(&not_pe, b"not a pe").unwrap();
        assert!(is_installer(&not_pe).is_none(), "名字像但不是 PE 应排除");

        let plain = dir.join("game.exe");
        std::fs::write(&plain, b"MZ\x90\x00").unwrap();
        assert!(is_installer(&plain).is_none(), "普通 exe 不应被当成安装包");

        let msi = dir.join("pkg.msi");
        std::fs::write(&msi, b"whatever").unwrap();
        assert_eq!(is_installer(&msi), Some("MSI"), "msi 不看内容直接认定");

        let unins = dir.join("unins000.exe");
        std::fs::write(&unins, b"MZ\x90\x00").unwrap();
        assert!(is_installer(&unins).is_none(), "卸载器必须排除");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
