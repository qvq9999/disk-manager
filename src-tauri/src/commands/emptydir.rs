//! 空文件夹扫描与清理
//!
//! 「空」的判定：该目录下**没有任何文件**（递归），且所有子目录也都是空的。
//! 自底向上后序遍历，先把子目录收进结果、再收父目录 —— 这个顺序正好也是删除顺序：
//! 子目录删掉后父目录才变空，`remove_dir` 才可能成功。
//!
//! 安全模型（宁可漏报，绝不误删）：
//!   ① 只删**完全空**的目录：`remove_dir` 遇到非空目录会失败，天然兜底
//!   ② 系统目录整枝跳过（Windows / WinSxS / WindowsApps / $Recycle.Bin / .git …）
//!   ③ 盘根永不入选
//!   ④ 深度上限 20 层；读不了的目录一律视为「非空」
//!   ⑤ id 编码真实绝对路径，删除前重新校验目录仍为空

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const EMPTYDIR_CANCELLED: &str = "EMPTYDIR_CANCELLED";

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// 递归深度上限：再深的目录往往是软件自己的层级，删空目录收益低、风险高
const MAX_DEPTH: u32 = 20;

/// 整枝跳过的目录名（小写精确匹配）
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
    "installer",
    "package cache",
    "node_modules",
    ".git",
    ".svn",
];

fn skip_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    SKIP_DIRS.iter().any(|s| *s == n)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EmptyDir {
    /// 真实绝对路径（删除端据此校验，不编码索引）
    pub path: String,
    /// 目录名（列表展示用，完整路径另有字段）
    pub name: String,
    /// 层级深度（越大越深；删除时按深度倒序执行）
    pub depth: u32,
    pub mtime: u64,
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 返回 true 表示「该目录树里存在文件」（即非空）。
/// 空目录会以**子在前、父在后**的顺序追加到 `out`。
fn walk<F: FnMut(&Path)>(
    dir: &Path,
    depth: u32,
    out: &mut Vec<EmptyDir>,
    seen: &mut u64,
    on_dir: &mut F,
) -> bool {
    if cancelled() {
        return true; // 中止时一律当作非空，绝不半途产出结果
    }
    if depth > MAX_DEPTH {
        return true;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return true; // 无权限/被占用：视为非空
    };

    let mut has_file = false;
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        *seen += 1;
        if *seen % 256 == 0 {
            on_dir(dir);
        }
        // file_type 不跟随符号链接：链接会被当作「文件」，从而让上级目录不再是空目录
        match e.file_type() {
            Ok(ft) if ft.is_dir() => {
                let name = e.file_name().to_string_lossy().to_string();
                if skip_dir(&name) {
                    has_file = true; // 跳过的目录按「有内容」处理，上级不参与清理
                    continue;
                }
                subdirs.push(e.path());
            }
            _ => has_file = true,
        }
    }

    for sub in subdirs {
        if walk(&sub, depth + 1, out, seen, on_dir) {
            has_file = true;
        }
    }

    if !has_file {
        out.push(EmptyDir {
            path: dir.to_string_lossy().to_string(),
            name: dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            depth,
            mtime: mtime_secs(dir),
        });
    }
    has_file
}

/// 扫描所选盘符下的空文件夹
pub fn scan<F: FnMut(u32, &str)>(drives: &[char], mut on_progress: F) -> Vec<EmptyDir> {
    CANCEL.store(false, Ordering::SeqCst);
    let mut out: Vec<EmptyDir> = Vec::new();
    let total = drives.len().max(1);

    for (i, &letter) in drives.iter().enumerate() {
        let root = PathBuf::from(format!("{letter}:\\"));
        if !root.is_dir() {
            continue;
        }
        let base = (i as u32 * 100 / total as u32) as u32;
        let span = 100 / total as u32;
        let mut seen = 0u64;
        let mut last = String::new();

        // 盘根只做「进入子目录」，自身永不作为空目录入选
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut subs: Vec<PathBuf> = Vec::new();
        for e in rd.flatten() {
            if let Ok(ft) = e.file_type() {
                if ft.is_dir() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !skip_dir(&name) {
                        subs.push(e.path());
                    }
                }
            }
        }
        for sub in subs {
            if cancelled() {
                break;
            }
            let mut on_dir = |p: &Path| {
                let s = p.to_string_lossy().to_string();
                if s != last {
                    last = s.clone();
                    on_progress(base + span / 2, &s);
                }
            };
            walk(&sub, 1, &mut out, &mut seen, &mut on_dir);
        }
        on_progress(base + span, &format!("{letter}: 扫描完成"));
        if cancelled() {
            break;
        }
    }

    // 深的在前：删除时先清子目录，父目录随后才会变空
    out.sort_by(|a, b| b.depth.cmp(&a.depth));
    out
}

/// 删除端安全闸：绝对路径 + 仍是目录 + 不在受保护目录下 + 不是盘根
pub fn path_allowed(p: &Path) -> bool {
    if !p.is_absolute() || !p.is_dir() {
        return false;
    }
    // 盘根（如 `D:\`）绝不删
    if p.parent().is_none() {
        return false;
    }
    if p.components().count() < 2 {
        return false;
    }
    if p.components()
        .any(|c| skip_dir(&c.as_os_str().to_string_lossy()))
    {
        return false;
    }
    true
}

/// 删除勾选的空目录。只调用 `remove_dir`（非空必然失败），不做递归删除兜底 ——
/// 一旦目录在扫描后又被写入内容，删除必须失败而不是把新内容一起删掉。
pub fn delete(paths: &[PathBuf], on_file: &mut dyn FnMut(u32, &Path)) -> (u32, u32) {
    let mut removed = 0u32;
    let mut failed = 0u32;
    // 深的优先，保证父目录在同批被删时能成功
    let mut list: Vec<&PathBuf> = paths.iter().collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for p in list {
        if !path_allowed(p) {
            failed += 1;
            continue;
        }
        match std::fs::remove_dir(p) {
            Ok(()) => {
                removed += 1;
                on_file(removed, p);
            }
            Err(_) => failed += 1,
        }
    }
    (removed, failed)
}

// ---------------- Tauri 命令 ----------------

/// 扫描所选盘符下的空文件夹。用户停止时返回固定错误码 `EMPTYDIR_CANCELLED`。
#[tauri::command]
pub async fn scan_empty_dirs(
    scope: Option<super::clean::CleanScope>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<Vec<EmptyDir>, String> {
    let drives = scope.map(|s| s.drives).unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let letters = super::clean::effective_drives(&drives);
        let items = scan(&letters, |pct, cur| {
            let _ = on_progress.send(serde_json::json!({ "pct": pct, "current": cur }));
        });
        if cancelled() {
            Err(EMPTYDIR_CANCELLED.to_string())
        } else {
            Ok(items)
        }
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

#[tauri::command]
pub fn cancel_empty_dir_scan() {
    request_cancel();
}

/// 删除勾选的空目录。`paths` 是真实绝对路径，删除端只接受绝对路径且只调 `remove_dir`。
#[tauri::command]
pub async fn delete_empty_dirs(
    paths: Vec<String>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<(u32, u32), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let list: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let total = list.len().max(1);
        delete(&list, &mut |done, p| {
            let pct = ((done as usize * 100 / total) as u32).min(99);
            let _ = on_progress.send(serde_json::json!({
                "pct": pct, "current": p.to_string_lossy()
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
    fn 系统目录被跳过() {
        for d in ["Windows", "WinSxS", "$Recycle.Bin", "WindowsApps", ".git", "Package Cache"] {
            assert!(skip_dir(d), "{d} 必须跳过");
        }
        assert!(!skip_dir("我的空文件夹"));
        assert!(!skip_dir("Downloads"));
    }

    #[test]
    fn 盘根与系统路径不可删() {
        assert!(!path_allowed(Path::new(r"C:\")));
        assert!(!path_allowed(Path::new(r"C:\Windows")));
        assert!(!path_allowed(Path::new(r"C:\Windows\System32")));
    }

    /// 真实目录树：只收「完全空」的目录，且子在前父在后
    #[test]
    fn 只回收完全空的目录且顺序正确() {
        let base = std::env::temp_dir().join("__dm_emptydir_test__");
        let _ = std::fs::remove_dir_all(&base);
        // 结构：a/(空)  b/c/(空)  d/有文件
        std::fs::create_dir_all(base.join("a")).unwrap();
        std::fs::create_dir_all(base.join("b").join("c")).unwrap();
        std::fs::create_dir_all(base.join("d")).unwrap();
        std::fs::write(base.join("d").join("keep.txt"), b"x").unwrap();

        let mut out = Vec::new();
        let mut seen = 0u64;
        let mut on_dir = |_: &Path| {};
        walk(&base, 1, &mut out, &mut seen, &mut on_dir);

        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        // c 在 b 之前；a 是空；d 不是空；base 自身因 d 有文件也不算空
        let ci = names.iter().position(|n| *n == "c");
        let bi = names.iter().position(|n| *n == "b");
        assert!(ci.is_some() && bi.is_some(), "b、c 都应被识别为空目录：{names:?}");
        assert!(ci < bi, "子目录必须排在父目录之前：{names:?}");
        assert!(names.contains(&"a"));
        assert!(!names.contains(&"d"), "有文件的目录不得入选");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 删除只对空目录成功；扫描后被写入内容的目录必须删不掉
    #[test]
    fn 删除不会误删后来写入的内容() {
        let base = std::env::temp_dir().join("__dm_emptydir_del_test__");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("empty")).unwrap();
        std::fs::create_dir_all(base.join("raced")).unwrap();
        // 模拟「扫描后又被写入」
        std::fs::write(base.join("raced").join("new.txt"), b"data").unwrap();

        let (removed, failed) = delete(
            &[base.join("empty"), base.join("raced")],
            &mut |_, _| {},
        );
        assert_eq!(removed, 1);
        assert_eq!(failed, 1);
        assert!(!base.join("empty").exists());
        assert!(base.join("raced").join("new.txt").is_file(), "内容必须原样保留");

        let _ = std::fs::remove_dir_all(&base);
    }
}
