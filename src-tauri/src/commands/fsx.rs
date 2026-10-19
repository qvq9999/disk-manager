//! 删除增强：属性 / 占用 / 权限 / 重启后删除
//!
//! 为什么需要这一层：`std::fs::remove_file` / `remove_dir_all` 在 Windows 上有三个硬伤，
//! 而它们的报错都会被上层折成同一个 `failed += 1`，用户只看到「权限不足」：
//!
//! ① **只读属性直接拒删**（`DeleteFileW` 返回 WinError 5）。本机实测：带
//!    `FILE_ATTRIBUTE_READONLY` 的文件删除失败，清掉属性后同一路径立刻成功。
//!    这跟"权限"毫无关系，却和真正的 ACL 拒绝共用同一个错误码 5。
//! ② **文件被占用**（WinError 32 SHARING_VIOLATION）也被报成 5 或 32，用户读到的仍是"权限不足"。
//! ③ `remove_dir_all` 遇到**第一个**失败项就整体返回 Err，既无法逐项处理，也说不清是哪一项挡住。
//!
//! 这里的策略是「能救的都救，救不了的如实说明，并给出重启后删除的兜底」：
//!   清属性 → 删除 → 失败则夺所有权/授权后重试 → 仍被占用则登记「重启后删除」。
//!
//! ⚠️ 只对**目标路径自身**动 ACL（takeown / icacls），不改任何全局安全策略；
//!    该分支只在 AccessDenied 时触发，正常删除不会多花一分钱。

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    DeleteFileW, GetFileAttributesW, MoveFileExW, RemoveDirectoryW, SetFileAttributesW,
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
    INVALID_FILE_ATTRIBUTES, MOVEFILE_DELAY_UNTIL_REBOOT,
};

/// 一次删除的结果。**必须逐项返回**，而不是只回一个计数：
/// 用户需要知道"哪一项没删掉、为什么、要不要重启"。
#[derive(Clone, Debug, Default)]
pub struct DelResult {
    pub ok: bool,
    pub freed: u64,
    /// 原始 WinError（0 表示成功）。保留给日志与单测：界面展示用 `reason` 的人话文案。
    #[allow(dead_code)]
    pub win_error: u32,
    /// 人话原因（失败时非空）
    pub reason: Option<String>,
    /// 已登记为「重启后删除」：空间要重启后才真正释放
    pub pending_reboot: bool,
    /// 曾修改过该路径的权限（takeown / icacls）
    pub acl_fixed: bool,
}

impl DelResult {
    fn ok(freed: u64) -> Self {
        DelResult { ok: true, freed, ..Default::default() }
    }
    fn fail(code: u32, freed: u64) -> Self {
        DelResult {
            ok: false,
            freed,
            win_error: code,
            reason: Some(reason_for(code).to_string()),
            pending_reboot: false,
            acl_fixed: false,
        }
    }
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn reason_for(code: u32) -> &'static str {
    match code {
        2 | 3 => "路径已不存在（可能已被删除）",
        5 => "权限拒绝（已尝试取得所有权）",
        32 => "文件被其他进程占用",
        33 => "文件被部分锁定",
        145 => "目录非空（内部有未能删除的项）",
        _ => "删除失败",
    }
}

/// 清掉只读 / 隐藏 / 系统属性。只读是「删除失败」最常见也最容易救的原因。
fn clear_attrs(p: &Path) -> bool {
    const STRIP: u32 = FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM;
    unsafe {
        let w = wide(p);
        let a = GetFileAttributesW(w.as_ptr());
        if a == INVALID_FILE_ATTRIBUTES {
            return false;
        }
        if a & STRIP == 0 {
            return true;
        }
        SetFileAttributesW(w.as_ptr(), a & !STRIP) != 0
    }
}

fn del_file_once(p: &Path) -> Result<(), u32> {
    clear_attrs(p);
    unsafe {
        if DeleteFileW(wide(p).as_ptr()) != 0 {
            Ok(())
        } else {
            Err(GetLastError())
        }
    }
}

fn del_dir_once(p: &Path) -> Result<(), u32> {
    clear_attrs(p);
    unsafe {
        if RemoveDirectoryW(wide(p).as_ptr()) != 0 {
            Ok(())
        } else {
            Err(GetLastError())
        }
    }
}

/// 登记「重启后删除」（PendingFileRenameOperations）。
///
/// 这条路径专门救**被占用**的文件：`MoveFileExW` 只把操作写进注册表，
/// 不需要现在就能拿到独占访问权，所以对句柄被占用的文件同样有效
/// （这也是各类安装器替换正在使用的 dll 时用的同一手段）。
pub fn schedule_delete_on_reboot(p: &Path) -> bool {
    unsafe {
        MoveFileExW(wide(p).as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) != 0
    }
}

/// 取得目标路径的所有权并授予本机 Administrators 完全控制。
///
/// 用 takeown / icacls 而不是手写 `SetNamedSecurityInfoW`：这条只在 AccessDenied 时才走，
/// 一次子进程几百毫秒可以接受；而手写 ACE 编辑要处理继承标志、SACL、大小计算，
/// 在"只删一个残留目录"这种场景里收益与风险不成比例。
fn acl_fix(p: &Path) -> bool {
    let s = p.to_string_lossy().to_string();
    if s.trim().is_empty() {
        return false;
    }
    let is_dir = p.is_dir();
    let mut takeown = crate::commands::silent_command("takeown");
    takeown.args(["/f", &s]);
    if is_dir {
        takeown.args(["/r", "/d", "y"]);
    }
    let t_ok = takeown.output().map(|o| o.status.success()).unwrap_or(false);

    let mut icacls = crate::commands::silent_command("icacls");
    icacls.args([&s, "/grant", "*S-1-5-32-544:F", "/c", "/q"]);
    if is_dir {
        icacls.arg("/t");
    }
    let i_ok = icacls.output().map(|o| o.status.success()).unwrap_or(false);
    t_ok || i_ok
}

/// 删除单个文件：清属性 → 删 → 权限拒绝则修 ACL 重试 → 仍失败则登记重启后删除。
pub fn remove_file(p: &Path) -> DelResult {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    match del_file_once(p) {
        Ok(()) => DelResult::ok(size),
        Err(5) | Err(33) => {
            let fixed = acl_fix(p);
            match del_file_once(p) {
                Ok(()) => DelResult { acl_fixed: fixed, ..DelResult::ok(size) },
                Err(e) => {
                    if e == 2 || e == 3 {
                        return DelResult::ok(0); // 已不存在，视为成功
                    }
                    let reboot = schedule_delete_on_reboot(p);
                    DelResult {
                        ok: false,
                        freed: 0,
                        win_error: e,
                        reason: Some(if reboot {
                            "被占用，已安排重启后删除".into()
                        } else {
                            reason_for(e).to_string()
                        }),
                        pending_reboot: reboot,
                        acl_fixed: fixed,
                    }
                }
            }
        }
        Err(32) => {
            let reboot = schedule_delete_on_reboot(p);
            DelResult {
                ok: false,
                freed: 0,
                win_error: 32,
                reason: Some(if reboot {
                    "被占用，已安排重启后删除".into()
                } else {
                    reason_for(32).to_string()
                }),
                pending_reboot: reboot,
                acl_fixed: false,
            }
        }
        Err(e) => DelResult::fail(e, 0),
    }
}

/// 收集目录树：返回 (文件列表, 目录列表)，均为「深的在前」，便于先删文件再自底向上删空目录。
/// `follow_links(false)` 保证不会顺着一层 junction 删到别的盘上去。
fn collect_tree(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = e.path().to_path_buf();
        if p == root {
            continue;
        }
        let ft = e.file_type();
        if ft.is_symlink() {
            // junction / symlink：只删链接本身，绝不进入目标
            files.push(p);
        } else if ft.is_dir() {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    // walkdir 默认自顶向下，反转得到「深的在前」
    files.reverse();
    dirs.reverse();
    (files, dirs)
}

/// 删除整棵目录树，逐项处理而不是整批失败。
///
/// 返回的 `freed` 是**实际删掉**的字节数（失败的项不计入），
/// 与旧实现「先统计再删、失败也照算」相比，界面上的数字不会再虚高。
pub fn remove_dir(root: &Path) -> DelResult {
    if root.symlink_metadata().is_err() {
        return DelResult::ok(0);
    }
    // 目录本身是链接：只摘链接
    if root.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        return match del_dir_once(root) {
            Ok(()) => DelResult::ok(0),
            Err(e) => DelResult::fail(e, 0),
        };
    }

    let (files, dirs) = collect_tree(root);
    let mut freed = 0u64;
    let mut errors: Vec<(PathBuf, u32)> = Vec::new();
    let mut acl_fixed = false;

    for f in &files {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        match del_file_once(f) {
            Ok(()) => freed += size,
            Err(32) | Err(33) => errors.push((f.clone(), 32)),
            Err(e) if e == 5 => errors.push((f.clone(), 5)),
            Err(e) if e == 2 || e == 3 => {}
            Err(e) => errors.push((f.clone(), e)),
        }
    }
    for d in &dirs {
        match del_dir_once(d) {
            Ok(()) => {}
            Err(e) if e == 145 => {} // 子项没删掉，稍后统一处理
            Err(e) if e == 2 || e == 3 => {}
            Err(e) => errors.push((d.clone(), e)),
        }
    }
    let root_gone = del_dir_once(root).is_ok();

    // 还有残留：修一次根目录权限，整轮重来
    if !root_gone {
        let need_acl = errors.iter().any(|(_, c)| *c == 5);
        if need_acl && acl_fix(root) {
            acl_fixed = true;
            let (files2, dirs2) = collect_tree(root);
            for f in &files2 {
                let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                if del_file_once(f).is_ok() {
                    freed += size;
                }
            }
            for d in &dirs2 {
                let _ = del_dir_once(d);
            }
            if del_dir_once(root).is_ok() {
                return DelResult { acl_fixed, ..DelResult::ok(freed) };
            }
        }
    } else {
        return DelResult { acl_fixed, ..DelResult::ok(freed) };
    }

    // 真正救不回来的那一批：能登记重启后删除的就登记，并如实标注
    let (left_files, _) = collect_tree(root);
    let mut reboot_all = !left_files.is_empty();
    for f in &left_files {
        if !schedule_delete_on_reboot(f) {
            reboot_all = false;
        }
    }
    let code = errors.iter().map(|(_, c)| *c).find(|c| *c != 0).unwrap_or(5);
    DelResult {
        ok: false,
        freed,
        win_error: code,
        reason: Some(format!(
            "{} 个文件未能删除（{}），已{}",
            left_files.len(),
            reason_for(code),
            if reboot_all { "安排重启后删除" } else { "保留原样，可重启后重试" }
        )),
        pending_reboot: reboot_all && !left_files.is_empty(),
        acl_fixed,
    }
}

/// 按路径类型分派。
pub fn remove_path(p: &Path) -> DelResult {
    let md = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return DelResult::ok(0),
    };
    if md.is_dir() && !md.file_type().is_symlink() {
        remove_dir(p)
    } else if md.is_dir() {
        match del_dir_once(p) {
            Ok(()) => DelResult::ok(0),
            Err(e) => DelResult::fail(e, 0),
        }
    } else {
        remove_file(p)
    }
}

/// 轻量删除（清理缓存/空目录/安装包时用）：清属性 + 权限修复重试，但**不登记重启后删除**。
/// 这类项目数量以万计，给每个文件写一条 PendingFileRenameOperations 会把注册表撑爆，
/// 而且缓存文件下次启动自然会被重建，没有"必须重启删掉"的必要。
pub fn remove_file_lenient(p: &Path) -> bool {
    match del_file_once(p) {
        Ok(()) => true,
        Err(2) | Err(3) => true,
        Err(e) => {
            if e == 5 {
                acl_fix(p);
                del_file_once(p).is_ok()
            } else {
                false
            }
        }
    }
}

/// 未使用但保留：判断句柄是否可独占打开，可用于将来更精确的占用诊断。
#[allow(dead_code)]
pub fn looks_locked(p: &Path) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, OPEN_EXISTING};
    let h = unsafe {
        CreateFileW(
            wide(p).as_ptr(),
            0x8000_0000,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return true;
    }
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("dm-fsx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn 只读文件会被清属性后删除() {
        let d = tmp("ro");
        let f = d.join("a.txt");
        std::fs::File::create(&f).unwrap().write_all(b"hello").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();
        let r = remove_file(&f);
        assert!(r.ok, "只读文件应删除成功，实际 {:?}", r);
        assert_eq!(r.freed, 5);
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 目录含只读文件也能整体删除() {
        let d = tmp("rodir");
        let sub = d.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let f = sub.join("a.txt");
        std::fs::File::create(&f).unwrap().write_all(b"1234").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();
        let r = remove_dir(&d);
        assert!(r.ok, "应整体删除成功，实际 {:?}", r);
        assert_eq!(r.freed, 4);
        assert!(!d.exists());
    }

    #[test]
    fn 空目录删除返回零字节() {
        let d = tmp("empty");
        let r = remove_path(&d);
        assert!(r.ok);
        assert_eq!(r.freed, 0);
        assert!(!d.exists());
    }

    #[test]
    fn 不存在的路径视为已完成() {
        let p = std::env::temp_dir().join("dm-fsx-does-not-exist-xyz");
        let r = remove_path(&p);
        assert!(r.ok);
    }

    #[test]
    fn 占用文件登记重启后删除() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tmp("lock");
        let f = d.join("locked.bin");
        std::fs::File::create(&f).unwrap().write_all(b"x").unwrap();
        // share_mode(0) = 独占：此时 DeleteFileW 必然失败（SHARING_VIOLATION）
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&f)
            .unwrap();
        let r = remove_file(&f);
        drop(held);
        assert!(!r.ok, "被占用时不应谎报成功");
        assert!(r.win_error == 32 || r.win_error == 33, "应识别为占用，实际 {}", r.win_error);
        // 登记重启后删除在非管理员下会失败，这里只断言"不谎报成功"
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_dir_all(&d);
    }
}
