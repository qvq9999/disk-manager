//! 目录树扫描（深度限制 + junction 防环 + 超大目录聚合）
//!
//! 支持「中途停止」与「后台进度查询」：
//! - 扫描线程在若干检查点读取全局取消标志，命中即放弃本次结果并返回 [`SCAN_CANCELLED`]
//! - 进度（已遍历条目数 / 当前目录）写进全局原子量，前端轮询 [`scan_status`] 即可
//!   —— 不用事件推送，避免高频 emit 拖慢遍历本身

use serde::Serialize;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

/// 扫描被用户中止的固定错误码（前端据此区分「停止」与「真失败」）
pub const SCAN_CANCELLED: &str = "SCAN_CANCELLED";

/// 取消请求标志。扫描是全局串行的（见 SCAN_LOCK），因此单个全局标志即可
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 是否有扫描任务正在运行（含刚启动尚未遍历的短暂窗口）
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// 已遍历的目录项总数（进度用）
static ENTRIES: AtomicU64 = AtomicU64::new(0);
/// 已进入的目录数（用于节流更新「当前目录」）
static DIRS: AtomicU64 = AtomicU64::new(0);
/// 当前所在目录（仅在内存中保留一份，避免每条目都分配字符串）
static CURRENT: Mutex<String> = Mutex::new(String::new());

/// 每遍历多少个目录项检查一次取消标志（256 次 fetch_add 约在微秒量级，
/// 既能做到「点停止基本立刻停」，又不会让原子操作成为遍历瓶颈）
const CANCEL_CHECK_MASK: u64 = 0xFF;
/// 每进入多少个目录刷新一次「当前目录」文案
const CURRENT_EVERY: u64 = 24;

/// 记一次遍历步进；返回 true 表示用户已请求停止
#[inline]
fn tick() -> bool {
    let n = ENTRIES.fetch_add(1, Ordering::Relaxed);
    if n & CANCEL_CHECK_MASK == CANCEL_CHECK_MASK {
        CANCEL.load(Ordering::Relaxed)
    } else {
        false
    }
}

#[inline]
fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// 节流地把「当前在哪」写给前端看
fn note_dir(p: &Path) {
    let n = DIRS.fetch_add(1, Ordering::Relaxed);
    if n % CURRENT_EVERY == 0 {
        if let Ok(mut g) = CURRENT.lock() {
            *g = p.to_string_lossy().into_owned();
        }
    }
}

/// 子树完整体积（walkdir 单遍，不跟随链接）；被中止时返回 None
fn dir_size_full(p: &Path) -> Option<u64> {
    let mut sum: u64 = 0;
    for e in WalkDir::new(p)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if tick() {
            return None;
        }
        if let Ok(m) = e.metadata() {
            if m.is_file() {
                sum += m.len();
            }
        }
    }
    Some(sum)
}

const MAX_DEPTH: usize = 6; // 展示深度上限（体积始终算满，参考 WinDirStat/WizTree：计算不设限、展示才设限）
const MAX_CHILDREN: usize = 200; // 每层最多展示的子项，其余聚合
const REPARSE_POINT: u32 = 0x0400;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub kind: String, // dir | file
    pub size: u64,
    pub category: String,
    pub mtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileNode>>,
}

fn categorize(name: &str, is_dir: bool) -> &'static str {
    let n = name.to_lowercase();
    const VIDEO: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts", "m4v"];
    const IMAGE: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "raw"];
    const AUDIO: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a", "wma"];
    const DOC: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv"];
    const ARCHIVE: &[&str] = &["zip", "rar", "7z", "tar", "gz", "iso", "cab"];
    const APP: &[&str] = &["exe", "dll", "msi", "appx", "msix", "jar", "lnk", "bat", "cmd"];
    if !is_dir {
        if n.starts_with("pagefile") || n.starts_with("hiberfil") || n.starts_with("swapfile") {
            return "system";
        }
        let ext = n.rsplit('.').next().unwrap_or("");
        if VIDEO.contains(&ext) { return "video"; }
        if IMAGE.contains(&ext) { return "image"; }
        if AUDIO.contains(&ext) { return "audio"; }
        if DOC.contains(&ext) { return "doc"; }
        if ARCHIVE.contains(&ext) { return "archive"; }
        if APP.contains(&ext) { return "app"; }
        if ext == "sys" { return "system"; }
        return "other";
    }
    if n == "$recycle.bin" || n == "system volume information" || n == "windows" || n == "winboot" {
        return "system";
    }
    "other"
}

/// 递归构建目录树；用户中止时返回 None（由上层一路冒泡，整份结果作废）
fn build_node(path: &Path, name: String, depth: usize) -> Option<FileNode> {
    if cancelled() {
        return None;
    }
    note_dir(path);
    let meta = fs::symlink_metadata(path);
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);

    // 目录到达展示深度上限：体积仍完整统计（一次 walkdir 走完子树），
    // 作为带真实大小的叶子展示 —— 保证总量与 Windows 报告值一致
    if is_dir && depth >= MAX_DEPTH {
        let category = categorize(&name, true);
        return Some(FileNode {
            name,
            kind: "dir".into(),
            size: dir_size_full(path)?,
            category: category.into(),
            mtime: meta.ok().and_then(|m| m.modified().ok())
                .map(|t| fmt_time(&t))
                .unwrap_or_default(),
            children: None,
        });
    }
    if !is_dir {
        let m = meta.ok();
        let category = categorize(&name, false);
        return Some(FileNode {
            name,
            kind: "file".into(),
            size: m.as_ref().map(|m| m.len()).unwrap_or(0),
            category: category.into(),
            mtime: m.and_then(|m| m.modified().ok())
                .map(|t| fmt_time(&t))
                .unwrap_or_default(),
            children: None,
        });
    }
    let mut dirs: Vec<(PathBuf, String)> = Vec::new();
    let mut files: Vec<FileNode> = Vec::new();
    if let Ok(rd) = fs::read_dir(path) {
        for entry in rd.flatten() {
            if tick() {
                return None;
            }
            let child_name = entry.file_name().to_string_lossy().into_owned();
            let attrs = entry.metadata().map(|m| m.file_attributes()).unwrap_or(0);
            if attrs & REPARSE_POINT != 0 {
                continue; // 跳过 junction/符号链接，防环
            }
            let child_path = entry.path();
            if child_path.is_dir() {
                dirs.push((child_path, child_name));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(FileNode {
                    size,
                    name: child_name.clone(),
                    kind: "file".into(),
                    category: categorize(&child_name, false).into(),
                    mtime: entry.metadata().ok().and_then(|m| m.modified().ok())
                        .map(|t| fmt_time(&t)).unwrap_or_default(),
                    children: None,
                });
            }
        }
    }
    let mut children: Vec<FileNode> = Vec::new();
    for (cp, cn) in dirs {
        children.push(build_node(&cp, cn, depth + 1)?);
    }
    children.extend(files);
    children.sort_by(|a, b| b.size.cmp(&a.size));
    // 超出上限的子项聚合为一条
    if children.len() > MAX_CHILDREN {
        let rest: Vec<FileNode> = children.split_off(MAX_CHILDREN - 1);
        children.push(FileNode {
            name: format!("（其余 {} 项）", rest.len()),
            kind: "dir".into(),
            size: rest.iter().map(|n| n.size).sum(),
            category: "other".into(),
            mtime: String::new(),
            children: None,
        });
    }
    let size: u64 = children.iter().map(|n| n.size).sum();
    let category = categorize(&name, true);
    Some(FileNode {
        name,
        kind: "dir".into(),
        size,
        category: category.into(),
        mtime: meta.ok().and_then(|m| m.modified().ok())
            .map(|t| fmt_time(&t)).unwrap_or_default(),
        children: Some(children),
    })
}

fn fmt_time(t: &SystemTime) -> String {    // 简易格式化：仅日期（避免引入 chrono）
    let d = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = d / 86400;
    // 1970-01-01 起算（忽略闰秒，误差可接受）
    let (y, m, dd) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{dd:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// 全局串行锁：同一时刻只允许一个全盘扫描任务，避免反复切页触发并发遍历把磁盘打满
static SCAN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// ACTIVE 标志的守卫：无论正常结束、出错、panic 还是中止，都会复位
struct ActiveGuard;
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

/// 扫描进度快照（前端 400~600ms 轮询一次，用于「后台进行中」的任务条）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// 是否有扫描在跑（前端据此判断「后台仍在加载」）
    pub active: bool,
    /// 已遍历的目录项数量（单调递增，可作为进度参考）
    pub entries: u64,
    /// 当前正在遍历的目录（取整条路径，便于用户判断还要多久）
    pub current: String,
}

#[tauri::command]
pub fn scan_status() -> ScanStatus {
    ScanStatus {
        active: ACTIVE.load(Ordering::Acquire),
        entries: ENTRIES.load(Ordering::Relaxed),
        current: CURRENT.lock().map(|g| g.clone()).unwrap_or_default(),
    }
}

/// 请求停止正在进行的扫描。
/// 扫描线程会在下一个检查点退出并返回 [`SCAN_CANCELLED`]；用户得到的是一份「已停止」而非错误。
#[tauri::command]
pub fn cancel_scan() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 复位进度与取消标志。⚠️ 必须在持有 SCAN_LOCK 之后调用：
/// 只有拿到锁才说明上一个扫描已经退出，此时清标志不会误伤它。
fn reset_progress() {
    CANCEL.store(false, Ordering::SeqCst);
    ENTRIES.store(0, Ordering::Relaxed);
    DIRS.store(0, Ordering::Relaxed);
    if let Ok(mut g) = CURRENT.lock() {
        g.clear();
    }
}

#[tauri::command]
pub async fn scan_drive(letter: String) -> Result<FileNode, String> {
    // 后台线程执行 + 串行化 + panic 兜底，确保异常不会影响其他功能
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = SCAN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_progress();
        ACTIVE.store(true, Ordering::Release);
        let _active = ActiveGuard;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan_drive_impl(letter)))
            .unwrap_or_else(|_| Err("扫描过程出现异常，已安全中止".into()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("扫描任务失败: {e}")))
}

fn scan_drive_impl(letter: String) -> Result<FileNode, String> {
    let root = format!("{}:\\", letter.to_uppercase());
    let p = PathBuf::from(&root);
    if !p.is_dir() {
        return Err(format!("盘符 {root} 不存在或不可访问"));
    }
    // 被中止时整份结果作废：残缺的目录树会给出错误的体积占比，比不给结果更糟
    build_node(&p, root, 0).ok_or_else(|| SCAN_CANCELLED.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 已置取消标志时，遍历入口应立即放弃（不产生半成品结果）
    #[test]
    fn cancel_flag_rejects_immediately() {
        reset_progress();
        CANCEL.store(true, Ordering::SeqCst);
        let got = build_node(Path::new("C:\\"), "C:\\".into(), 0);
        assert!(got.is_none(), "已取消时不应返回任何结果");
        reset_progress();
    }

    /// 实机验证「中途停止」是否真的生效（含响应延迟）。
    ///
    /// 默认跳过，避免依赖开发机环境；需要真跑时指定一个足够大的真实目录：
    ///   DM_SCAN_TEST_ROOT='C:\' DM_SCAN_TEST_WAIT_MS=120 cargo test cancel_midway -- --nocapture
    #[test]
    fn cancel_midway_stops_traversal() {
        let Ok(root) = std::env::var("DM_SCAN_TEST_ROOT") else {
            eprintln!("skip: 设置 DM_SCAN_TEST_ROOT 指向一个足够大的真实目录后再跑");
            return;
        };
        let wait_ms = std::env::var("DM_SCAN_TEST_WAIT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(120);
        let p = PathBuf::from(&root);
        assert!(p.is_dir(), "DM_SCAN_TEST_ROOT 必须是一个存在的目录");

        reset_progress();
        ACTIVE.store(true, Ordering::Release);
        let h = std::thread::spawn(move || build_node(&p, "root".to_string(), 0).is_some());

        std::thread::sleep(Duration::from_millis(wait_ms));
        let at_cancel = ENTRIES.load(Ordering::Relaxed);
        let cancel_at = Instant::now();
        CANCEL.store(true, Ordering::SeqCst);

        let produced = h.join().unwrap();
        let latency = cancel_at.elapsed();
        let at_end = ENTRIES.load(Ordering::Relaxed);
        let overshoot = at_end.saturating_sub(at_cancel);

        eprintln!(
            "取消点: 已遍历 {at_cancel} 项 | 停止延迟: {:?} | 取消后多遍历: {overshoot} 项 | 是否产出结果: {produced}",
            latency
        );

        assert!(!produced, "取消后不应产出结果（半成品目录树会误导体积占比）");
        assert!(at_cancel > 0, "取消点之前应有遍历进展，否则测试没有意义");
        assert!(
            latency < Duration::from_millis(800),
            "停止响应过慢: {:?}（检查点粒度应是毫秒级）",
            latency
        );
        assert!(overshoot < 5_000, "取消后仍遍历了 {overshoot} 项，检查点过粗");
        ACTIVE.store(false, Ordering::Release);
        reset_progress();
    }
}
