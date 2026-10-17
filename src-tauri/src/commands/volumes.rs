//! 盘符容量查询（GetLogicalDrives + GetDiskFreeSpaceExW + IOCTL 介质识别）

use serde::Serialize;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY};
use windows_sys::Win32::System::IO::DeviceIoControl;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub letter: String,
    pub label: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub media: String, // SSD | HDD | 未知
    pub fs: String,
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// 盘符 → 介质 映射：Get-Partition → Get-Disk → Get-PhysicalDisk
/// 这是判定 SSD/HDD 的**可靠**方式（此前的 IOCTL 总线猜测会把 SATA 固态盘误判为 HDD）
fn part_media_map() -> std::collections::HashMap<char, String> {
    use serde_json::Value;
    let mut map = std::collections::HashMap::new();
    let script = format!(
        "{} $o=@(Get-Partition | Where-Object {{ $_.DriveLetter }} | ForEach-Object {{ $pd = (Get-Disk -Number $_.DiskNumber | Get-PhysicalDisk); [pscustomobject]@{{ L=\"$($_.DriveLetter)\"; M=\"$($pd.MediaType)\" }} }}); $o | ConvertTo-Json -Compress",
        crate::commands::PS_UTF8_PRELUDE
    );
    let out = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    let Ok(o) = out else { return map };
    let text = crate::commands::decode_out(&o.stdout);
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return map };
    let arr = match v {
        Value::Array(a) => a,
        obj @ Value::Object(_) => vec![obj],
        _ => vec![],
    };
    for item in arr {
        let (Some(l), Some(m)) = (
            item.get("L").and_then(|x| x.as_str()),
            item.get("M").and_then(|x| x.as_str()),
        ) else { continue };
        if let Some(c) = l.chars().next() {
            let lm = m.to_lowercase();
            let media = if lm.contains("ssd") {
                "SSD"
            } else if lm.contains("hdd") {
                "HDD"
            } else if lm.contains("scm") {
                "SCM"
            } else {
                "未知"
            };
            map.insert(c.to_ascii_uppercase(), media.to_string());
        }
    }
    map
}

/// 介质兜底判定：仅 NVMe 总线可确定为 SSD；其余总线（SATA/USB 等）不猜测，返回「未知」
/// 避免把 SATA 接口的固态硬盘错误标成 HDD
fn detect_media(root: &str) -> String {
    let path = format!("\\\\.\\{}", root.trim_end_matches('\\'));
    let wide = to_wide(&path);
    unsafe {
        let h = CreateFileW(
            wide.as_ptr(),
            0x8000_0000, // GENERIC_READ
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0, // 非 FILE_FLAG_WRITE_THROUGH，普通句柄即可
            0 as _,
        );
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return "未知".into();
        }
        let q = STORAGE_PROPERTY_QUERY { PropertyId: 0, QueryType: 0, AdditionalParameters: [0] };
        let mut buf = [0u8; 512];
        let mut ret = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        );
        CloseHandle(h);
        if ok == 0 || ret < 29 {
            return "未知".into();
        }
        // STORAGE_DEVICE_DESCRIPTOR.BusType 位于偏移 28（17=Nvme 可确定为 SSD）
        match buf[28] {
            17 => "SSD".into(),
            _ => "未知".into(),
        }
    }
}

/// 当前进程是否以管理员权限运行（用于侧栏权限状态展示）
#[tauri::command]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[tauri::command]
pub async fn get_volumes() -> Vec<Volume> {
    // 含 PowerShell 物理盘映射查询，放后台线程执行
    tauri::async_runtime::spawn_blocking(get_volumes_impl).await.unwrap_or_default()
}

fn get_volumes_impl() -> Vec<Volume> {
    let media_map = part_media_map();
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let root_w = to_wide(&root);
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(root_w.as_ptr(), std::ptr::null_mut(), &mut total, &mut free)
        };
        if ok == 0 || total == 0 {
            continue;
        }
        let mut vol_buf = [0u16; 261];
        let mut fs_buf = [0u16; 64];
        let mut label = String::new();
        let mut fs = String::from("NTFS");
        unsafe {
            if GetVolumeInformationW(
                root_w.as_ptr(),
                vol_buf.as_mut_ptr(),
                vol_buf.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            ) != 0
            {
                label = String::from_utf16_lossy(&vol_buf[..vol_buf.iter().position(|&c| c == 0).unwrap_or(0)]);
                fs = String::from_utf16_lossy(&fs_buf[..fs_buf.iter().position(|&c| c == 0).unwrap_or(0)]);
            }
        }
        let drive_type = unsafe { GetDriveTypeW(root_w.as_ptr()) };
        if drive_type < 3 {
            continue; // 跳过软驱/未就绪等
        }
        // 介质：优先用物理盘真实映射；未命中时仅 NVMe 兜底判定，其余返回「未知」（不猜测）
        let media = match media_map.get(&letter) {
            Some(m) => m.clone(),
            None => {
                if drive_type == 3 { detect_media(&root) } else { "未知".to_string() }
            }
        };
        out.push(Volume {
            letter: letter.to_string(),
            label,
            total_bytes: total,
            used_bytes: total.saturating_sub(free),
            free_bytes: free,
            media,
            fs,
        });
    }
    out
}
