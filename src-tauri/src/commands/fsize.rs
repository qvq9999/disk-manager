//! 文件大小口径（**唯一入口**）
//!
//! 为什么必须单独一层：`metadata().len()` 与用户/资源管理器看到的数字不是一回事。
//!
//! ① **逻辑大小 ≠ 占用空间**。NTFS 按簇分配，`AllocationSize` 才是「占用空间」；
//!    稀疏文件 / NTFS 压缩 / 内存映射写出的「有效数据长度」小于文件大小的情况，
//!    两者能差一倍以上。本机实测用户 Temp：len 之和 6.9 MiB，占用之和 13.7 MiB（1.98x）。
//!    此前扫描与释放都用 len，于是「预计释放 6.9MB」而磁盘实际多了 13.7MB。
//! ② **硬链接不计入**。`Windows.old` / `$WINDOWS.~BT` 里大量文件是硬链接到新系统的，
//!    删掉链接只是少一个引用，**一个字节都不会释放**。按 len 累加会凭空多出几个 GB 的「已释放」。
//!    判据：删除前 `NumberOfLinks > 1` 的一律不计（保守：宁少报不虚高）。
//!
//! 结论：**扫描、删除、目录统计三处必须调用同一个函数**，否则三个数字永远对不上。

use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileStandardInfo, GetDiskFreeSpaceExW, GetFileInformationByHandleEx,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
};

/// 结构与 Win32 对齐，编译期锁死尺寸（踩过：字段顺序错一字节，数字全是垃圾）
const _: () = assert!(std::mem::size_of::<FILE_STANDARD_INFO>() == 24);

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FileStat {
    /// 实际占用（AllocationSize）：删掉它能释放的字节数
    pub alloc: u64,
    /// 逻辑大小（EndOfFile）：资源管理器「大小」列。
    /// 只用于对照排查（`len` 与 `alloc` 差多少），业务判断一律用 `alloc`。
    #[allow(dead_code)]
    pub logical: u64,
    /// 硬链接数；>1 表示删掉当前链接不释放空间
    pub links: u32,
}

/// 取单个文件的占用与链接数。拿不到（权限/已删除/不支持）返回 None。
pub fn stat(p: &Path) -> Option<FileStat> {
    unsafe {
        let h = CreateFileW(
            wide(p).as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: FILE_STANDARD_INFO = std::mem::zeroed();
        let ok = GetFileInformationByHandleEx(
            h,
            FileStandardInfo,
            &mut info as *mut FILE_STANDARD_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        );
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let logical = info.EndOfFile.max(0) as u64;
        let alloc = if info.AllocationSize > 0 {
            info.AllocationSize as u64
        } else {
            logical // 常驻文件（数据在 MFT 里）AllocationSize 可能为 0
        };
        Some(FileStat { alloc, logical, links: info.NumberOfLinks })
    }
}

/// 卷可用空间（当前调用者）。用于给「实际释放」提供独立佐证。
pub fn volume_free(path: &Path) -> Option<u64> {
    let mut free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide(path).as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        Some(free)
    } else {
        None
    }
}

/// 一个清理项的累计结果。
///
/// `bytes` 才是「可清理 / 已释放」；硬链接单独计数，既不计入也不丢弃 ——
/// 界面要能回答「为什么这里还有 2GB 你没算」。
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub bytes: u64,
    pub files: u64,
    pub hardlinks: u64,
    pub hardlink_bytes: u64,
    /// 读不到大小的文件（按 len 兜底计入 bytes）
    pub unknown: u64,
    /// 遍历时被拒的目录数。
    /// `walkdir ... filter_map(|e| e.ok())` 会把 Access Denied 的整棵子树删掉，
    /// 既不统计也不报错 —— 用户看到的就是「盘很干净」，实际是「没权限看」。
    pub denied_dirs: u64,
}

impl Tally {
    pub fn add(&mut self, st: FileStat, known: bool) {
        self.files += 1;
        if !known {
            self.unknown += 1;
        }
        if st.links > 1 {
            // 删掉这个链接不释放空间：单独记，不混进 bytes
            self.hardlinks += 1;
            self.hardlink_bytes += st.alloc;
        } else {
            self.bytes += st.alloc;
        }
    }

    pub fn add_stat(&mut self, p: &Path) {
        match stat(p) {
            Some(s) => self.add(s, true),
            None => {
                let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                self.add(FileStat { alloc: len, logical: len, links: 1 }, false);
            }
        }
    }

    pub fn merge(&mut self, o: Tally) {
        self.bytes += o.bytes;
        self.files += o.files;
        self.hardlinks += o.hardlinks;
        self.hardlink_bytes += o.hardlink_bytes;
        self.unknown += o.unknown;
        self.denied_dirs += o.denied_dirs;
    }
}

/// 被跳过的统计。
///
/// 以前只有一个 `failed: u32`，把「被占用 / 权限不足 / 安全闸拒绝」三种完全不同的原因
/// 混成一个数，前端还统一写成「N 项被占用或权限不足」——
/// 于是用户看到「预计 20GB、释放 12GB、8 项跳过」时，既不知道少的 8GB 在哪，
/// 也不知道那 8 项到底是什么原因。这里必须拆开，并累计被跳过的**字节数**。
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipStats {
    /// WinError 32 / 33：文件被其他进程持有（关掉对应程序可删）
    pub locked: u32,
    /// WinError 5：权限拒绝（已尝试取得所有权与授权）
    pub denied: u32,
    /// 安全闸拒绝：路径校验不通过 —— 这是**保护**，不是失败
    pub rejected: u32,
    /// 其它（目录非空、I/O 错误、路径已变化）
    pub other: u32,
    /// 遍历时整棵子树就读不进去的目录数（连"有哪些文件"都不知道）
    pub denied_dirs: u32,
    /// 被跳过项占用的字节数（与扫描同口径：AllocationSize）
    pub bytes: u64,
}

impl SkipStats {
    pub fn record(&mut self, code: u32, bytes: u64) {
        match code {
            32 | 33 => self.locked += 1,
            5 => self.denied += 1,
            _ => self.other += 1,
        }
        self.bytes += bytes;
    }

    /// 安全闸拒绝：这是保护生效，不该混进「失败」
    pub fn reject(&mut self, bytes: u64) {
        self.rejected += 1;
        self.bytes += bytes;
    }

    pub fn count(&self) -> u32 {
        self.locked + self.denied + self.rejected + self.other
    }

    pub fn merge(&mut self, o: &SkipStats) {
        self.locked += o.locked;
        self.denied += o.denied;
        self.rejected += o.rejected;
        self.other += o.other;
        self.denied_dirs += o.denied_dirs;
        self.bytes += o.bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("dm_fsize_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn 占用大小不小于逻辑大小且非零() {
        let d = tmp("basic");
        let f = d.join("a.bin");
        let mut fh = std::fs::File::create(&f).unwrap();
        fh.write_all(&vec![7u8; 5000]).unwrap();
        fh.sync_all().unwrap();
        drop(fh);
        let st = stat(&f).expect("stat 应有结果");
        assert_eq!(st.logical, 5000);
        assert!(st.alloc >= 5000, "AllocationSize 至少要覆盖文件长度: {}", st.alloc);
        assert_eq!(st.links, 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 硬链接计入hardlinks而不是bytes() {
        let d = tmp("hardlink");
        let a = d.join("a.bin");
        std::fs::write(&a, vec![1u8; 8192]).unwrap();
        let b = d.join("b.link");
        // 建硬链接（std 没有 API，直接用 mklink /H）
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/H", &b.to_string_lossy(), &a.to_string_lossy()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            // 无权限建链接时跳过（CI 常见），不制造假绿
            eprintln!("跳过：无法创建硬链接");
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        let sa = stat(&a).unwrap();
        assert_eq!(sa.links, 2, "两条链接应报 2");
        let mut t = Tally::default();
        t.add(sa, true);
        assert_eq!(t.bytes, 0, "硬链接不得计入可释放字节");
        assert_eq!(t.hardlinks, 1);
        assert!(t.hardlink_bytes > 0, "但要如实记下它占了多少");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 不存在的文件不算入() {
        let mut t = Tally::default();
        t.add_stat(Path::new("C:\\__dm_not_exists__\\x.bin"));
        // 兜底逻辑会把 len=0 记成一个文件，字节数必须为 0
        assert_eq!(t.bytes, 0);
    }

    #[test]
    fn 卷可用空间可读() {
        assert!(volume_free(Path::new("C:\\")).unwrap_or(0) > 0);
    }
}
