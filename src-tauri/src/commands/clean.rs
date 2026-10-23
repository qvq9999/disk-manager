//! 垃圾清理：规则驱动（规则表见 `clean_rules.rs`，对齐 Winapp2.ini 语义）
//! 全部命令后台线程执行，避免大目录 I/O 阻塞 UI
//!
//! 变更要点：
//!   ① 从「~20 条写死 C 盘的规则」升级为 **70+ 条声明式规则**，按 DetectFile 门控、
//!      FileKey 通配符 + RECURSE、ExcludeKey 排除；
//!   ② 系统类条目全部改为**逐盘**（%Drive%），勾哪个盘就只统计/清理哪个盘；
//!   ③ 新增「系统占用（不可删）」只读分组，如实呈现 pagefile / hiberfil 等占用的空间；
//!   ④ 清理复用同一 scope，保证「扫描看到的」与「实际删除的」一致。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use super::clean_rules::{self, Cat, RULES, USED_FILES};
use super::app_cache;
use super::fsize;
use super::fsx;

/// 清理取消标志。清理可能涉及几万个文件，用户点「停止」后必须能真的停下来，
/// 而不是等它把当前分类全部删完。检查点放在循环里（每 256 个文件一次），
/// 单次开销可以忽略，但响应延迟能压到毫秒级。
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 供 clean_rules / app_cache 的删除循环查询
pub(crate) fn cancel_requested() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// 前端点「停止清理」时调用
#[tauri::command]
pub fn cancel_clean() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 扫描范围：用户勾选的盘符（如 ["C","E"]）；为空表示按默认全盘
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanScope {
    pub drives: Vec<String>,
}

/// 扫描进度回调：百分比 + 当前动作描述
pub type ProgressFn<'a> = &'a dyn Fn(u32, &str);

/// 跳过统计定义在 `fsize`（统计口径模块），这里再导出一次，
/// 让 `clean_rules` / `app_cache` 能共用同一套定义而不产生模块环。
pub use super::fsize::SkipStats;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub safe: bool,
    pub lock_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_on: Option<bool>,
    /// 所属盘符（如 "E"）；跨盘或用户态条目为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
    /// 只读条目（系统占用，不可删除）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// 危险提示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// 为 true 表示「高级项」（前端显示琥珀色徽标）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advanced: Option<bool>,
    /// 该清理项里硬链接文件的数量与占用。
    /// 硬链接删掉不释放空间，所以**不计入 `size`**；但要让用户知道
    /// 「这里其实还压着 8GB，只是删了不会还给你」（典型：Windows.old）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardlinks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardlink_bytes: Option<u64>,
    /// 扫描时整棵目录读不进去的数量：这些位置的真实大小未知，
    /// 当它 > 0 时「可清理量」必然偏小，必须让用户看见。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_dirs: Option<u64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanCategory {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub items: Vec<CleanItem>,
}

/// 用户勾选的盘符；为空则回落到默认盘（C/D/E/F）
pub(crate) fn effective_drives(drives: &[String]) -> Vec<char> {
    let src: Vec<String> = if drives.is_empty() {
        ["C", "D", "E", "F"].iter().map(|s| s.to_string()).collect()
    } else {
        drives.to_vec()
    };
    let mut out: Vec<char> = src
        .iter()
        .filter_map(|s| s.trim().chars().next())
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 收集清理项（已按盘符过滤）
fn collect(drives: &[char], progress: Option<ProgressFn>) -> Vec<(Cat, CleanItem)> {
    let mut out: Vec<(Cat, CleanItem)> = Vec::new();
    let report = |pct: u32, msg: &str| {
        if let Some(f) = progress {
            f(pct, msg);
        }
    };

    // ---- 声明式规则 ----
    // 进度 0~65%：按规则序号推进（规则之间耗时差异大，但总量可控，够用作体感反馈）
    let rule_total = RULES.len().max(1);
    for (idx, rule) in RULES.iter().enumerate() {
        report(
            (idx as u32 * 65 / rule_total as u32).min(65),
            &format!("正在扫描：{}", rule.name),
        );
        if !clean_rules::detect_ok(rule) {
            continue;
        }
        if rule.per_drive {
            for &letter in drives {
                let t = clean_rules::scan_rule(rule, Some(letter));
                // 只有硬链接、没有可释放字节的项也要显示出来（否则 Windows.old 会凭空消失）
                if t.bytes == 0 && t.hardlinks == 0 {
                    continue;
                }
                out.push((
                    rule.cat,
                    CleanItem {
                        id: format!("{}:{}", rule.id, letter),
                        name: format!("{}（{}:）", rule.name, letter),
                        path: rule
                            .paths
                            .first()
                            .map(|p| p.path.replace("%Drive%", &format!("{letter}:")))
                            .unwrap_or_default(),
                        size: t.bytes,
                        safe: true,
                        lock_reason: None,
                        default_on: Some(rule.default_on),
                        drive: Some(letter.to_string()),
                        read_only: None,
                        warning: rule.warning.map(|w| w.to_string()),
                        advanced: if rule.advanced { Some(true) } else { None },
                        hardlinks: if t.hardlinks > 0 { Some(t.hardlinks) } else { None },
                        hardlink_bytes: if t.hardlinks > 0 { Some(t.hardlink_bytes) } else { None },
                        denied_dirs: if t.denied_dirs > 0 { Some(t.denied_dirs) } else { None },
                    },
                ));
            }
        } else {
            let t = clean_rules::scan_rule(rule, None);
            if t.bytes == 0 && t.hardlinks == 0 {
                continue;
            }
            out.push((
                rule.cat,
                CleanItem {
                    id: rule.id.to_string(),
                    name: rule.name.to_string(),
                    path: rule.paths.first().map(|p| p.path.to_string()).unwrap_or_default(),
                    size: t.bytes,
                    safe: true,
                    lock_reason: None,
                    default_on: Some(rule.default_on),
                    drive: None,
                    read_only: None,
                    warning: rule.warning.map(|w| w.to_string()),
                    advanced: if rule.advanced { Some(true) } else { None },
                    hardlinks: if t.hardlinks > 0 { Some(t.hardlinks) } else { None },
                    hardlink_bytes: if t.hardlinks > 0 { Some(t.hardlink_bytes) } else { None },
                    denied_dirs: if t.denied_dirs > 0 { Some(t.denied_dirs) } else { None },
                },
            ));
        }
    }

    // ---- 应用缓存自动发现（覆盖未逐条建规则的 Electron / Chromium 系应用）----
    // 应用缓存全部位于用户目录（通常 C:），只有扫描范围含该盘时才做深度扫描
    let user_drive = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| p.chars().next())
        .map(|c| c.to_ascii_uppercase());
    if user_drive.map(|d| drives.contains(&d)).unwrap_or(false) {
        report(66, "正在深度扫描应用缓存…");
        // 跳过静态规则已覆盖的路径，避免同一批文件被统计两次
        let covered = clean_rules::covered_cache_paths(drives);
        let found = app_cache::discover(&covered, 5);
        report(90, &format!("已发现 {} 个应用的缓存", found.len()));
        for fc in found {
            let paths: Vec<String> = fc
                .dirs
                .iter()
                .map(|d| d.to_string_lossy().to_string())
                .collect();
            out.push((
                Cat::AppCache,
                CleanItem {
                    // ⚠️ id 编码**真实绝对路径**（多个用 | 分隔）：扫描与清理是两次独立调用，
                    //    按索引在顺序变化时会删错目录。删除端会重新过安全闸。
                    id: format!("appcache:{}", paths.join("|")),
                    name: format!("{} 缓存", fc.app),
                    path: paths.first().cloned().unwrap_or_default(),
                    size: fc.size,
                    safe: true,
                    lock_reason: None,
                    // 显卡相关（NVIDIA / AMD / Intel 的 DXCache、GLCache、着色器预编译等）
                    // **默认不勾**：它们装着游戏与创作软件的预加载内容，清了要重新编译，
                    // 首次启动明显变慢。用户可手动勾选，但不能默认替用户做这个决定。
                    default_on: Some(!fc.gpu),
                    drive: None,
                    read_only: None,
                    warning: if fc.gpu {
                        Some("显卡相关缓存（着色器预编译 / 驱动预加载）：删除后游戏与创作软件首次启动会重新编译，可能明显变慢。默认不勾选。".to_string())
                    } else {
                        None
                    },
                    advanced: if fc.gpu { Some(true) } else { None },
                    hardlinks: if fc.hardlinks > 0 { Some(fc.hardlinks) } else { None },
                    hardlink_bytes: if fc.hardlinks > 0 { Some(fc.hardlink_bytes) } else { None },
                    denied_dirs: None,
                },
            ));
        }
    }

    // ---- 已卸载软件残留（Geek / HiBit 式孤儿目录）----
    // 保守判定：目录名未命中任何已安装软件、且不在系统白名单内
    // ⚠️ id 里编码**真实绝对路径**而不是索引：扫描与清理是两次独立调用，
    //    按索引删会在两次结果顺序不一致时删错目录（这是不可接受的破坏性风险）
    report(95, "正在检查已卸载软件的残留…");
    for od in clean_rules::scan_uninstall_residuals() {
        out.push((
            Cat::Residual,
            CleanItem {
                id: format!("residual:{}", od.path.to_string_lossy()),
                name: format!(
                    "{}（残留）",
                    od.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                path: od.shown.clone(),
                size: od.size,
                safe: true,
                lock_reason: None,
                default_on: Some(false), // 高级项：默认不勾选，必须人工确认
                drive: od
                    .path
                    .to_string_lossy()
                    .chars()
                    .next()
                    .filter(|c| c.is_ascii_alphabetic())
                    .map(|c| c.to_ascii_uppercase().to_string()),
                read_only: None,
                warning: Some(format!(
                    "疑似已卸载软件的残留目录：目录名未出现在已安装列表中，且最近 45 天无写入。共 {} 个文件。\
                     ⚠️ 清理会删除**整个目录**，其中的配置与数据不会保留；请确认该软件确已卸载（且里面没有你要留的文件）后再勾选。",
                    od.files
                )),
                advanced: Some(true),
                hardlinks: None,
                hardlink_bytes: None,
                denied_dirs: None,
            },
        ));
    }

    // 系统占用（只读展示）
    for &letter in drives {
        for (file, label, hint) in USED_FILES {
            let p = PathBuf::from(format!("{letter}:\\{file}"));
            if !p.exists() {
                continue;
            }
            // 页面锁定文件同样按占用空间展示：用户对照资源管理器时才不会觉得"少了"
            let size = fsize::stat(&p).map(|s| s.alloc).unwrap_or(0);
            if size == 0 {
                continue;
            }
            out.push((
                Cat::Used,
                CleanItem {
                    id: format!("used:{file}:{letter}"),
                    name: format!("{label}（{letter}:）"),
                    path: format!("{letter}:\\{file}"),
                    size,
                    safe: false,
                    lock_reason: Some(hint.to_string()),
                    default_on: None,
                    drive: Some(letter.to_string()),
                    read_only: Some(true),
                    warning: None,
                    advanced: None,
                    hardlinks: None,
                    hardlink_bytes: None,
                    denied_dirs: None,
                },
            ));
        }
    }

    out
}

static STATS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 统计可清理项；scope 为空 → 默认全盘，保持向后兼容。
/// `on_progress` 用于汇报进度：深度扫描应用缓存需要数秒，前端要能显示「正在扫哪个应用」。
///
/// 返回 `Result`：以前这里用 `catch_unwind(...).unwrap_or_default()`，
/// 一旦扫描 panic 就返回**空列表**，界面显示「未发现可清理项」——
/// 用户会以为盘很干净。现在 panic 会经由 JoinError 变成明确的错误提示。
#[tauri::command]
pub async fn get_clean_items(
    scope: Option<CleanScope>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
) -> Result<Vec<CleanCategory>, String> {
    let drives = scope.map(|s| s.drives).unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = STATS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let letters = effective_drives(&drives);
        let reporter = |pct: u32, msg: &str| {
            let _ = on_progress.send(serde_json::json!({ "pct": pct, "current": msg }));
        };
        let items = collect(&letters, Some(&reporter));
        reporter(100, "扫描完成");
        Cat::ALL
            .iter()
            .map(|cat| CleanCategory {
                id: cat.id().to_string(),
                name: cat.name().to_string(),
                icon: cat.icon().to_string(),
                items: items
                    .iter()
                    .filter(|(c, _)| c == cat)
                    .map(|(_, i)| i.clone())
                    .collect(),
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| format!("扫描任务失败（{e}），请重试；若持续失败请反馈"))
}

#[tauri::command]
pub async fn run_clean(
    ids: Vec<String>,
    scope: Option<CleanScope>,
    on_progress: tauri::ipc::Channel<serde_json::Value>,
    // 前端已知的待清理总量。传了就跳过「清理前再扫一遍」——
    // 深度扫描加入后这次重扫要 1~4 秒，白等没有意义（它只影响进度百分比）
    expected_total: Option<u64>,
) -> Result<CleanOutcome, String> {
    let drives = scope.map(|s| s.drives).unwrap_or_default();
    CANCEL.store(false, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        run_clean_impl(ids, &drives, &on_progress, expected_total)
    })
    .await
    .map_err(|e| format!("清理任务失败: {e}"))?
}

/// 清理结果。
///
/// `cancelled` 必须单独回报：用户中途停止时，界面既要展示**已释放多少**（真删了就是真删了），
/// 又不能把"没处理完"说成"清理完成"。
///
/// `skipped` 与 `diskDelta` 是回答"为什么数字对不上"的关键：
///   - `freed` 是**按 AllocationSize 算的、真的会释放的字节**；
///   - `skipped.bytes` 是被占用/权限不足挡住、仍然躺在盘上的字节；
///   - `diskDelta` 是卷可用空间的实际变化（清理前后各读一次）——
///     它会受其他进程同时写盘影响，所以只作为佐证呈现，不参与对账。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanOutcome {
    pub freed: u64,
    /// 兼容字段 = skipped.count()（旧前端仍在读）
    pub failed: u32,
    pub cancelled: bool,
    pub skipped: SkipStats,
    /// 卷可用空间变化（字节）；正数=释放，负数=期间有程序写入
    pub disk_delta: Option<i64>,
}

/// id 形态：全局规则 `rule.id`；逐盘规则 `rule.id:盘符`
fn split_id(id: &str) -> (&str, Option<char>) {
    match id.rsplit_once(':') {
        Some((rid, d)) => {
            let c = d.trim().chars().next().map(|c| c.to_ascii_uppercase());
            (rid, c)
        }
        None => (id, None),
    }
}

fn run_clean_impl(
    ids: Vec<String>,
    drives: &[String],
    on_progress: &tauri::ipc::Channel<serde_json::Value>,
    expected_total: Option<u64>,
) -> Result<CleanOutcome, String> {
    let letters = effective_drives(drives);
    // 清理前记一次卷可用空间：结束时再读一次，差值就是用户眼睛真正看到的"释放了多少"
    let free_before = total_free(&letters);
    // 总量只用于进度百分比：前端能给出就不必再全量扫一遍
    let total: u64 = match expected_total {
        Some(t) => t,
        None => {
            let scanned = collect(&letters, None);
            ids.iter()
                .filter_map(|id| scanned.iter().find(|(_, i)| &i.id == id).map(|(_, i)| i.size))
                .sum()
        }
    };

    let mut freed = 0u64;
    let mut skip = SkipStats::default();
    let mut cancelled = false;
    let item_total = ids.len() as u32;

    for (idx, id) in ids.iter().enumerate() {
        if cancel_requested() {
            cancelled = true;
            break;
        }
        if id.starts_with("used:") {
            continue; // 只读项，永不删除
        }
        let done = idx as u32;
        // 应用缓存：id 形如 `appcache:C:\...\Cache|C:\...\Code Cache`
        // 每条路径都重新过 app_cache::path_allowed（白名单目录名 + 黑名单路径段 + 必须在用户目录下），
        // 只删目录内文件、保留目录本身
        if let Some(raw) = id.strip_prefix("appcache:") {
            let _ = on_progress.send(serde_json::json!({
                "pct": if total > 0 { ((freed * 100 / total) as u32).min(99) } else { 0 },
                "current": "正在清理应用缓存",
                "freed": freed,
                "done": done,
                "total": item_total,
            }));
            for one in raw.split('|') {
                if cancel_requested() {
                    cancelled = true;
                    break;
                }
                let target = std::path::PathBuf::from(one);
                // 安全闸复核在 delete_contents 内部做（拒绝会计入 rejected，不算失败）
                app_cache::delete_contents(&target, &mut freed, &mut skip, |f| {
                    let _ = on_progress.send(serde_json::json!({
                        "pct": if total > 0 { ((f * 100 / total) as u32).min(99) } else { 0 },
                        "current": target
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        "freed": f,
                        "done": done,
                        "total": item_total,
                    }));
                });
            }
            continue;
        }
        // 已卸载软件残留：直接删目录（不进回收站，与残留清理一致）
        // id 形如 `residual:D:\Program Files\Foo`，带 4 道安全闸：
        //   ① 必须是绝对路径且存在；② 必须是目录；③ 必须位于允许的根目录之下；
        //   ④ 删除前重新确认它仍不在「已安装软件关键词集」里
        if let Some(raw) = id.strip_prefix("residual:") {
            let target = std::path::PathBuf::from(raw);
            let _ = on_progress.send(serde_json::json!({
                "pct": if total > 0 { ((freed * 100 / total) as u32).min(99) } else { 0 },
                "current": format!(
                    "正在清理卸载残留 {}",
                    target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                ),
                "freed": freed,
                "done": done,
                "total": item_total,
            }));
            if !target.is_absolute() || !target.is_dir() || !clean_rules::residual_path_allowed(&target) {
                skip.reject(0); // 保护生效，不是失败
                continue;
            }
            // 走 fsx：清只读 → 权限修复重试；`freed` 只按**实际删掉且会释放**的字节计
            let r = fsx::remove_dir(&target);
            freed += r.freed;
            skip.merge(&r.skipped);
            continue;
        }
        let (rid, drive) = split_id(id);
        let Some(rule) = clean_rules::rule_by_id(rid) else { continue };
        let _ = on_progress.send(serde_json::json!({
            "pct": if total > 0 { ((freed * 100 / total) as u32).min(99) } else { 0 },
            "current": format!("正在清理 {}", rule.name),
            "freed": freed,
            "done": done,
            "total": item_total,
        }));
        clean_rules::delete_rule(rule, drive, &mut freed, &mut skip, |f, name| {
            let _ = on_progress.send(serde_json::json!({
                "pct": if total > 0 { ((f * 100 / total) as u32).min(99) } else { 0 },
                "current": name,
                "freed": f,
                "done": done,
                "total": item_total,
            }));
        });
        cancelled = cancel_requested();
        if cancelled {
            break;
        }
    }

    // 差值可能为负：清理期间别的程序在写盘。如实呈现，不修正成"看起来对"的数字。
    let disk_delta = match (free_before, total_free(&letters)) {
        (Some(a), Some(b)) => Some(b as i64 - a as i64),
        _ => None,
    };

    Ok(CleanOutcome { freed, failed: skip.count(), cancelled, skipped: skip, disk_delta })
}

/// 参与清理的盘符当前可用空间之和
fn total_free(letters: &[char]) -> Option<u64> {
    let mut sum = 0u64;
    let mut any = false;
    for &l in letters {
        if let Some(f) = fsize::volume_free(&PathBuf::from(format!("{l}:\\"))) {
            sum += f;
            any = true;
        }
    }
    if any {
        Some(sum)
    } else {
        None
    }
}
