//! 目录树扫描（深度限制 + junction 防环 + 超大目录聚合）
//!
//! 支持「中途停止」与「后台进度查询」：
//! - 扫描线程在若干检查点读取全局取消标志，命中即放弃本次结果并返回 [`SCAN_CANCELLED`]
//! - 进度（已遍历条目数 / 当前目录）写进全局原子量，前端轮询 [`scan_status`] 即可
//!   —— 不用事件推送，避免高频 emit 拖慢遍历本身

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

/// 扫描被用户中止的固定错误码（前端据此区分「停止」与「真失败」）
pub const SCAN_CANCELLED: &str = "SCAN_CANCELLED";

/// 取消请求标志。扫描是全局串行的（见 SCAN_LOCK），因此单个全局标志即可
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 是否有扫描任务正在运行（含刚启动尚未遍历的短暂窗口）
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// 已遍历的目录项总数（进度用）
pub(crate) static ENTRIES: AtomicU64 = AtomicU64::new(0);
/// 已进入的目录数（用于节流更新「当前目录」）
static DIRS: AtomicU64 = AtomicU64::new(0);
/// 当前所在目录（仅在内存中保留一份，避免每条目都分配字符串）
static CURRENT: Mutex<String> = Mutex::new(String::new());
/// 当前正在遍历的盘符。
///
/// 后端扫描是串行的（SCAN_LOCK），但前端可能同时存在多个「扫描中」的任务
/// （比如启动预热扫 C 盘时用户又点了 D 盘）。没有这个字段，D 盘会显示 C 盘的进度，
/// 用户看到的就是「数字在涨、但涨的不是我要的盘」。
static SCANNING_LETTER: Mutex<String> = Mutex::new(String::new());

/// 每进入多少个目录刷新一次「当前目录」文案
const CURRENT_EVERY: u64 = 24;
/// 每个线程本地攒够多少条才写一次全局计数
const FLUSH_EVERY: u64 = 64;

thread_local! {
    /// 线程本地条目计数。
    ///
    /// 为什么不能每项直接 `ENTRIES.fetch_add(1)`：单线程时它几乎免费，
    /// 但多线程下所有核心都在抢同一根缓存行，每次约 50~100ns，
    /// 40 万项就是一秒以上 —— 并行收益会被这个计数器吃掉大半（实测 8 线程只有 1.7 倍）。
    /// 改成本地累加、每 64 项合并一次后，争抢次数直接降到 1/64。
    static LOCAL_ENTRIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// 记一次遍历步进；返回 true 表示用户已请求停止
#[inline]
fn tick() -> bool {
    let n = LOCAL_ENTRIES.with(|c| {
        let v = c.get() + 1;
        c.set(v);
        v
    });
    if n % FLUSH_EVERY == 0 {
        ENTRIES.fetch_add(FLUSH_EVERY, Ordering::Relaxed);
        // 顺带在这个频率上检查取消：64 项一次，比原来的 256 项响应更快
        return CANCEL.load(Ordering::Relaxed);
    }
    false
}

#[inline]
pub(crate) fn cancelled() -> bool {
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

// ============ 并行上下文 ============
//
// 并行只对 NTFS 开：NTFS 的目录项集中在 MFT，多线程读能吃到并发；
// FAT32/exFAT 没有这种结构，多线程只会把顺序读变成随机读。

/// 同时在跑的遍历线程上限（含主线程）
struct ScanCtx {
    live: AtomicUsize,
    max: usize,
}

/// 进入并行区的凭证，drop 时归还额度。
///
/// 用 RAII 而不是手动 leave：`build_node` 里有好几个 `?` 提前返回点，
/// 漏掉任何一个都会让额度永久少一个，并行会慢慢退化成串行 —— 而且这种泄漏
/// 单看代码很难发现，必须靠类型系统兜住。
struct EnterGuard<'a>(&'a ScanCtx);

impl Drop for EnterGuard<'_> {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ScanCtx {
    fn new(max: usize) -> Self {
        Self {
            live: AtomicUsize::new(1),
            max: max.max(1),
        }
    }

    /// 还有额度就占一个；没有则返回 None，调用方退回串行
    fn enter(&self) -> Option<EnterGuard<'_>> {
        let mut cur = self.live.load(Ordering::Relaxed);
        loop {
            if cur >= self.max {
                return None;
            }
            match self.live.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(EnterGuard(self)),
                Err(v) => cur = v,
            }
        }
    }
}

fn parallel_width() -> usize {
    let n = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4);
    // 上限 8：再往上收益递减，对机械盘也不友好
    n.clamp(1, 8)
}

/// 只有 NTFS 才并行遍历
fn is_ntfs(root: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let mut fs_buf = [0u16; 16];
        let ok = unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            )
        };
        if ok == 0 {
            return false;
        }
        let len = fs_buf.iter().position(|&c| c == 0).unwrap_or(fs_buf.len());
        String::from_utf16_lossy(&fs_buf[..len]).eq_ignore_ascii_case("NTFS")
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        false
    }
}

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
///
/// `sc` / `ctx` 用于并行：每个目录节点都可能把一半子目录派给新线程，
/// 并发额度由 `ctx` 统一控制，超过上限就退回串行 —— 不做无限线程爆炸。
fn build_node<'s>(
    sc: &'s std::thread::Scope<'s, '_>,
    ctx: &'s ScanCtx,
    path: &Path,
    name: String,
    depth: usize,
) -> Option<FileNode> {
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
            // ⚠️ metadata() 只调一次就够：属性、是不是目录、大小、修改时间都在同一份
            // WIN32_FILE_ATTRIBUTE_DATA 里。之前这里调了四次（属性 / is_dir / size / mtime），
            // 等于每个文件多跑三次系统调用 —— 40 万项就是 120 万次白干。
            let md = entry.metadata().ok();
            let attrs = md.as_ref().map(|m| m.file_attributes()).unwrap_or(0);
            if attrs & REPARSE_POINT != 0 {
                continue; // 跳过 junction/符号链接，防环
            }
            let is_child_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            if is_child_dir {
                dirs.push((entry.path(), child_name));
            } else {
                let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
                let mtime = md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(|t| fmt_time(&t))
                    .unwrap_or_default();
                let category = categorize(&child_name, false);
                files.push(FileNode {
                    size,
                    name: child_name,
                    kind: "file".into(),
                    category: category.into(),
                    mtime,
                    children: None,
                });
            }
        }
    }

    // ---- 子目录：够条件就分一半给新线程 ----
    //
    // 为什么按「递归分片」而不是「只在根目录并行」：C 盘的时间大头往往落在
    // 一两个巨型子树里（Windows、Users），只按顶层分派会变成「一个线程干一半活、
    // 其余线程早早收工」，加速比受最大子树限制。递归分片能把负载摊平。
    let mut children: Vec<FileNode> = Vec::new();
    let parallel = ctx.max > 1 && dirs.len() >= 2;
    let mut handled = false;
    if parallel {
        if let Some(_g) = ctx.enter() {
            handled = true;
            let tail = dirs.split_off(dirs.len() / 2);
            let handle = sc.spawn(move || -> Option<Vec<FileNode>> {
                let mut out = Vec::with_capacity(tail.len());
                for (p, n) in tail {
                    out.push(build_node(sc, ctx, &p, n, depth + 1)?);
                }
                Some(out)
            });
            for (cp, cn) in dirs.drain(..) {
                children.push(build_node(sc, ctx, &cp, cn, depth + 1)?);
            }
            match handle.join() {
                Ok(Some(mut v)) => children.append(&mut v),
                // 子线程被取消或 panic：整份结果作废（半棵树会给出错误的占比）
                _ => return None,
            }
        }
    }
    if !handled {
        for (cp, cn) in dirs.drain(..) {
            children.push(build_node(sc, ctx, &cp, cn, depth + 1)?);
        }
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
    /// 当前真正在遍历的盘符（前端据此区分「我在扫」与「我在排队」）
    pub letter: String,
}

#[tauri::command]
pub fn scan_status() -> ScanStatus {
    ScanStatus {
        active: ACTIVE.load(Ordering::Acquire),
        entries: ENTRIES.load(Ordering::Relaxed),
        current: CURRENT.lock().map(|g| g.clone()).unwrap_or_default(),
        letter: SCANNING_LETTER.lock().map(|g| g.clone()).unwrap_or_default(),
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
        if let Ok(mut g) = SCANNING_LETTER.lock() {
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
    if let Ok(mut g) = SCANNING_LETTER.lock() {
        *g = letter.to_uppercase();
    }

    // 先走 $MFT：顺序读一遍元文件就能拿到全盘清单，比逐目录遍历快一个数量级。
    // 失败最常见的原因是「非管理员」或「非 NTFS」，此时静默回退到遍历，功能不受影响。
    match scan_via_mft(&letter) {
        Ok(tree) => return Ok(tree),
        Err(_) => { /* 回退到遍历 */ }
    }

    let width = if is_ntfs(&root) { parallel_width() } else { 1 };
    let ctx = ScanCtx::new(width);
    // 被中止时整份结果作废：残缺的目录树会给出错误的体积占比，比不给结果更糟。
    // scope 结束时会等待所有派发出去的线程收尾（它们也都在检查取消标志，不会久等）。
    std::thread::scope(|sc| build_node(sc, &ctx, &p, root, 0))
        .ok_or_else(|| SCAN_CANCELLED.to_string())
}

// ============ MFT 快速路径 ============

/// 用 $MFT 枚举整盘并重建目录树。
///
/// 体积口径与遍历保持一致：文件取逻辑大小（$DATA 的 RealSize），目录自底向上累加；
/// 硬链接按路径展开重复计算（和 `read_dir` 看到的一样）。
fn scan_via_mft(letter: &str) -> Result<FileNode, String> {
    let entries = super::mft::read_entries(letter)?;
    // 读取中途被取消时 MFT 只读了一半，绝不能拿半份数据建树
    if cancelled() {
        return Err(SCAN_CANCELLED.to_string());
    }
    ENTRIES.store(entries.len() as u64, Ordering::Relaxed);

    let mut by_parent: HashMap<u64, Vec<usize>> = HashMap::with_capacity(entries.len() / 4);
    for (i, e) in entries.iter().enumerate() {
        by_parent.entry(e.parent).or_default().push(i);
    }

    let root_name = format!("{}:\\", letter.trim_end_matches(':').to_uppercase());
    let mut visited = 0usize;
    let (node, _) = mft_build(
        super::mft::root_id(),
        root_name,
        0,
        &entries,
        &by_parent,
        &mut visited,
    )
    .ok_or_else(|| SCAN_CANCELLED.to_string())?;

    // 合理性自检：MFT 解析一旦偏移写错，算出来的总量会离谱到几个数量级。
    // 用「卷已用字节数」当尺子，落在合理区间才采信，否则退回遍历。
    //
    // 区间放宽到 0.5~1.5 倍是有原因的，两个口径本来就不完全等价：
    // 小文件按簇分配、压缩/稀疏文件的逻辑大小与实际占用不同、硬链接按路径重复计数。
    if let Some((used, _total)) = super::mft::volume_usage(letter) {
        if used > 0 {
            let ratio = node.size as f64 / used as f64;
            if !(0.5..=1.5).contains(&ratio) {
                return Err(format!(
                    "MFT 结果与卷使用量差异过大（{ratio:.2} 倍），已回退到遍历"
                ));
            }
        }
    }

    Ok(node)
}

/// 递归建树。返回 (节点, 该子树总体积)。
///
/// 到展示深度上限时不再展开，但体积仍按整棵子树算满 —— 与遍历路径的口径一致。
fn mft_build(
    id: u64,
    name: String,
    depth: usize,
    entries: &[super::mft::MftEntry],
    by_parent: &HashMap<u64, Vec<usize>>,
    visited: &mut usize,
) -> Option<(FileNode, u64)> {
    let mut children: Vec<FileNode> = Vec::new();
    let mut total: u64 = 0;

    if let Some(list) = by_parent.get(&id) {
        for &i in list {
            *visited += 1;
            if *visited % 256 == 0 && cancelled() {
                return None;
            }
            let e = &entries[i];
            if e.is_dir {
                if depth + 1 >= MAX_DEPTH {
                    let sz = mft_subtree_size(e.id, entries, by_parent, 0);
                    total += sz;
                    children.push(FileNode {
                        name: e.name.clone(),
                        kind: "dir".into(),
                        size: sz,
                        category: categorize(&e.name, true).into(),
                        mtime: fmt_unix(e.mtime),
                        children: None,
                    });
                } else {
                    let (node, sz) = mft_build(e.id, e.name.clone(), depth + 1, entries, by_parent, visited)?;
                    total += sz;
                    children.push(node);
                }
            } else {
                total += e.size;
                children.push(FileNode {
                    name: e.name.clone(),
                    kind: "file".into(),
                    size: e.size,
                    category: categorize(&e.name, false).into(),
                    mtime: fmt_unix(e.mtime),
                    children: None,
                });
            }
        }
    }

    children.sort_by(|a, b| b.size.cmp(&a.size));
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

    Some((
        FileNode {
            category: categorize(&name, true).into(),
            name,
            kind: "dir".into(),
            size: total,
            mtime: String::new(),
            children: Some(children),
        },
        total,
    ))
}

/// 只算体积、不建节点（给深度上限处的兄弟节点用）。
/// `depth` 兼作防环保护：MFT 理应无环，但脏数据不该让进程爆栈。
fn mft_subtree_size(
    id: u64,
    entries: &[super::mft::MftEntry],
    by_parent: &HashMap<u64, Vec<usize>>,
    depth: usize,
) -> u64 {
    if depth > 64 {
        return 0;
    }
    let Some(list) = by_parent.get(&id) else { return 0 };
    let mut total = 0u64;
    for &i in list {
        let e = &entries[i];
        if e.is_dir {
            total += mft_subtree_size(e.id, entries, by_parent, depth + 1);
        } else {
            total += e.size;
        }
    }
    total
}

fn fmt_unix(secs: u64) -> String {
    fmt_time(&(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 测试用扫描入口：把 scope + ctx 包起来，避免每个用例重复写样板
    fn scan_root(width: usize, p: &Path, label: &str) -> Option<FileNode> {
        let ctx = ScanCtx::new(width);
        std::thread::scope(|sc| build_node(sc, &ctx, p, label.to_string(), 0))
    }

    /// 已置取消标志时，遍历入口应立即放弃（不产生半成品结果）
    #[test]
    fn cancel_flag_rejects_immediately() {
        reset_progress();
        CANCEL.store(true, Ordering::SeqCst);
        let got = scan_root(1, Path::new("C:\\"), "C:\\");
        assert!(got.is_none(), "已取消时不应返回任何结果");
        reset_progress();
    }

    #[test]
    fn ctx_respects_limit() {
        let ctx = ScanCtx::new(2); // live 初始为 1（主线程），还能再进 1 个
        let g1 = ctx.enter();
        assert!(g1.is_some(), "还有额度时必须允许进入");
        assert!(ctx.enter().is_none(), "额度用满后必须拒绝，不能无限开线程");
        drop(g1);
        assert!(ctx.enter().is_some(), "归还后应可以再次进入");
    }

    /// 并行结果必须与串行完全一致。
    ///
    /// 并行遍历最容易出的错不是慢，而是**漏项或多算**：某个子目录被派出去之后
    /// 主线程又数了一遍，或者 join 失败被静默吞掉。体积一旦算错，
    /// 树图的占比就是错的，而用户完全看不出来。
    #[test]
    fn parallel_result_matches_serial() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("scan-parity-test");
        let _ = fs::remove_dir_all(&base);

        // 3 层 × 每层 4 个子目录 × 每个目录 2 个 1KB 文件
        fn build(p: &Path, depth: usize) {
            fs::create_dir_all(p).unwrap();
            for i in 0..2 {
                fs::write(p.join(format!("f{i}.txt")), vec![b'x'; 1024]).unwrap();
            }
            if depth == 0 {
                return;
            }
            for i in 0..4 {
                build(&p.join(format!("d{i}")), depth - 1);
            }
        }
        build(&base, 2);

        reset_progress();
        let serial = scan_root(1, &base, "root").expect("串行扫描应成功");
        reset_progress();
        let parallel = scan_root(4, &base, "root").expect("并行扫描应成功");

        fn count(n: &FileNode) -> (usize, usize) {
            let mut dirs = usize::from(n.kind == "dir");
            let mut files = usize::from(n.kind == "file");
            if let Some(cs) = &n.children {
                for c in cs {
                    let (d, f) = count(c);
                    dirs += d;
                    files += f;
                }
            }
            (dirs, files)
        }

        // 目录：1 个根 + 4 个子 + 16 个孙 = 21；文件：每个目录 2 个 = 42
        assert_eq!(count(&serial), (21, 42), "测试树结构应固定");
        assert_eq!(count(&parallel), count(&serial), "并行不能漏项或多算");
        assert_eq!(serial.size, 21 * 2 * 1024, "串行总体积");
        assert_eq!(parallel.size, serial.size, "并行与串行的总体积必须一致");

        let _ = fs::remove_dir_all(&base);
        reset_progress();
    }

    /// 并行到底快多少：分别用 1/2/4/8 线程扫同一棵树。
    ///
    /// 手动跑：
    ///   cargo test --release --lib bench_scan -- --ignored --nocapture
    /// 对真实目录跑（更接近实际）：
    ///   DM_SCAN_BENCH_ROOT='C:\Windows' cargo test --release --lib bench_scan -- --ignored --nocapture
    #[test]
    #[ignore = "会创建几千个临时文件 / 或需要指定真实目录"]
    fn bench_scan() {
        if let Ok(root) = std::env::var("DM_SCAN_BENCH_ROOT") {
            let p = PathBuf::from(&root);
            assert!(p.is_dir(), "DM_SCAN_BENCH_ROOT 必须是一个存在的目录");
            // 真实盘在扫描期间本来就有文件增删（缓存、日志、pagefile），
            // 体积严格相等不成立，只打印偏差供人判断
            bench_on(&p, false);
            return;
        }

        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("scan-bench");
        let _ = fs::remove_dir_all(&base);

        // 300 个目录 × 20 文件 + 60 个二级目录 × 20 文件
        for d in 0..300 {
            let p = base.join(format!("d{d}"));
            fs::create_dir_all(&p).unwrap();
            for f in 0..20 {
                fs::write(p.join(format!("f{f}.dat")), [0u8; 512]).unwrap();
            }
        }
        for d in 0..60 {
            let p = base.join(format!("d{d}")).join("sub");
            fs::create_dir_all(&p).unwrap();
            for f in 0..20 {
                fs::write(p.join(format!("s{f}.dat")), [0u8; 512]).unwrap();
            }
        }

        bench_on(&base, true);
        let _ = fs::remove_dir_all(&base);
    }

    /// 对同一棵树按不同线程上限各跑一次，打印耗时与加速比。 每个宽度前先热身一次，尽量让比较落在「缓存已热」的同一条件下。
    ///
    /// `strict` 为真时要求各次扫描体积完全相等 —— 只对静态的合成树成立。
    fn bench_on(p: &Path, strict: bool) {
        let widths: Vec<usize> = std::env::var("DM_SCAN_BENCH_WIDTHS")
            .ok()
            .map(|s| s.split(',').filter_map(|x| x.trim().parse::<usize>().ok()).collect())
            .filter(|v: &Vec<usize>| !v.is_empty())
            .unwrap_or_else(|| vec![1, 2, 4, 8]);

        let mut rows: Vec<(usize, Duration, u64)> = Vec::new();
        for w in widths {
            reset_progress();
            let _ = scan_root(w, p, "bench");
            reset_progress();
            let t = Instant::now();
            let tree = scan_root(w, p, "bench").expect("扫描应成功");
            rows.push((w, t.elapsed(), tree.size));
        }

        eprintln!("── {} ──", p.display());
        let base_d = rows[0].1.as_secs_f64();
        let base_size = rows[0].2 as f64;
        for (w, d, size) in &rows {
            let diff = if base_size > 0.0 {
                (*size as f64 - base_size) / base_size * 100.0
            } else {
                0.0
            };
            eprintln!(
                "线程上限 {w}: {:>10.1?}  加速比 {:.2}x  体积 {size} ({diff:+.2}%)",
                d,
                base_d / d.as_secs_f64()
            );
        }

        if strict {
            let first = rows[0].2;
            for (w, _, size) in &rows {
                assert_eq!(*size, first, "{w} 线程扫出来的体积与 1 线程不一致");
            }
        } else {
            // 真实盘：偏差应该很小；差得离谱就是漏项，要人看一眼
            for (w, _, size) in &rows {
                let diff = (*size as f64 - base_size).abs() / base_size * 100.0;
                assert!(diff < 3.0, "{w} 线程与 1 线程的体积相差 {diff:.2}%，疑似漏项或重复计数");
            }
        }
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
        let h = std::thread::spawn(move || scan_root(4, &p, "root").is_some());

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

    /// MFT 快速路径必须与遍历结果对得上。
    ///
    /// 需要管理员权限（读 $MFT 要打开卷设备），非管理员会直接 skip：
    ///   cargo test --release --lib mft_matches -- --ignored --nocapture
    /// 指定盘符：DM_MFT_TEST_DRIVE=D
    #[test]
    #[ignore = "需要管理员权限 + 真实 NTFS 盘"]
    fn mft_matches_walkdir() {
        let letter = std::env::var("DM_MFT_TEST_DRIVE").unwrap_or_else(|_| "C".into());
        let letter = letter.trim_end_matches(':').to_uppercase();

        let t0 = Instant::now();
        let mft = match scan_via_mft(&letter) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skip: MFT 不可用（{e}）—— 多半是没以管理员身份运行测试");
                return;
            }
        };
        let mft_time = t0.elapsed();

        reset_progress();
        let t1 = Instant::now();
        let root = format!("{letter}:\\");
        let ctx = ScanCtx::new(4);
        let walk = std::thread::scope(|sc| {
            build_node(sc, &ctx, &PathBuf::from(&root), root.clone(), 0)
        })
        .expect("遍历应成功");
        let walk_time = t1.elapsed();

        fn count(n: &FileNode) -> (usize, usize) {
            let mut d = usize::from(n.kind == "dir");
            let mut f = usize::from(n.kind == "file");
            if let Some(cs) = &n.children {
                for c in cs {
                    let (dd, ff) = count(c);
                    d += dd;
                    f += ff;
                }
            }
            (d, f)
        }
        let (md, mf) = count(&mft);
        let (wd, wf) = count(&walk);

        eprintln!(
            "MFT  {:>10.2?}  {:>14} 字节  目录 {md}  文件 {mf}",
            mft_time, mft.size
        );
        eprintln!(
            "遍历 {:>10.2?}  {:>14} 字节  目录 {wd}  文件 {wf}",
            walk_time, walk.size
        );

        // MFT 能看到「遍历因权限读不到」的目录里的内容，所以体积通常大于或等于遍历结果。
        // 反过来若明显偏小，就是解析漏了东西。
        let ratio = mft.size as f64 / walk.size.max(1) as f64;
        eprintln!("体积比 {ratio:.4}（>1 属正常）");

        assert!(ratio > 0.97, "MFT 比遍历小 {:.1}%，疑似漏解析", (1.0 - ratio) * 100.0);
        assert!(ratio < 1.15, "MFT 比遍历大 {:.1}%，疑似重复计数", (ratio - 1.0) * 100.0);
        assert!(md * 2 > wd, "MFT 目录数 {md} 远少于遍历 {wd}");
    }
}
