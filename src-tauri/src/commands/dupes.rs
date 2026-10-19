//! 重复文件查找。
//!
//! 三段式，越往后越贵，只有前一步没排除才继续：
//!   1. 按**大小**分组（读目录就能拿到，最便宜）—— 大小不同一定不是重复；
//!   2. **逐字节比对**（前 64KB 先比，内容不同立刻断开）；
//!   3. 全部相同才算一组重复。
//!
//! 为什么不用哈希：sha256 要额外依赖，而这里只需要判断"是否完全相同"。
//! 逐字节比对是精确的，且内容不同时通常在第一个 64KB 就断开，实际 IO 比全量哈希更省。
//!
//! 安全边界：
//! - 只读扫描，删除是另一个命令，且**绝不允许删除系统目录**；
//! - 每组至少保留一个（前端强制，后端再校验路径不在保护目录）。

use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 取消标志：与扫描/清理一致，全局原子量，命令置位、循环里检查
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 小于这个体积的文件不参与：1MB 以下的重复没有清理价值，只会让结果列表爆炸
const MIN_SIZE: u64 = 1024 * 1024;
/// 候选文件上限，防止在几百万文件的盘上跑半小时
const MAX_CANDIDATES: usize = 200_000;
const CHUNK: usize = 64 * 1024;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupFile {
    pub path: String,
    pub size: u64,
    /// 修改时间（Unix 秒），用于默认保留"最早创建的那份"
    pub mtime: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    pub size: u64,
    /// 这一组如果只保留一份，能省下的字节
    pub wasted: u64,
    pub files: Vec<DupFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupReport {
    pub groups: Vec<DupGroup>,
    pub scanned: u64,
    pub wasted: u64,
    pub cancelled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupProgress {
    pub phase: String,
    pub scanned: u64,
    pub groups: usize,
}

/// 永远不参与删除的目录（扫描时也默认跳过）。判断用大写前缀比较，
/// 避免大小写差异导致放行。
fn protected(p: &Path) -> bool {
    let mut s = p.to_string_lossy().replace('/', "\\").to_uppercase();
    if s.starts_with(r"\\?\") {
        s = s[4..].to_string();
    }
    const BAD: [&str; 6] = [
        r"\WINDOWS\",
        r"\PROGRAM FILES\",
        r"\PROGRAM FILES (X86)\",
        r"\PROGRAMDATA\MICROSOFT\",
        r"\$RECYCLE.BIN\",
        r"\SYSTEM VOLUME INFORMATION\",
    ];
    let padded = format!("{s}\\");
    BAD.iter().any(|b| padded.contains(b))
}

fn is_reparse(p: &Path) -> bool {
    p.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// 递归收集候选文件：只记路径与大小，不读内容
fn walk(dir: &Path, out: &mut Vec<(u64, PathBuf)>, scanned: &mut u64, skip_system: bool) -> bool {
    if CANCEL.load(Ordering::Relaxed) || out.len() >= MAX_CANDIDATES {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    for e in rd.flatten() {
        if CANCEL.load(Ordering::Relaxed) || out.len() >= MAX_CANDIDATES {
            return false;
        }
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if skip_system && protected(&p) {
                continue;
            }
            if !walk(&p, out, scanned, skip_system) {
                return false;
            }
        } else if ft.is_file() {
            *scanned += 1;
            if let Ok(md) = e.metadata() {
                if md.len() >= MIN_SIZE {
                    out.push((md.len(), p));
                }
            }
        }
    }
    true
}

/// 逐字节比对：大小已相同，先比 64KB，通常在这一步就断开
fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (File::open(a), File::open(b)) else { return false };
    let mut ba = vec![0u8; CHUNK];
    let mut bb = vec![0u8; CHUNK];
    loop {
        if CANCEL.load(Ordering::Relaxed) {
            return false;
        }
        let na = match fa.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let nb = match fb.read(&mut bb) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if na != nb {
            return false;
        }
        if na == 0 {
            return true;
        }
        if ba[..na] != bb[..nb] {
            return false;
        }
    }
}

fn mtime_secs(p: &Path) -> i64 {
    p.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn cancel_dupes() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// 扫描重复文件。`on_progress` 只在阶段切换与每 2000 个文件时回调，避免刷爆 IPC。
#[tauri::command]
pub async fn find_duplicates(
    roots: Vec<String>,
    skip_system: bool,
    on_progress: tauri::ipc::Channel<DupProgress>,
) -> Result<DupReport, String> {
    CANCEL.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || scan_impl(roots, skip_system, |p| {
        let _ = on_progress.send(p);
    }))
    .await
    .map_err(|e| e.to_string())?
}

fn scan_impl(
    roots: Vec<String>,
    skip_system: bool,
    mut progress: impl FnMut(DupProgress),
) -> Result<DupReport, String> {
    let mut cand: Vec<(u64, PathBuf)> = Vec::new();
    let mut scanned: u64 = 0;
    progress(DupProgress { phase: "正在列出文件…".into(), scanned: 0, groups: 0 });
    for r in &roots {
        let p = PathBuf::from(r);
        if !p.is_dir() {
            continue;
        }
        if skip_system && protected(&p) {
            continue;
        }
        if !walk(&p, &mut cand, &mut scanned, skip_system) {
            break;
        }
        progress(DupProgress {
            phase: "正在列出文件…".into(),
            scanned,
            groups: 0,
        });
    }
    if CANCEL.load(Ordering::Relaxed) {
        return Ok(DupReport { groups: vec![], scanned, wasted: 0, cancelled: true });
    }

    // 1) 按大小分组
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for (n, p) in cand {
        by_size.entry(n).or_default().push(p);
    }

    // 2) 只比大小相同的，且组内至少两个
    let mut groups: Vec<DupGroup> = Vec::new();
    let mut wasted: u64 = 0;
    let mut checked: u64 = 0;
    let total_groups = by_size.values().filter(|v| v.len() > 1).count() as u64;
    let mut done_groups = 0u64;

    for (size, paths) in by_size.into_iter() {
        if CANCEL.load(Ordering::Relaxed) {
            break;
        }
        if paths.len() < 2 {
            continue;
        }
        done_groups += 1;
        if done_groups % 20 == 0 || done_groups == total_groups {
            progress(DupProgress {
                phase: format!("正在比对内容…（{done_groups}/{total_groups} 组）"),
                scanned,
                groups: groups.len(),
            });
        }

        // 同大小内部再聚类：每个簇用第一个文件当代表
        let mut clusters: Vec<Vec<PathBuf>> = Vec::new();
        for p in paths {
            checked += 1;
            if CANCEL.load(Ordering::Relaxed) {
                break;
            }
            let mut placed = false;
            for c in clusters.iter_mut() {
                if same_content(&c[0], &p) {
                    c.push(p.clone());
                    placed = true;
                    break;
                }
            }
            if !placed {
                clusters.push(vec![p]);
            }
        }
        for mut c in clusters {
            if c.len() < 2 {
                continue;
            }
            // 默认保留"最早的那份"：靠前的就是原始文件，后拷的往往是副本
            c.sort_by_key(|p| mtime_secs(p));
            let files: Vec<DupFile> = c
                .iter()
                .map(|p| DupFile {
                    path: p.to_string_lossy().to_string(),
                    size,
                    mtime: mtime_secs(p),
                })
                .collect();
            let w = size * (files.len() as u64 - 1);
            wasted += w;
            groups.push(DupGroup { size, wasted: w, files });
        }
    }

    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));
    let cancelled = CANCEL.load(Ordering::Relaxed);
    progress(DupProgress { phase: "完成".into(), scanned, groups: groups.len() });
    let _ = checked;
    Ok(DupReport { groups, scanned, wasted, cancelled })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupDeleteReport {
    pub freed: u64,
    pub failed: usize,
    pub pending_reboot: usize,
    pub refused: Vec<String>,
}

/// 删除选中的重复文件。
///
/// 后端只知道"要删这些路径"，不知道分组，所以**每组保留一份**由前端保证；
/// 这里兜住底线：只删普通文件、不碰系统目录、不存在视为已删。
#[tauri::command]
pub async fn delete_duplicates(paths: Vec<String>) -> Result<DupDeleteReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut freed = 0u64;
        let mut failed = 0usize;
        let mut pending = 0usize;
        let mut refused = Vec::new();
        for s in paths {
            let p = PathBuf::from(&s);
            if !p.is_absolute() || protected(&p) || is_reparse(&p) {
                refused.push(s);
                continue;
            }
            match p.metadata() {
                Ok(m) if m.is_file() => {}
                Ok(_) => {
                    refused.push(s);
                    continue;
                }
                Err(_) => continue, // 已经不在了，算成功
            }
            let r = super::fsx::remove_file(&p);
            if r.ok {
                freed += r.freed;
                if r.pending_reboot {
                    pending += 1;
                }
            } else {
                failed += 1;
            }
        }
        Ok(DupDeleteReport { freed, failed, pending_reboot: pending, refused })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dm-dupes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, body: &[u8]) {
        let mut f = File::create(p).unwrap();
        f.write_all(body).unwrap();
    }

    #[test]
    fn 相同内容才归为一组() {
        let d = tmpdir("same");
        let a = d.join("a.bin");
        let b = d.join("b.bin");
        let c = d.join("c.bin");
        let body = vec![7u8; (MIN_SIZE + 10) as usize];
        write(&a, &body);
        write(&b, &body);
        // 同大小、最后一个字节不同 → 不能算重复
        let mut other = body.clone();
        *other.last_mut().unwrap() = 9;
        write(&c, &other);

        let rep = scan_impl(vec![d.to_string_lossy().to_string()], false, |_| {}).unwrap();
        assert_eq!(rep.groups.len(), 1, "只应有一组重复");
        assert_eq!(rep.groups[0].files.len(), 2);
        assert_eq!(rep.groups[0].wasted, MIN_SIZE + 10);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 小于一兆的文件不参与() {
        let d = tmpdir("small");
        write(&d.join("a.txt"), b"hello");
        write(&d.join("b.txt"), b"hello");
        let rep = scan_impl(vec![d.to_string_lossy().to_string()], false, |_| {}).unwrap();
        assert!(rep.groups.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 系统目录永远被拒绝删除() {
        // 只验证判定函数，不真的删任何东西
        assert!(protected(Path::new(r"C:\Windows\System32\a.dll")));
        assert!(protected(Path::new(r"C:\Program Files\X\a.dll")));
        assert!(protected(Path::new(r"\\?\C:\Windows\a.bin")));
        assert!(!protected(Path::new(r"D:\Downloads\a.bin")));
    }
}
