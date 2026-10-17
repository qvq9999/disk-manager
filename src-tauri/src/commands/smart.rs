//! SMART 直读（对齐 CrystalDiskInfo：主通道 + 多路独立属性源交叉校验）
//!
//! # 取数策略
//!
//! **主通道**（先用 `STORAGE_DEVICE_DESCRIPTOR` 判 BusType 再选，避免对不匹配的盘白发 IOCTL）
//!   ① NVMe : `IOCTL_STORAGE_QUERY_PROPERTY` + `StorageDeviceProtocolSpecificProperty`
//!            `NVMeDataTypeLogPage` / `LID=0x02 (SMART & Health)` / **NSID = 0xFFFFFFFF**
//!   ② ATA  : `IOCTL_ATA_PASS_THROUGH_DIRECT` → `IOCTL_ATA_PASS_THROUGH` → `SMART_RCV_DRIVE_DATA`
//!            `SMART ENABLE(0xD8)` + `SMART READ DATA(0xD0)` + `SMART READ THRESHOLDS(0xD1)`
//!            + `IDENTIFY DEVICE(0xEC)`
//!
//! **元数据补全**（主通道失败时也能拿到盘的身份信息）
//!   • `StorageDeviceProperty(0)`                → 厂商 / 型号 / 序列号 / **总线类型**
//!   • `StorageAdapterSerialNumberProperty(57)`  → 序列号（部分 RAID 控制器）
//!
//! 所有 `STORAGE_PROPERTY_ID` 数值均核对自本机 `windows-sys` 源码。
//!
//! # 历史踩坑记录（都已修正，不要改回去）
//!
//! 1. **`StorageDeviceProtocolSpecificProperty` 是 50，不是 49**。
//!    49 是 `StorageAdapterProtocolSpecificProperty`（RAID 适配器专用）。
//!    用 49 查 NVMe 协议数据会被驱动拒绝 —— 这是 M.2「大量参数读不到」的首要原因。
//! 2. **`STORAGE_PROTOCOL_SPECIFIC_DATA` 是 10 个 u32 = 40 字节**（含 `…SubValue4`）。
//!    按 9 个字段（36 字节）构造会让 `ProtocolDataOffset` 与真实数据区错位 4 字节。
//! 3. **`IOCTL_STORAGE_QUERY_PROPERTY` = 0x002D1400**
//!    （`CTL_CODE(0x2D, 0x0500, METHOD_BUFFERED, FILE_ANY_ACCESS)`，
//!     已核对 windows-sys `IOCTL_STORAGE_QUERY_PROPERTY = 2954240`）。
//!    不要“顺手”改成 0x2D1403 —— 那是错的。
//! 4. **NVMe SMART/Health Log 的 128 位计数器从 byte 32 开始**，间隔 16 字节：
//!    `32=读量 / 48=写量 / 64=主机读命令 / 80=主机写命令 / 96=控制器忙碌 /
//!     112=通电次数 / 128=通电小时 / 144=异常掉电 / 160=介质错误 / 176=错误日志条数`。
//!    6..31 是 Endurance Group Critical Warning Summary + 保留区，**不是计数器**。
//! 5. **SMART/Health Log 的 NSID 必须是 `0xFFFFFFFF`**（全部命名空间）；
//!    而 **Identify Controller 的 NSID 必须是 0**。两者不能混用。
//! 6. **数据起始位置要按驱动回填的 `ProtocolDataOffset`** 计算，不能写死 `8+36`。
//! 7. `STORAGE_DEVICE_DESCRIPTOR` 的偏移：`Vendor@12 / Product@16 / Serial@24 / BusType@28`
//!    （中间有 4 个 1 字节字段，容易误算成 16/20/28/32）。
//! 8. `STORAGE_ADAPTER_SERIAL_NUMBER.SerialNumber` 是**内联 WCHAR[128]**，不是偏移字段。
//! 9. `ATA_PASS_THROUGH_EX` 在 `TimeOutValue` 之后还有 `ReservedAsUlong`，
//!    x64 下 `sizeof == 48`；漏了它驱动按 `Length` 校验失败 → ATA 通道全废。
//!    已用编译期断言锁死。
//! 10. `SMART READ DATA(0xD0)` 的属性块里**没有阈值**（byte[1..3] 是 Status Flags）。
//!     阈值必须单独发 `SMART READ THRESHOLDS(0xD1)` 再按 ID 合并。

use serde::Serialize;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

// ---------------------------------------------------------------- IOCTL 常量

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// `CTL_CODE(IOCTL_SCSI_BASE=0x04, 0x040C, METHOD_BUFFERED, READ|WRITE)` = 0x0004D030
const IOCTL_ATA_PASS_THROUGH_DIRECT: u32 = 0x0004_D030;
/// `CTL_CODE(IOCTL_SCSI_BASE=0x04, 0x040B, METHOD_BUFFERED, READ|WRITE)` = 0x0004D02C
const IOCTL_ATA_PASS_THROUGH: u32 = 0x0004_D02C;
/// 旧式 SMART 通道（CrystalDiskInfo 的回退路径）= 0x0007C088
const IOCTL_SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;
/// `CTL_CODE(FILE_DEVICE_MASS_STORAGE=0x2D, 0x0500, METHOD_BUFFERED, FILE_ANY_ACCESS)`
/// = **0x002D1400**（windows-sys `IOCTL_STORAGE_QUERY_PROPERTY = 2954240`
///   = 0x2D1400，已核对 `System/Ioctl/mod.rs`）
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
/// `CTL_CODE(IOCTL_DISK_BASE=7, 0x0017, BUFFERED, READ)` = 0x0007405C
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;

// ------------------------------------------------------- STORAGE_PROPERTY_ID
//
// 数值全部核对自 windows-sys `System/Ioctl/mod.rs`（`STORAGE_PROPERTY_ID` 枚举）。
// ⚠️ 别凭记忆写：`StorageDeviceProtocolSpecificProperty` 是 **50**，
//    49 是 `StorageAdapterProtocolSpecificProperty`（RAID 适配器用）。
//    用 49 查 NVMe 协议数据会被驱动拒绝 —— 这正是 M.2「读不到」的根因之一。

const PROP_STORAGE_DEVICE: u32 = 0; // StorageDeviceProperty
const PROP_DEVICE_PROTOCOL_SPECIFIC: u32 = 50; // StorageDeviceProtocolSpecificProperty
const PROP_ADAPTER_SERIAL_NUMBER: u32 = 57; // StorageAdapterSerialNumberProperty

const QUERY_STANDARD: u32 = 0; // PropertyStandardQuery

// ------------------------------------------------- STORAGE_PROTOCOL_* 常量

const PROTOCOL_TYPE_NVME: u32 = 3;
const NVME_DATA_TYPE_IDENTIFY: u32 = 1;
const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
/// SMART/Health Log 与 Identify 都必须用「全部命名空间」
const NVME_NSID_ALL: u32 = 0xFFFF_FFFF;
const NVME_LOG_SMART_HEALTH: u32 = 0x02;
const NVME_CNS_IDENTIFY_CONTROLLER: u32 = 0x01;

// ---------------------------------------------------------- STORAGE_BUS_TYPE

const BUS_TYPE_UNKNOWN: u32 = 0x00;
const BUS_TYPE_SCSI: u32 = 0x01;
const BUS_TYPE_ATA: u32 = 0x03;
const BUS_TYPE_VIRTUAL: u32 = 0x08;
const BUS_TYPE_NVME: u32 = 0x0B;
const BUS_TYPE_RAID: u32 = 0x0C;
const BUS_TYPE_SAS: u32 = 0x0E;
const BUS_TYPE_SATA: u32 = 0x0F;
const BUS_TYPE_USB: u32 = 0x10;
const BUS_TYPE_MMC: u32 = 0x15;
const BUS_TYPE_SD: u32 = 0x16;

fn bus_label(b: u32) -> String {
    let s = match b {
        BUS_TYPE_SCSI => "SCSI",
        BUS_TYPE_ATA => "ATA",
        BUS_TYPE_VIRTUAL => "虚拟磁盘",
        BUS_TYPE_NVME => "NVMe",
        BUS_TYPE_RAID => "RAID",
        BUS_TYPE_SAS => "SAS",
        BUS_TYPE_SATA => "SATA",
        BUS_TYPE_USB => "USB",
        BUS_TYPE_MMC => "MMC/SD",
        BUS_TYPE_SD => "SD",
        BUS_TYPE_UNKNOWN => "",
        _ => "其他",
    };
    s.to_string()
}

// ------------------------------------------------------------------ ATA 常量

const ATA_FLAGS_DATA_IN: u16 = 0x02;
const ATA_FLAGS_DRDY_REQUIRED: u16 = 0x01;

const ATA_SMART: u8 = 0xB0;
const SMART_READ_DATA: u8 = 0xD0;
const SMART_READ_THRESHOLDS: u8 = 0xD1;
const SMART_ENABLE: u8 = 0xD8;
const ATA_IDENTIFY: u8 = 0xEC;

// ------------------------------------------------------------- 数据结构定义

/// ATA SMART 属性（CrystalDiskInfo 属性表的行）
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub current: u8,
    pub worst: u8,
    pub threshold: u8,
    pub raw: u64,
}

/// NVMe 健康项（CDI 对 NVMe 盘就是渲染这张列表，而不是 ATA 式属性表）
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthItem {
    /// 中文项名
    pub label: String,
    /// 已格式化的值
    pub value: String,
    /// 0=正常 1=注意 2=异常 3=信息
    pub level: u8,
    /// 底层原始数值（可选，便于排查）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<f64>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SmartInfo {
    pub drive: String,
    /// ATA / NVMe / 未知
    pub kind: String,
    pub temperature: Option<i32>,
    pub critical_temperature: Option<i32>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub wear: Option<u32>,
    pub reallocated_sectors: Option<u64>,
    pub pending_sectors: Option<u64>,
    pub total_written_bytes: Option<u64>,
    pub total_read_bytes: Option<u64>,
    pub available_spare: Option<u32>,
    pub used_spare_threshold: Option<u32>,
    pub critical_warning: Option<u32>,
    pub controller_busy_minutes: Option<u64>,
    pub unsafe_shutdowns: Option<u64>,
    /// IDENTIFY DEVICE / NVMe Identify / STORAGE_DEVICE_DESCRIPTOR 补全的元数据
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    pub vendor: Option<String>,
    /// 总线类型（来自 STORAGE_DEVICE_DESCRIPTOR，最准）
    pub bus: Option<String>,
    pub rotation_rate: Option<u32>,
    /// 传输模式：SATA/600、SATA/300、NVMe ...
    pub transfer_mode: Option<String>,
    pub host_read_commands: Option<u64>,
    pub host_write_commands: Option<u64>,
    /// 是否读到了阈值表（0xD1）；未读到时属性表的 threshold 不可信
    pub has_thresholds: bool,
    /// ATA 属性表
    pub attributes: Vec<SmartAttribute>,
    /// NVMe 健康项列表
    pub health_items: Vec<HealthItem>,
    pub predict_failure: Option<bool>,
    /// 数据来源（可能包含多路拼接说明）
    pub source: String,
    /// 失败原因 / 诊断信息
    pub message: String,
}

/// `ATA_PASS_THROUGH_DIRECT`（x64 = 48 字节）
#[repr(C)]
#[derive(Clone, Copy)]
struct AtaPassThroughDirect {
    length: u16,
    ata_flags: u16,
    path_id: u8,
    target_id: u8,
    lun: u8,
    reserved_as_uch: u8,
    data_transfer_length: u32,
    timeout_value: u32,
    // ⚠️ 漏掉这个字段 → 后续成员整体前移 8 字节，驱动按 Length 校验失败
    reserved_as_ulong: u32,
    data_buffer: *mut core::ffi::c_void,
    previous_task_file: [u8; 8],
    current_task_file: [u8; 8],
}

/// `ATA_PASS_THROUGH_EX`（x64 = 48 字节，数据用偏移寻址）
#[repr(C)]
#[derive(Clone, Copy)]
struct AtaPassThroughBuffered {
    length: u16,
    ata_flags: u16,
    path_id: u8,
    target_id: u8,
    lun: u8,
    reserved_as_uch: u8,
    data_transfer_length: u32,
    timeout_value: u32,
    reserved_as_ulong: u32,
    data_buffer_offset: usize,
    previous_task_file: [u8; 8],
    current_task_file: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IdeRegs {
    features: u8,
    sector_count: u8,
    sector_number: u8,
    cyl_low: u8,
    cyl_high: u8,
    drive_head: u8,
    command: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SendCmdInParams {
    c_buf_size: u32,
    ir_drive_regs: IdeRegs,
    drive_number: u8,
    reserved: [u8; 3],
    dw_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DriverStatus {
    driver_error: u8,
    ide_error: u8,
    reserved: [u8; 2],
    dw_reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SendCmdOutParams {
    c_buf_size: u32,
    driver_status: DriverStatus,
    buffer: [u8; 512],
}

/// `STORAGE_PROPERTY_QUERY`
#[repr(C)]
#[derive(Clone, Copy)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional_parameters: [u8; 1],
}

/// `STORAGE_PROTOCOL_SPECIFIC_DATA`（**10 × u32 = 40 字节**）
///
/// ⚠️ 共 10 个字段（含 `ProtocolDataRequestSubValue4`）。漏掉它会让整个结构体少 4 字节，
///    输入缓冲区里的数据区偏移随之错位 → 驱动按 `ProtocolDataOffset` 读到垃圾。
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StorageProtocolSpecificData {
    protocol_type: u32,
    data_type: u32,
    protocol_data_request_value: u32,
    /// 对 NVMe 就是 **NSID**
    protocol_data_request_sub_value: u32,
    /// 请求侧填 40（本结构体大小）；返回后驱动会**回填真实的数据偏移**，必须按它取数
    protocol_data_offset: u32,
    protocol_data_length: u32,
    fixed_protocol_return_data: u32,
    protocol_data_request_sub_value2: u32,
    protocol_data_request_sub_value3: u32,
    protocol_data_request_sub_value4: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StoragePropertyQueryWithProtocol {
    property_id: u32,
    query_type: u32,
    protocol: StorageProtocolSpecificData,
}

// 编译期锁死布局：x64 下这几个尺寸错了就说明结构体被改坏了
const _: () = {
    assert!(std::mem::size_of::<AtaPassThroughDirect>() == 48);
    assert!(std::mem::size_of::<AtaPassThroughBuffered>() == 48);
    assert!(std::mem::size_of::<StoragePropertyQuery>() == 12);
    assert!(std::mem::size_of::<StorageProtocolSpecificData>() == 40);
    assert!(std::mem::size_of::<StoragePropertyQueryWithProtocol>() == 48);
};

/// `STORAGE_PROTOCOL_DATA_DESCRIPTOR` 固定 8 字节（Version + DataLength）
const PROTOCOL_DATA_DESCRIPTOR_SIZE: usize = 8;

// ------------------------------------------------------------- 基础工具函数

fn open_drive(n: u32) -> Option<isize> {
    let path = format!("\\\\.\\PhysicalDrive{n}");
    let wide: Vec<u16> = std::ffi::OsStr::new(&path).encode_wide().chain(Some(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0 as _,
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        None
    } else {
        Some(h as isize)
    }
}

fn is_le_zero(buf: &[u8]) -> bool {
    buf.iter().all(|&b| b == 0)
}

fn le_u64(raw: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    let n = raw.len().min(8);
    b[..n].copy_from_slice(&raw[..n]);
    u64::from_le_bytes(b)
}

fn le_u128(bytes: &[u8]) -> u128 {
    let mut b = [0u8; 16];
    let n = bytes.len().min(16);
    b[..n].copy_from_slice(&bytes[..n]);
    u128::from_le_bytes(b)
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    if off + 4 > b.len() {
        return None;
    }
    Some(u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]))
}

/// 从 NUL 结尾的字节串取文本（STORAGE_*_PROPERTY 里都是这种）
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn human_bytes(n: u64) -> String {
    const UNITS: [(&str, u64); 5] = [
        ("PB", 1u64 << 50),
        ("TB", 1u64 << 40),
        ("GB", 1u64 << 30),
        ("MB", 1u64 << 20),
        ("KB", 1u64 << 10),
    ];
    for (s, m) in UNITS {
        if n >= m {
            return format!("{:.2} {}", n as f64 / m as f64, s);
        }
    }
    format!("{} B", n)
}

// ------------------------------------------------ 通用属性查询（交叉校验源）

/// `IOCTL_STORAGE_QUERY_PROPERTY` + `PropertyStandardQuery`
fn storage_property(h: isize, property_id: u32, out_len: u32) -> Option<Vec<u8>> {
    if out_len == 0 {
        return None;
    }
    let mut buf = vec![0u8; out_len as usize];
    let q = StoragePropertyQuery {
        property_id,
        query_type: QUERY_STANDARD,
        additional_parameters: [0],
    };
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            out_len,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(buf)
}

/// `STORAGE_DEVICE_DESCRIPTOR` → (vendor, product, serial, bus_type)
///
/// 布局（已核对 windows-sys）：
/// `Version@0 Size@4 DeviceType@8 DeviceTypeModifier@9 RemovableMedia@10 CommandQueueing@11
///  VendorIdOffset@12 ProductIdOffset@16 ProductRevisionOffset@20 SerialNumberOffset@24
///  BusType@28 RawPropertiesLength@32`
fn storage_device_descriptor(
    h: isize,
) -> Option<(Option<String>, Option<String>, Option<String>, Option<u32>)> {
    let b = storage_property(h, PROP_STORAGE_DEVICE, 1024)?;
    if b.len() < 36 {
        return None;
    }
    let pick = |off: Option<u32>| -> Option<String> {
        let o = off? as usize;
        if o == 0 || o >= b.len() {
            return None;
        }
        let s = cstr(&b[o..]);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    Some((
        pick(u32_at(&b, 12)),
        pick(u32_at(&b, 16)),
        pick(u32_at(&b, 24)),
        u32_at(&b, 28),
    ))
}

/// `STORAGE_ADAPTER_SERIAL_NUMBER` → 序列号
///
/// 布局：`Version@0 Size@4 SerialNumber[WCHAR;128]@8`（**内联数组，不是偏移**）
fn property_adapter_serial(h: isize) -> Option<String> {
    let b = storage_property(h, PROP_ADAPTER_SERIAL_NUMBER, 512)?;
    if b.len() < 16 {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    let mut i = 8usize;
    while i + 1 < b.len() {
        let u = u16::from_le_bytes([b[i], b[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    let s = String::from_utf16_lossy(&units).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ------------------------------------------------------------- ATA 命令发送

/// SMART 子命令任务文件：
/// `Features / SectorCount / SectorNumber / CylLow(4F) / CylHigh(C2) / DriveHead(A0) / Command(B0) / Reserved`
#[inline]
fn smart_regs(feature: u8) -> [u8; 8] {
    [feature, 0x01, 0x01, 0x4F, 0xC2, 0xA0, ATA_SMART, 0x00]
}

#[inline]
fn smart_enable_regs() -> [u8; 8] {
    [SMART_ENABLE, 0x00, 0x00, 0x4F, 0xC2, 0xA0, ATA_SMART, 0x00]
}

#[inline]
fn identify_regs() -> [u8; 8] {
    [0x00, 0x00, 0x00, 0x00, 0x00, 0xA0, ATA_IDENTIFY, 0x00]
}

/// ① `IOCTL_ATA_PASS_THROUGH_DIRECT`
fn ata_direct(h: isize, regs: [u8; 8], len: u32) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    let words = ((len as usize) + 7) / 8;
    let mut data = vec![0u64; words];
    let sz = std::mem::size_of::<AtaPassThroughDirect>() as u32;
    let mut apt = AtaPassThroughDirect {
        length: sz as u16,
        ata_flags: ATA_FLAGS_DATA_IN | ATA_FLAGS_DRDY_REQUIRED,
        path_id: 0,
        target_id: 0,
        lun: 0,
        reserved_as_uch: 0,
        data_transfer_length: len,
        timeout_value: 30,
        reserved_as_ulong: 0,
        data_buffer: data.as_mut_ptr() as *mut core::ffi::c_void,
        previous_task_file: [0; 8],
        current_task_file: regs,
    };
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_ATA_PASS_THROUGH_DIRECT,
            &apt as *const _ as *const core::ffi::c_void,
            sz,
            &mut apt as *mut _ as *mut core::ffi::c_void,
            sz,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, len as usize) }.to_vec())
}

/// ② `IOCTL_ATA_PASS_THROUGH`（数据放在缓冲区尾部）
fn ata_buffered(h: isize, regs: [u8; 8], len: u32) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    let sz = std::mem::size_of::<AtaPassThroughBuffered>();
    let mut buf = vec![0u8; sz + len as usize];
    let apt = AtaPassThroughBuffered {
        length: sz as u16,
        ata_flags: ATA_FLAGS_DATA_IN | ATA_FLAGS_DRDY_REQUIRED,
        path_id: 0,
        target_id: 0,
        lun: 0,
        reserved_as_uch: 0,
        data_transfer_length: len,
        timeout_value: 30,
        reserved_as_ulong: 0,
        data_buffer_offset: sz,
        previous_task_file: [0; 8],
        current_task_file: regs,
    };
    unsafe {
        std::ptr::copy_nonoverlapping(&apt as *const _ as *const u8, buf.as_mut_ptr(), sz);
    }
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_ATA_PASS_THROUGH,
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(buf[sz..sz + len as usize].to_vec())
}

/// ③ 旧式 `SMART_RCV_DRIVE_DATA`（`SENDCMDINPARAMS` / `SENDCMDOUTPARAMS`）
fn ata_legacy(h: isize, regs: [u8; 8], len: u32) -> Option<Vec<u8>> {
    if len == 0 || len > 512 {
        return None;
    }
    let inbuf = SendCmdInParams {
        c_buf_size: 512,
        ir_drive_regs: IdeRegs {
            features: regs[0],
            sector_count: regs[1],
            sector_number: regs[2],
            cyl_low: regs[3],
            cyl_high: regs[4],
            drive_head: regs[5],
            command: regs[6],
            reserved: 0,
        },
        drive_number: 0,
        reserved: [0; 3],
        dw_reserved: [0; 4],
    };
    let mut out = SendCmdOutParams {
        c_buf_size: 512,
        driver_status: DriverStatus::default(),
        buffer: [0u8; 512],
    };
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_SMART_RCV_DRIVE_DATA,
            &inbuf as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<SendCmdInParams>() as u32,
            &mut out as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<SendCmdOutParams>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || out.driver_status.driver_error != 0 {
        return None;
    }
    Some(out.buffer[..len as usize].to_vec())
}

/// 依次尝试三条 ATA 通道
fn ata_read(h: isize, regs: [u8; 8], len: u32) -> Option<Vec<u8>> {
    ata_direct(h, regs, len)
        .or_else(|| ata_buffered(h, regs, len))
        .or_else(|| ata_legacy(h, regs, len))
}

/// 无数据返回的 ATA 命令（`SMART ENABLE` 等）：仅用 DIRECT 通道
fn ata_cmd_no_data(h: isize, regs: [u8; 8]) -> bool {
    let sz = std::mem::size_of::<AtaPassThroughDirect>() as u32;
    let mut apt = AtaPassThroughDirect {
        length: sz as u16,
        ata_flags: ATA_FLAGS_DRDY_REQUIRED,
        path_id: 0,
        target_id: 0,
        lun: 0,
        reserved_as_uch: 0,
        data_transfer_length: 0,
        timeout_value: 30,
        reserved_as_ulong: 0,
        data_buffer: std::ptr::null_mut(),
        previous_task_file: [0; 8],
        current_task_file: regs,
    };
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_ATA_PASS_THROUGH_DIRECT,
            &apt as *const _ as *const core::ffi::c_void,
            sz,
            &mut apt as *mut _ as *mut core::ffi::c_void,
            sz,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    ok != 0
}

// --------------------------------------------------------------- NVMe 取数

/// `StorageDeviceProtocolSpecificProperty` 通用查询
///
/// `data_type`: 1 = Identify，2 = LogPage
/// `request_value`: CNS（Identify）或 LID（LogPage）
/// `nsid`: **SMART/Health Log 必须传 `0xFFFFFFFF`**
///
/// 取数位置按驱动**回填的 `ProtocolDataOffset`** 计算，不再写死 `8 + 36`。
fn nvme_query(h: isize, data_type: u32, request_value: u32, nsid: u32, len: u32) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    let proto_size = std::mem::size_of::<StorageProtocolSpecificData>() as u32; // 40
    let head = std::mem::size_of::<StoragePropertyQueryWithProtocol>() as u32; // 48
    let mut buf = vec![0u8; (head + len) as usize];
    let query = StoragePropertyQueryWithProtocol {
        property_id: PROP_DEVICE_PROTOCOL_SPECIFIC,
        query_type: QUERY_STANDARD,
        protocol: StorageProtocolSpecificData {
            protocol_type: PROTOCOL_TYPE_NVME,
            data_type,
            protocol_data_request_value: request_value,
            protocol_data_request_sub_value: nsid,
            protocol_data_offset: proto_size,
            protocol_data_length: len,
            fixed_protocol_return_data: 0,
            protocol_data_request_sub_value2: 0,
            protocol_data_request_sub_value3: 0,
            protocol_data_request_sub_value4: 0,
        },
    };
    unsafe {
        std::ptr::copy_nonoverlapping(
            &query as *const _ as *const u8,
            buf.as_mut_ptr(),
            head as usize,
        );
    }
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_STORAGE_QUERY_PROPERTY,
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let d = PROTOCOL_DATA_DESCRIPTOR_SIZE;
    if buf.len() < d + 24 {
        return None;
    }
    // 驱动回填的真实数据偏移（相对 STORAGE_PROTOCOL_SPECIFIC_DATA 起点，该字段位于 +16）
    let ret_off =
        u32::from_le_bytes([buf[d + 16], buf[d + 17], buf[d + 18], buf[d + 19]]) as usize;
    let ret_off = if ret_off == 0 { proto_size as usize } else { ret_off };
    let start = d + ret_off;
    if start >= buf.len() {
        return None;
    }
    let avail = (buf.len() - start).min(len as usize);
    if avail == 0 {
        return None;
    }
    let data = buf[start..start + avail].to_vec();
    if is_le_zero(&data) {
        return None;
    }
    Some(data)
}

fn nvme_smart_log(h: isize) -> Option<Vec<u8>> {
    nvme_query(h, NVME_DATA_TYPE_LOG_PAGE, NVME_LOG_SMART_HEALTH, NVME_NSID_ALL, 512)
}

fn nvme_identify(h: isize) -> Option<Vec<u8>> {
    // Identify Controller（CNS=01h）的 NSID 按规范应为 0（SMART 日志页才用 0xFFFFFFFF）
    nvme_query(h, NVME_DATA_TYPE_IDENTIFY, NVME_CNS_IDENTIFY_CONTROLLER, 0, 4096)
}

// ------------------------------------------------------------------ 解析层

/// 取单个 ATA 属性：(current, worst, raw[6])
fn smart_attr(data: &[u8], id: u8) -> Option<(u8, u8, [u8; 6])> {
    let mut i = 2usize;
    while i + 12 <= data.len().min(362) {
        if data[i] == id {
            let mut raw = [0u8; 6];
            raw.copy_from_slice(&data[i + 5..i + 11]);
            return Some((data[i + 3], data[i + 4], raw));
        }
        i += 12;
    }
    None
}

/// SMART 属性 ID → 名称（覆盖 CrystalDiskInfo 常见条目）
fn attr_name(id: u8) -> &'static str {
    match id {
        1 => "底层读取错误率",
        2 => "吞吐性能",
        3 => "主轴起转时间",
        4 => "启停次数",
        5 => "重分配扇区数",
        6 => "读取通道余量",
        7 => "寻道错误率",
        8 => "寻道时间性能",
        9 => "通电时间（小时）",
        10 => "主轴重试次数",
        11 => "校准重试次数",
        12 => "通电次数",
        13 => "软读取错误率",
        22 => "氦气液位",
        170 => "坏块计数",
        171 => "可编程失败块",
        172 => "擦除失败块",
        173 => "平均擦除次数",
        174 => "意外掉电计数",
        175 => "最大擦除次数",
        176 => "擦除失败计数",
        177 => "磨损范围",
        178 => "已用备用块",
        179 => "已用备用块（合计）",
        180 => "不可用保留块",
        181 => "可编程失败总数",
        182 => "擦除失败总数",
        183 => "SATA 降速计数",
        184 => "端到端错误",
        185 => "磁头稳定性",
        186 => "飞行高度",
        187 => "无法修正的错误",
        188 => "指令超时",
        189 => "高低温剩余",
        190 => "气流温度",
        191 => "冲击错误率",
        192 => "紧急断电次数",
        193 => "磁头加载/卸载",
        194 => "温度",
        195 => "硬件 ECC 恢复",
        196 => "重分配事件数",
        197 => "待映射扇区数",
        198 => "无法修正的扇区数",
        199 => "接口 CRC 错误",
        200 => "多区域错误率",
        201 => "软读取错误率",
        202 => "厂商特定（磨损）",
        203 => "运行中超时",
        204 => "软 ECC 修正",
        205 => "热 asperity",
        206 => "飞行高度",
        207 => "主轴高电流",
        208 => "主轴离线",
        209 => "离线寻道性能",
        220 => "磁盘位移/冲击",
        221 => "加电时间",
        222 => "加载/卸载重试",
        223 => "加载摩擦",
        224 => "加载摩擦计数",
        225 => "主机写入量",
        226 => "主机读取量",
        227 => "扭矩放大计数",
        228 => "掉电保护失败",
        230 => "磁头寿命",
        231 => "SSD 剩余寿命",
        232 => "可用保留空间",
        233 => "介质磨损指示",
        234 => "平均擦除次数",
        235 => "剩余寿命（厂商）",
        240 => "磁头飞行时间",
        241 => "总写入量（LBA）",
        242 => "总读取量（LBA）",
        243 => "累计通电时间",
        244 => "当前温度（厂商）",
        245 => "最差温度",
        250 => "读取错误重试率",
        _ => "厂商特定属性",
    }
}

/// `SMART READ THRESHOLDS (0xD1)` → ID → 阈值
fn parse_thresholds(data: &[u8]) -> std::collections::HashMap<u8, u8> {
    let mut map = std::collections::HashMap::new();
    let mut i = 2usize;
    while i + 12 <= data.len().min(362) {
        let id = data[i];
        if id != 0 {
            map.insert(id, data[i + 1]);
        }
        i += 12;
    }
    map
}

fn parse_ata(data: &[u8], thresholds: &std::collections::HashMap<u8, u8>) -> SmartInfo {
    let mut s = SmartInfo {
        kind: "ATA".into(),
        source: "SMART直读(ATA)".into(),
        has_thresholds: !thresholds.is_empty(),
        ..Default::default()
    };
    let mut i = 2usize;
    while i + 12 <= data.len().min(362) {
        let id = data[i];
        if id == 0 {
            i += 12;
            continue;
        }
        let mut raw6 = [0u8; 6];
        raw6.copy_from_slice(&data[i + 5..i + 11]);
        s.attributes.push(SmartAttribute {
            id,
            name: attr_name(id).to_string(),
            current: data[i + 3],
            worst: data[i + 4],
            threshold: *thresholds.get(&id).unwrap_or(&0),
            raw: le_u64(&raw6),
        });
        i += 12;
    }
    if let Some((_, _, raw)) = smart_attr(data, 9) {
        s.power_on_hours = Some(le_u64(&raw[..4]));
    }
    if let Some((_, _, raw)) = smart_attr(data, 194).or_else(|| smart_attr(data, 190)) {
        let t = raw[0] as i32;
        if t > 0 && t < 120 {
            s.temperature = Some(t);
        }
    }
    if let Some((_, _, raw)) = smart_attr(data, 5) {
        s.reallocated_sectors = Some(le_u64(&raw));
    }
    if let Some((_, _, raw)) = smart_attr(data, 197) {
        s.pending_sectors = Some(le_u64(&raw));
    }
    if let Some((_, _, raw)) = smart_attr(data, 12) {
        s.power_cycles = Some(le_u64(&raw[..4]));
    }
    // 241/242 = 总读写 LBA（×512）；225/226 = 主机读写量（单位 32MB）兜底
    if let Some((_, _, raw)) = smart_attr(data, 241) {
        s.total_written_bytes = Some(le_u64(&raw).saturating_mul(512));
    } else if let Some((_, _, raw)) = smart_attr(data, 225) {
        s.total_written_bytes = Some(le_u64(&raw).saturating_mul(32 * 1024 * 1024));
    }
    if let Some((_, _, raw)) = smart_attr(data, 242) {
        s.total_read_bytes = Some(le_u64(&raw).saturating_mul(512));
    } else if let Some((_, _, raw)) = smart_attr(data, 226) {
        s.total_read_bytes = Some(le_u64(&raw).saturating_mul(32 * 1024 * 1024));
    }
    // 剩余寿命：231 标准化值 = 剩余%；202/177 为厂商磨损
    for id in [231u8, 202, 177] {
        if let Some((v, _, _)) = smart_attr(data, id) {
            if v > 0 && v <= 100 {
                s.wear = Some((100 - v as u32).min(100));
                break;
            }
        }
    }
    let bad = s.reallocated_sectors.unwrap_or(0) + s.pending_sectors.unwrap_or(0);
    s.predict_failure = Some(bad > 0);
    s
}

/// ATA 字符串：按 ATA 规范每 2 字节交换
fn ata_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for c in bytes.chunks(2) {
        if c.len() == 2 {
            out.push(c[1] as char);
            out.push(c[0] as char);
        } else {
            out.push(c[0] as char);
        }
    }
    out.replace('\0', " ").trim().to_string()
}

fn parse_identify(data: &[u8], s: &mut SmartInfo) {
    if data.len() < 512 {
        return;
    }
    let serial = ata_string(&data[20..40]);
    let firmware = ata_string(&data[46..54]);
    let model = ata_string(&data[54..94]);
    if !serial.is_empty() {
        s.serial = Some(serial);
    }
    if !firmware.is_empty() {
        s.firmware = Some(firmware);
    }
    if !model.is_empty() {
        s.model = Some(model);
    }
    // word 217 = 转速（0x0001 = SSD / 非旋转）
    let rpm = u16::from_le_bytes([data[217 * 2], data[217 * 2 + 1]]);
    if rpm > 0 {
        s.rotation_rate = Some(rpm as u32);
    }
    // word 76 = 当前协商速率：bit1=Gen1 / bit2=Gen2 / bit3=Gen3
    let cap = u16::from_le_bytes([data[76 * 2], data[76 * 2 + 1]]);
    if cap & 0x000E != 0 {
        let mode = if cap & 0x0008 != 0 {
            "SATA/600"
        } else if cap & 0x0004 != 0 {
            "SATA/300"
        } else {
            "SATA/150"
        };
        s.transfer_mode = Some(mode.to_string());
    }
}

/// NVMe SMART/Health Log (LID 0x02) 解析
/// ⚠️ 128 位计数器**紧凑排列**，起点是 byte 6。旧代码整体错位 +26 字节。
fn parse_nvme(data: &[u8]) -> SmartInfo {
    let mut s = SmartInfo {
        kind: "NVMe".into(),
        source: "SMART直读(NVMe日志页)".into(),
        transfer_mode: Some("NVMe".into()),
        rotation_rate: Some(0),
        ..Default::default()
    };
    if data.len() < 192 {
        s.message = "NVMe SMART 日志长度不足".into();
        return s;
    }

    // byte 0 临界警告位（bit0 备用空间 / bit1 温度 / bit2 可靠性 / bit3 只读 / bit4 掉电保护）
    let critical = data[0];
    s.critical_warning = Some(critical as u32);
    // byte 1-2 复合温度（Kelvin）
    let temp_k = u16::from_le_bytes([data[1], data[2]]);
    if temp_k > 200 && temp_k < 500 {
        s.temperature = Some(temp_k as i32 - 273);
    }
    // byte 3 备用空间，byte 4 备用空间阈值，byte 5 已用寿命百分比
    let spare = data[3];
    let spare_thr = data[4];
    let used = data[5];
    s.available_spare = Some(spare as u32);
    s.used_spare_threshold = Some(spare_thr as u32);
    if used <= 100 {
        s.wear = Some(used as u32);
    }

    // ---- NVMe SMART/Health Log（LID 02h）官方字段偏移 ----
    //  32 Data Units Read / 48 Data Units Written / 64 Host Read Commands /
    //  80 Host Write Commands / 96 Controller Busy Time / 112 Power Cycles /
    // 128 Power On Hours / 144 Unsafe Shutdowns / 160 Media & Data Integrity Errors /
    // 176 Number of Error Information Log Entries
    // ⚠️ 6..31 是 Endurance Group Critical Warning Summary + 保留区，
    //    128 位计数器从 byte 32 才开始（不要误以为从 byte 6 紧凑排列）
    let du_read = le_u128(&data[32..48]); // 单位：1000 × 512B
    let du_written = le_u128(&data[48..64]);
    let hr = le_u128(&data[64..80]);
    let hw = le_u128(&data[80..96]);
    let busy = le_u128(&data[96..112]); // 分钟
    let cycles = le_u128(&data[112..128]);
    let hours = le_u128(&data[128..144]);
    let unsafe_sd = le_u128(&data[144..160]);
    let media_err = le_u128(&data[160..176]);
    let err_log = if data.len() >= 192 { le_u128(&data[176..192]) } else { 0 };

    const UNIT: u128 = 512_000; // Data Unit = 1000 × 512 字节
    if du_read > 0 {
        s.total_read_bytes = Some((du_read.saturating_mul(UNIT)).min(u64::MAX as u128) as u64);
    }
    if du_written > 0 {
        s.total_written_bytes = Some((du_written.saturating_mul(UNIT)).min(u64::MAX as u128) as u64);
    }
    if hr > 0 {
        s.host_read_commands = Some(hr as u64);
    }
    if hw > 0 {
        s.host_write_commands = Some(hw as u64);
    }
    if busy > 0 {
        s.controller_busy_minutes = Some(busy as u64);
    }
    s.power_cycles = Some(cycles as u64);
    s.power_on_hours = Some(hours as u64);
    if unsafe_sd > 0 {
        s.unsafe_shutdowns = Some(unsafe_sd as u64);
    }
    if media_err > 0 {
        s.reallocated_sectors = Some(media_err as u64);
    }
    s.predict_failure = Some(critical != 0 || used >= 90 || media_err > 0);

    // ---- CDI 式健康项列表（NVMe 没有 ATA 属性表，用这个视图呈现）----
    let crit_names: [(&str, u8); 5] = [
        ("备用空间不足", 0x01),
        ("温度过高", 0x02),
        ("可靠性降级", 0x04),
        ("介质变为只读", 0x08),
        ("掉电保护失效", 0x10),
    ];
    let mut items: Vec<HealthItem> = Vec::new();
    let active: Vec<&str> = crit_names
        .iter()
        .filter(|(_, bit)| critical & bit != 0)
        .map(|(n, _)| *n)
        .collect();
    items.push(HealthItem {
        label: "临界警告".into(),
        value: if active.is_empty() {
            "无（0x00）".to_string()
        } else {
            active.join("、")
        },
        level: if critical == 0 { 0 } else { 2 },
        raw: Some(critical as f64),
    });
    items.push(match s.temperature {
        Some(t) => HealthItem {
            label: "温度".into(),
            value: format!("{} °C", t),
            level: if t >= 75 {
                2
            } else if t >= 65 {
                1
            } else {
                0
            },
            raw: Some(t as f64),
        },
        None => HealthItem {
            label: "温度".into(),
            value: "未提供".into(),
            level: 3,
            raw: None,
        },
    });
    items.push(HealthItem {
        label: "可用备用空间".into(),
        value: format!("{}%（阈值 {}%）", spare, spare_thr),
        level: if spare_thr > 0 && spare <= spare_thr { 1 } else { 0 },
        raw: Some(spare as f64),
    });
    items.push(HealthItem {
        label: "已用寿命".into(),
        value: format!("{}%", used),
        level: if used >= 90 {
            2
        } else if used >= 70 {
            1
        } else {
            0
        },
        raw: Some(used as f64),
    });
    if let Some(v) = s.total_read_bytes {
        items.push(HealthItem {
            label: "累计读取量".into(),
            value: human_bytes(v),
            level: 3,
            raw: Some(v as f64),
        });
    }
    if let Some(v) = s.total_written_bytes {
        items.push(HealthItem {
            label: "累计写入量".into(),
            value: human_bytes(v),
            level: 3,
            raw: Some(v as f64),
        });
    }
    if let Some(v) = s.host_read_commands {
        items.push(HealthItem {
            label: "主机读命令数".into(),
            value: format!("{}", v),
            level: 3,
            raw: Some(v as f64),
        });
    }
    if let Some(v) = s.host_write_commands {
        items.push(HealthItem {
            label: "主机写命令数".into(),
            value: format!("{}", v),
            level: 3,
            raw: Some(v as f64),
        });
    }
    if let Some(v) = s.controller_busy_minutes {
        items.push(HealthItem {
            label: "控制器忙碌时间".into(),
            value: format!("{} 分钟", v),
            level: 3,
            raw: Some(v as f64),
        });
    }
    items.push(HealthItem {
        label: "通电次数".into(),
        value: format!("{}", s.power_cycles.unwrap_or(0)),
        level: 3,
        raw: None,
    });
    items.push(HealthItem {
        label: "通电时长".into(),
        value: format!("{} 小时", s.power_on_hours.unwrap_or(0)),
        level: 3,
        raw: None,
    });
    items.push(HealthItem {
        label: "异常掉电".into(),
        value: format!("{} 次", s.unsafe_shutdowns.unwrap_or(0)),
        level: if s.unsafe_shutdowns.unwrap_or(0) > 0 { 1 } else { 0 },
        raw: None,
    });
    items.push(HealthItem {
        label: "介质与数据完整性错误".into(),
        value: format!("{}", s.reallocated_sectors.unwrap_or(0)),
        level: if s.reallocated_sectors.unwrap_or(0) > 0 { 2 } else { 0 },
        raw: None,
    });
    items.push(HealthItem {
        label: "错误日志条目".into(),
        value: format!("{}", err_log),
        level: 3,
        raw: None,
    });
    // 温度传感器：byte 200 起，每 2 字节一个（Sensor 1 / Sensor 2），单位 K
    if data.len() >= 204 {
        let sensors: Vec<String> = (0..2)
            .filter_map(|k| {
                let off = 200 + k * 2;
                if off + 2 > data.len() {
                    return None;
                }
                let kelvin = u16::from_le_bytes([data[off], data[off + 1]]);
                if kelvin > 200 && kelvin < 500 {
                    Some(format!("T{} {} °C", k + 1, kelvin as i32 - 273))
                } else {
                    None
                }
            })
            .collect();
        if !sensors.is_empty() {
            items.push(HealthItem {
                label: "温度传感器".into(),
                value: sensors.join(" / "),
                level: 3,
                raw: None,
            });
        }
    }
    s.health_items = items;
    s
}

/// NVMe Identify Controller (CNS 0x01)：补齐型号 / 固件 / 序列号
fn parse_nvme_identify(data: &[u8], s: &mut SmartInfo) {
    if data.len() < 72 {
        return;
    }
    let take = |a: usize, b: usize| -> String { cstr(&data[a..b]) };
    let serial = take(4, 24);
    let model = take(24, 64);
    let firmware = take(64, 72);
    if !serial.is_empty() {
        s.serial = Some(serial);
    }
    if !model.is_empty() {
        s.model = Some(model);
    }
    if !firmware.is_empty() {
        s.firmware = Some(firmware);
    }
    s.rotation_rate = Some(0); // NVMe 无旋转介质
}

// ------------------------------------------------------------ 主流程

fn try_ata(h: isize) -> Option<SmartInfo> {
    let _ = ata_cmd_no_data(h, smart_enable_regs()); // 先使能（失败忽略）
    let data = ata_read(h, smart_regs(SMART_READ_DATA), 512)?;
    if data.len() < 362 || is_le_zero(&data[2..362]) {
        return None;
    }
    // 阈值必须单独读 0xD1（0xD0 里没有阈值字段）
    let thresholds = ata_read(h, smart_regs(SMART_READ_THRESHOLDS), 512)
        .map(|t| parse_thresholds(&t))
        .unwrap_or_default();
    let mut s = parse_ata(&data, &thresholds);
    if let Some(id) = ata_read(h, identify_regs(), 512) {
        parse_identify(&id, &mut s);
    }
    Some(s)
}

fn try_nvme(h: isize) -> Option<SmartInfo> {
    let log = nvme_smart_log(h)?;
    if log.len() < 64 || is_le_zero(&log[..64]) {
        return None;
    }
    let mut s = parse_nvme(&log);
    if let Some(id) = nvme_identify(h) {
        parse_nvme_identify(&id, &mut s);
    }
    Some(s)
}

/// 读取指定物理盘的完整健康数据
pub(crate) fn read_smart(n: u32) -> SmartInfo {
    let drive_name = format!("PhysicalDrive{n}");
    let mut info = SmartInfo {
        drive: drive_name.clone(),
        kind: "未知".into(),
        ..Default::default()
    };

    let Some(h) = open_drive(n) else {
        info.message = "无法打开物理盘（需管理员权限，或被驱动 / 杀软占用）".into();
        return info;
    };

    // ① 先判总线类型，避免对不匹配的盘白连发 IOCTL
    let desc = storage_device_descriptor(h);
    let bus_type = desc.as_ref().and_then(|d| d.3);
    let bus_name = bus_type.map(bus_label).unwrap_or_default();

    // ② 主通道：按 BusType 选顺序，另一路做兜底
    let mut tried: Vec<&str> = Vec::new();
    let mut primary = if bus_type == Some(BUS_TYPE_NVME) {
        tried.push("NVMe日志页(优先)");
        match try_nvme(h) {
            Some(s) => Some(s),
            None => {
                tried.push("ATA(回退)");
                try_ata(h)
            }
        }
    } else {
        tried.push("ATA(优先)");
        match try_ata(h) {
            Some(s) => Some(s),
            None => {
                tried.push("NVMe日志页(回退)");
                try_nvme(h)
            }
        }
    };
    if primary.is_none() && bus_type != Some(BUS_TYPE_NVME) {
        tried.push("NVMe日志页(再试)");
        primary = try_nvme(h);
    }

    let mut s = primary.unwrap_or_else(|| SmartInfo {
        drive: drive_name.clone(),
        kind: if bus_type == Some(BUS_TYPE_NVME) {
            "NVMe".into()
        } else {
            "未知".into()
        },
        ..Default::default()
    });
    s.drive = drive_name.clone();
    if !bus_name.is_empty() {
        s.bus = Some(bus_name.clone());
    }
    if let Some((vendor, _, _, _)) = &desc {
        if let Some(v) = vendor {
            s.vendor = Some(v.clone());
        }
    }

    // ③ 元数据补全（只补空缺，不覆盖主通道已取到的值）
    let mut notes: Vec<String> = Vec::new();
    if s.serial.as_deref().unwrap_or("").trim().is_empty() {
        if let Some(v) = property_adapter_serial(h) {
            notes.push("序列号←适配器属性".to_string());
            s.serial = Some(v);
        }
    }
    if s.serial.as_deref().unwrap_or("").trim().is_empty() {
        if let Some(v) = desc.as_ref().and_then(|d| d.2.clone()) {
            notes.push("序列号←设备描述符".to_string());
            s.serial = Some(v);
        }
    }
    if s.model.as_deref().unwrap_or("").trim().is_empty() {
        if let Some(v) = desc.as_ref().and_then(|d| d.1.clone()) {
            notes.push("型号←设备描述符".to_string());
            s.model = Some(v);
        }
    }

    // ④ 来源与诊断
    if s.kind == "未知" {
        s.message = format!(
            "主通道均失败（尝试：{}）；已用属性源补全可用字段",
            tried.join(" → ")
        );
        s.source = format!(
            "属性源({}{}{})",
            if s.temperature.is_some() { "温度" } else { "" },
            if s.wear.is_some() { "+磨损" } else { "" },
            if s.serial.is_some() { "+序列号" } else { "" }
        );
    } else {
        let mut parts = vec![s.source.clone()];
        if !notes.is_empty() {
            parts.push(format!("交叉补全：{}", notes.join("、")));
        }
        s.source = parts.join(" + ");
        s.message = String::new();
    }

    unsafe { CloseHandle(h as _) };
    s
}

/// 读取物理盘容量（用于「Get-PhysicalDisk 枚举不到」时的兜底）
pub(crate) fn disk_length(n: u32) -> Option<u64> {
    let h = open_drive(n)?;
    let mut len: i64 = 0;
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h as _,
            IOCTL_DISK_GET_LENGTH_INFO,
            std::ptr::null_mut(),
            0,
            &mut len as *mut i64 as *mut core::ffi::c_void,
            std::mem::size_of::<i64>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h as _) };
    if ok == 0 || len <= 0 {
        None
    } else {
        Some(len as u64)
    }
}

/// 读取全部物理盘（调试 / 扩展用）
#[allow(dead_code)]
pub(crate) fn read_all_smart() -> Vec<SmartInfo> {
    let mut out = Vec::new();
    for n in 0..16u32 {
        let info = read_smart(n);
        if info.kind != "未知" {
            out.push(info);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_log_偏移符合规范() {
        // 构造一条最小 SMART/Health Log：把「通电小时」写在其规范位置 byte 128
        let mut log = vec![0u8; 512];
        log[0] = 0; // 无临界警告
        log[1..3].copy_from_slice(&313u16.to_le_bytes()); // 40℃ = 313K
        log[3] = 100; // 备用空间
        log[4] = 10; // 备用阈值
        log[5] = 3; // 已用 3%
        log[112..128].copy_from_slice(&88u128.to_le_bytes()); // Power Cycles = 88
        log[128..144].copy_from_slice(&4321u128.to_le_bytes()); // Power On Hours = 4321
        let s = parse_nvme(&log);
        assert_eq!(s.kind, "NVMe");
        assert_eq!(s.power_on_hours, Some(4321), "通电小时读 byte128");
        assert_eq!(s.power_cycles, Some(88), "通电次数读 byte112");
        assert_eq!(s.temperature, Some(40));
        assert_eq!(s.wear, Some(3));
        assert!(!s.health_items.is_empty(), "NVMe 必须产出健康项列表");
    }

    #[test]
    fn nvme_数据单元换算为字节() {
        let mut log = vec![0u8; 512];
        log[3] = 100;
        log[48..64].copy_from_slice(&1_000_000u128.to_le_bytes()); // 写 100 万个 Data Unit
        let s = parse_nvme(&log);
        // 1 Data Unit = 1000 × 512 字节
        assert_eq!(s.total_written_bytes, Some(1_000_000u64 * 512_000));
        assert!(s.total_read_bytes.is_none(), "写量不得被误读成读量");
    }

    #[test]
    fn bus_type_映射正确() {
        assert_eq!(bus_label(BUS_TYPE_NVME), "NVMe");
        assert_eq!(bus_label(BUS_TYPE_SATA), "SATA");
        assert_eq!(bus_label(BUS_TYPE_USB), "USB");
        assert_eq!(bus_label(BUS_TYPE_VIRTUAL), "虚拟磁盘");
    }

    #[test]
    fn 结构体尺寸锁死() {
        assert_eq!(std::mem::size_of::<AtaPassThroughDirect>(), 48);
        assert_eq!(std::mem::size_of::<StorageProtocolSpecificData>(), 40);
        assert_eq!(std::mem::size_of::<StoragePropertyQueryWithProtocol>(), 48);
    }

    #[test]
    fn 关键常量与_sdk_一致() {
        // windows-sys: IOCTL_STORAGE_QUERY_PROPERTY = 2954240 = 0x2D1400
        assert_eq!(IOCTL_STORAGE_QUERY_PROPERTY, 0x002D_1400);
        // StorageDeviceProtocolSpecificProperty = 50（49 是 Adapter 版本，别用）
        assert_eq!(PROP_DEVICE_PROTOCOL_SPECIFIC, 50);
    }
}