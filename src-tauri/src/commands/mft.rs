//! 直接解析 NTFS 的 $MFT，一次性枚举全盘文件。
//!
//! # 为什么这么做
//!
//! 常规遍历要逐层 `read_dir`，每个目录都是一次独立的索引查询（随机读）。
//! 40 万个条目意味着几万次随机 IO。而 NTFS 的所有目录项都记录在 $MFT 里，
//! **顺序读一遍这块连续区域就能拿到全盘文件清单** —— 这是 WizTree 这类工具
//! 能在 1~2 秒扫完全盘的原因。
//!
//! # 前提与代价
//!
//! - **必须管理员权限**：打开卷设备 `\\.\C:` 需要 `GENERIC_READ`，标准用户会被拒绝。
//!   调用方必须准备好回退到普通遍历。
//! - **只支持 NTFS**：ReFS / exFAT / FAT32 上 `FSCTL_GET_NTFS_VOLUME_DATA` 会直接失败。
//! - 解析的是磁盘上的二进制结构，**任何一处偏移算错都不会报错**，只会算出离谱的数字。
//!   所以本模块所有读取都做边界检查，且必须用「与遍历结果逐项对比」来验证。
//!
//! # $MFT 记录结构（NTFS 3.1）
//!
//! ```text
//! 0x00  "FILE" 签名        0x14  第一个属性偏移
//! 0x04  USA 偏移           0x16  标志位 (0x01 使用中 / 0x02 目录)
//! 0x06  USA 数量           0x18  记录已用长度
//! 0x10  序列号             0x1C  记录分配长度
//! 0x12  硬链接数
//! ```
//!
//! 属性以 `类型(4) + 长度(4)` 开头，`0xFFFFFFFF` 表示结束。
//! 我们只关心两种：`$FILE_NAME`(0x30) 给父目录和名字，`$DATA`(0x80) 给真实大小。

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, SetFilePointerEx, FILE_BEGIN, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows_sys::Win32::System::IO::DeviceIoControl;
use std::ffi::c_void;

const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;
/// 根目录固定是 5 号记录
const ROOT_RECORD: u64 = 5;
/// 每次读入多少条记录（8M 块，避免一次把 400MB 的 $MFT 全读进内存）
const RECORDS_PER_CHUNK: usize = 8192;

/// 一条目录项。硬链接会让同一个文件产生多条（父目录 + 名字不同），
/// 与遍历口径保持一致：**按路径展开，重复计数**。
pub struct MftEntry {
    /// 本条目的 MFT 记录号（目录用它当父引用）
    pub id: u64,
    /// 父目录的记录号
    pub parent: u64,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// Unix 秒
    pub mtime: u64,
}

struct Volume {
    handle: HANDLE,
    bytes_per_record: u32,
    bytes_per_sector: u32,
    mft_start_offset: u64,
    mft_len: u64,
    total_bytes: u64,
    free_bytes: u64,
}

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_volume(letter: &str) -> Result<Volume, String> {
    let drive = letter.trim_end_matches(':').to_uppercase();
    let path = wstr(&format!("\\\\.\\{drive}:"));
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err("无法打开卷设备（需要管理员权限，且必须是 NTFS）".into());
    }

    let mut vd: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(),
            0,
            &mut vd as *mut _ as *mut c_void,
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        unsafe { CloseHandle(handle) };
        return Err("该卷不是 NTFS，或无法读取卷信息".into());
    }
    if vd.BytesPerFileRecordSegment == 0 || vd.BytesPerCluster == 0 || vd.MftValidDataLength <= 0 {
        unsafe { CloseHandle(handle) };
        return Err("卷信息异常（记录长度或簇大小为 0）".into());
    }

    Ok(Volume {
        handle,
        bytes_per_record: vd.BytesPerFileRecordSegment,
        bytes_per_sector: if vd.BytesPerSector == 0 { 512 } else { vd.BytesPerSector },
        mft_start_offset: vd.MftStartLcn as u64 * vd.BytesPerCluster as u64,
        mft_len: vd.MftValidDataLength as u64,
        total_bytes: vd.TotalClusters as u64 * vd.BytesPerCluster as u64,
        free_bytes: vd.FreeClusters as u64 * vd.BytesPerCluster as u64,
    })
}

/// 卷的总容量与已用字节数。
///
/// 用来给 MFT 结果做一次合理性校验：解析一旦写错偏移，算出来的总量会离谱到
/// 几个数量级，有这把尺子才能自己发现「我算错了」并退回遍历，而不是把错数字摆给用户。
pub fn volume_usage(letter: &str) -> Option<(u64, u64)> {
    let vol = open_volume(letter).ok()?;
    Some((
        vol.total_bytes.saturating_sub(vol.free_bytes),
        vol.total_bytes,
    ))
}

#[inline]
fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn u64le(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3],
        b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

/// 修掉 USA（Update Sequence Array）。
///
/// NTFS 会在每个扇区的最后两个字节写一个序号，用来检测扇区撕裂。
/// 真正的数据被挪到了记录头的 USA 数组里。不还原的话，属性数据末尾 2 字节是错的。
///
/// 返回 false 表示这条记录损坏（序号对不上），应当整条跳过。
fn apply_fixup(rec: &mut [u8], sector: usize) -> bool {
    if rec.len() < 8 {
        return false;
    }
    let usa_off = u16le(rec, 4) as usize;
    let usa_cnt = u16le(rec, 6) as usize;
    if usa_cnt == 0 || usa_off + usa_cnt * 2 > rec.len() {
        return false;
    }
    let usn = u16le(rec, usa_off);
    for i in 1..usa_cnt {
        let end = i * sector;
        if end < 2 || end > rec.len() {
            break;
        }
        let se = end - 2;
        if u16le(rec, se) != usn {
            return false;
        }
        let src = usa_off + i * 2;
        rec[se] = rec[src];
        rec[se + 1] = rec[src + 1];
    }
    true
}

fn filetime_to_unix(ft: u64) -> u64 {
    // FILETIME 起点是 1601-01-01，比 Unix 纪元早 11644473600 秒
    (ft / 10_000_000).saturating_sub(11_644_473_600)
}

/// 从一条记录里解析出所有目录项。
///
/// 一个文件可能有多个 `$FILE_NAME`（硬链接），按路径展开每个都算一条，
/// 与 `read_dir` 遍历的口径一致。`$FILE_NAME` 的 namespace 为 2 时是 8.3 短名，
/// 必须跳过，否则同一文件会在同一目录里出现两次。
fn parse_record(rec: &mut [u8], sector: usize, record_no: u64, out: &mut Vec<MftEntry>) {
    // 0~15 号是 NTFS 自己的元文件（$MFT / $Bitmap / $LogFile …）。
    // 它们对 Win32 枚举不可见，资源管理器里也看不到；如果放进来，
    // 总体积会比遍历结果凭空多出 1~2GB，用户还会看到一个叫 `$MFT` 的巨型文件。
    if record_no < 16 {
        return;
    }
    let len = rec.len();
    if len < 0x30 || &rec[0..4] != b"FILE" {
        return;
    }
    if !apply_fixup(rec, sector) {
        return;
    }

    let flags = u16le(rec, 0x16);
    if flags & 0x01 == 0 {
        return; // 记录未使用（已删除）
    }
    let is_dir = flags & 0x02 != 0;

    let first_attr = u16le(rec, 0x14) as usize;
    if first_attr >= len {
        return;
    }

    // 一条记录内的所有名字与大小
    let mut names: Vec<(u64, String, u64)> = Vec::new(); // (parent, name, mtime)
    let mut size: u64 = 0;

    let mut off = first_attr;
    let mut guard = 0usize;
    while off + 8 <= len && guard < 4096 {
        guard += 1;
        let atype = u32le(rec, off);
        if atype == ATTR_END {
            break;
        }
        let alen = u32le(rec, off + 4) as usize;
        if alen < 8 || off + alen > len {
            break;
        }

        match atype {
            // 属性头本身至少 0x18 字节，不够就跳过（不能读 off+0x10）
            ATTR_FILE_NAME if alen >= 0x18 => {
                let non_resident = rec[off + 8];
                if non_resident == 0 {
                    let content_len = u32le(rec, off + 0x10) as usize;
                    let content_off = u16le(rec, off + 0x14) as usize;
                    let s = off + content_off;
                    if content_len >= 0x42 && s + content_len <= len {
                        let c = &rec[s..s + content_len];
                        let parent = u64le(c, 0) & 0x0000_FFFF_FFFF_FFFF;
                        let mtime = filetime_to_unix(u64le(c, 0x10));
                        let name_len = c[0x40] as usize;
                        let ns = c[0x41];
                        // ns == 2 是 DOS 8.3 短名，跳过，否则同一文件会出现两次
                        if name_len > 0 && 0x42 + name_len * 2 <= c.len() && ns != 2 {
                            let units: Vec<u16> = (0..name_len)
                                .map(|i| u16::from_le_bytes([c[0x42 + i * 2], c[0x43 + i * 2]]))
                                .collect();
                            let name = String::from_utf16_lossy(&units);
                            if !name.is_empty() && name != "." {
                                names.push((parent, name, mtime));
                            }
                        }
                    }
                }
            }
            ATTR_DATA if alen >= 0x18 => {
                // 有名字的 $DATA 是备用数据流（ADS），不计入文件大小
                let name_len = rec[off + 9];
                if name_len == 0 {
                    let non_resident = rec[off + 8];
                    size = if non_resident == 1 {
                        // non-resident：real_size 在 0x30（逻辑大小，不是占用的簇数）
                        if alen >= 0x38 { u64le(rec, off + 0x30) } else { size }
                    } else {
                        // resident：内容长度就是大小
                        u32le(rec, off + 0x10) as u64
                    };
                }
            }
            _ => {}
        }
        off += alen;
    }

    for (parent, name, mtime) in names {
        out.push(MftEntry {
            // 记录号由读取位置推算，比记录头里的字段可靠（老版本 NTFS 没有那个字段）
            id: record_no,
            parent,
            name,
            size: if is_dir { 0 } else { size },
            is_dir,
            mtime,
        });
    }
}

/// 读取整盘的 MFT 并返回所有目录项。失败时调用方应回退到普通遍历。
pub fn read_entries(letter: &str) -> Result<Vec<MftEntry>, String> {
    let vol = open_volume(letter)?;
    let rec_size = vol.bytes_per_record as usize;
    let total = (vol.mft_len / rec_size as u64) as usize;
    if total == 0 {
        return Err("MFT 长度异常".into());
    }

    let mut out: Vec<MftEntry> = Vec::with_capacity(total);
    let mut buf = vec![0u8; rec_size * RECORDS_PER_CHUNK];

    let mut idx = 0usize;
    while idx < total {
        let want = (total - idx).min(RECORDS_PER_CHUNK);
        let byte_len = want * rec_size;
        let offset = vol.mft_start_offset + (idx as u64) * rec_size as u64;

        let ok = unsafe {
            SetFilePointerEx(vol.handle, offset as i64, std::ptr::null_mut(), FILE_BEGIN)
        };
        if ok == 0 {
            break;
        }
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                vol.handle,
                buf.as_mut_ptr(),
                byte_len as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }
        let read = read as usize;

        let mut p = 0usize;
        while p + rec_size <= read {
            let record_no = (idx + p / rec_size) as u64;
            parse_record(
                &mut buf[p..p + rec_size],
                vol.bytes_per_sector as usize,
                record_no,
                &mut out,
            );
            p += rec_size;
        }
        idx += want;

        // 让前端的进度条动起来：MFT 只有一次「读完」的那一刻，
        // 不喂进度的话界面会在 0% 上停住一两秒，看起来像卡死。
        super::scan::ENTRIES.fetch_add(want as u64, std::sync::atomic::Ordering::Relaxed);
        if super::scan::cancelled() {
            break;
        }
    }

    if out.is_empty() {
        return Err("MFT 中未解析出任何目录项".into());
    }
    Ok(out)
}

/// 根目录的记录号（供上层建树用）
pub const fn root_id() -> u64 {
    ROOT_RECORD
}
