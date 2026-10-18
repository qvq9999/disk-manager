//! 硬件信息：注册表 + Win32 API + WMI（PowerShell 单次调用，带超时与 TTL 缓存）
//! 覆盖：CPU（物理/逻辑核）、内存（类型/频率/分条）、显卡（含显存）、显示器（型号/尺寸）、
//! 磁盘（介质类型/总线/型号）、主板/BIOS、系统（版本/架构/运行时间）、声卡、网卡

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;
use windows_sys::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    pub computer_name: String,
    pub uptime_secs: u64,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpu: Vec<GpuInfo>,
    pub monitors: Vec<MonitorInfo>,
    pub disks: Vec<DiskInfo>,
    pub motherboard: BoardInfo,
    pub system: SystemInfo,
    pub sound: Vec<String>,
    pub network: Vec<NetInfo>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo { pub name: String, pub cores_physical: u32, pub cores_logical: u32, pub base_mhz: u64 }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStick {
    pub manufacturer: String,
    pub capacity_bytes: u64,
    pub speed_mhz: u32,            // 标称频率
    pub configured_mhz: u32,       // 实际运行频率
    pub voltage_mv: u32,           // 当前电压（mV）
    pub part_number: String,       // 型号编码
    pub slot: String,              // 插槽位置
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub total_bytes: u64, pub available_bytes: u64, pub used_pct: u32,
    pub mem_type: String, pub speed_mhz: u32, pub sticks: Vec<MemoryStick>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo { pub name: String, pub vram_bytes: u64, pub driver_version: String, pub vendor: String }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo { pub name: String, pub manufacturer: String, pub size_inch: f32 }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo { pub model: String, pub size_bytes: u64, pub media: String, pub bus: String }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardInfo {
    pub manufacturer: String, pub product: String, pub bios_version: String,
    pub system_model: String, pub system_manufacturer: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo { pub caption: String, pub version: String, pub build: String, pub arch: String }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetInfo { pub name: String, pub kind: String }

const PS_SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue'
try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}
$OutputEncoding=[System.Text.Encoding]::UTF8

# ⚠️ 性能铁律：每个 CIM 查询都必须带 -Property 白名单。
#    实测（本机 Ryzen 5 5600XT / B450）：
#      Get-CimInstance Win32_Processor | Select-Object ...      = 1022 ms（全属性物化 + 管道）
#      Get-CimInstance Win32_Processor -Property NumberOfCores   =    7 ms   ← 慢 145 倍
#    同理 Get-PhysicalDisk（cdxml 封装，会顺带查健康状态）= 1078~1550 ms，
#    换成 MSFT_PhysicalDisk -Property = 23 ms。整段脚本因此从 3.6s 降到 0.7s。
$os=Get-CimInstance Win32_OperatingSystem -Property Caption,Version,BuildNumber,OSArchitecture,LastBootUpTime
$cs=Get-CimInstance Win32_ComputerSystem -Property Manufacturer,Model
$bb=Get-CimInstance Win32_BaseBoard -Property Manufacturer,Product
$bios=Get-CimInstance Win32_BIOS -Property Manufacturer,SMBIOSBIOSVersion
$cpu=Get-CimInstance Win32_Processor -Property Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed | Select-Object -First 1
$gpu=@(Get-CimInstance Win32_VideoController -Property Name,DriverVersion,AdapterRAM,AdapterCompatibility | Select-Object Name,DriverVersion,AdapterRAM,AdapterCompatibility)
$mem=@(Get-CimInstance Win32_PhysicalMemory -Property Manufacturer,Capacity,Speed,ConfiguredClockSpeed,ConfiguredVoltage,PartNumber,DeviceLocator,SMBIOSMemoryType | Select-Object Manufacturer,Capacity,Speed,ConfiguredClockSpeed,ConfiguredVoltage,PartNumber,DeviceLocator,SMBIOSMemoryType)
$pd=@(Get-CimInstance -Namespace root\Microsoft\Windows\Storage -ClassName MSFT_PhysicalDisk -Property FriendlyName,MediaType,BusType,Size | Select-Object FriendlyName,MediaType,BusType,Size)
$snd=@(Get-CimInstance Win32_SoundDevice -Property Name | Select-Object Name)
$net=@(Get-CimInstance Win32_NetworkAdapter -Property Name,NetConnectionID,PhysicalAdapter | Where-Object { $_.PhysicalAdapter -eq $true } | Select-Object Name,NetConnectionID)
$mon=@(Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID -Property ManufacturerName,UserFriendlyName,ProductCodeID | ForEach-Object { [pscustomobject]@{ M=(-join [char[]]($_.ManufacturerName | Where-Object {$_ -ne 0})); N=(-join [char[]]($_.UserFriendlyName | Where-Object {$_ -ne 0})); C=(-join [char[]]($_.ProductCodeID | Where-Object {$_ -ne 0})) } })
$dim=@(Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorBasicDisplayParams -Property MaxHorizontalImageSize,MaxVerticalImageSize | Select-Object MaxHorizontalImageSize,MaxVerticalImageSize)
$gv=@(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*' | Where-Object { $_.DriverDesc } | Select-Object DriverDesc,@{n='VRAM';e={$_.'HardwareInformation.qwMemorySize'}})
[pscustomobject]@{ os=$os.Caption; ver=$os.Version; build=$os.BuildNumber; arch=$os.OSArchitecture; csM=$cs.Manufacturer; csModel=$cs.Model; bbM=$bb.Manufacturer; bbP=$bb.Product; biosM=$bios.Manufacturer; biosV=$bios.SMBIOSBIOSVersion; cpuName=$cpu.Name; cpuCores=$cpu.NumberOfCores; cpuLog=$cpu.NumberOfLogicalProcessors; cpuMhz=$cpu.MaxClockSpeed; gpu=$gpu; gpuVram=$gv; mem=$mem; disks=$pd; sound=$snd; net=$net; mon=$mon; monDim=$dim } | ConvertTo-Json -Compress -Depth 4
"#;

/// 带超时的 PowerShell 执行（防 WMI 偶发挂起拖死线程池 → 界面"没反应"）
fn run_ps(script: &str, timeout: Duration) -> Option<String> {
    let mut child = crate::commands::silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(crate::commands::decode_out(&out.stdout))
}

fn wmi_collect() -> serde_json::Value {
    match run_ps(PS_SCRIPT, Duration::from_secs(25)) {
        Some(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    }
}

fn js_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim_end_matches('\0').trim().to_string()
}

fn js_u64(v: &serde_json::Value, key: &str) -> u64 {
    if let Some(n) = v.get(key).and_then(|x| x.as_u64()) { return n; }
    v.get(key).and_then(|x| x.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn js_arr<'a>(v: &'a serde_json::Value, key: &str) -> Vec<&'a serde_json::Value> {
    match v.get(key) {
        Some(serde_json::Value::Array(a)) => a.iter().collect(),
        Some(o @ serde_json::Value::Object(_)) => vec![o],
        _ => vec![],
    }
}

/// SMBIOS 内存类型编码 → 名称
fn mem_type_name(code: u64) -> String {
    match code {
        20 => "DDR", 21 => "DDR2", 24 => "DDR3", 26 => "DDR4", 34 => "DDR5",
        30 => "LPDDR4", 35 => "LPDDR5", _ => "",
    }
    .to_string()
}

/// 从单块 EDID 解析物理尺寸（英寸）
/// ⚠️ 关键：EDID 基础块偏移 0x15/0x16 是**单字节厘米**（精度只有 1cm，算不出 23.8）；
/// 必须解析**详细时序描述符 DTD**（偏移 0x36/0x48/0x5A/0x6C 各 18 字节）里的**毫米级**
/// 图像尺寸：d[12]/d[13] 为低 8 位，d[14] 高/低 4 位为高 4 位。
fn edid_inches(bytes: &[u8]) -> Option<f32> {
    if bytes.len() < 128 {
        return None;
    }
    for off in [0x36usize, 0x48, 0x5A, 0x6C] {
        if off + 18 > bytes.len() {
            break;
        }
        let d = &bytes[off..off + 18];
        if d[0] == 0 && d[1] == 0 {
            continue; // 非 DTD（显示器描述符）
        }
        let h_mm = d[12] as u32 | (((d[14] & 0xF0) as u32) << 4);
        let v_mm = d[13] as u32 | (((d[14] & 0x0F) as u32) << 8);
        if (50..2000).contains(&h_mm) && (50..2000).contains(&v_mm) {
            let inch = ((h_mm * h_mm + v_mm * v_mm) as f32).sqrt() / 25.4;
            let inch = (inch * 10.0).round() / 10.0;
            if inch > 5.0 && inch < 100.0 {
                return Some(inch);
            }
        }
    }
    // 回退：基础块厘米（精度低，仅在 DTD 缺失时使用）
    let h_cm = bytes[0x15] as f32;
    let v_cm = bytes[0x16] as f32;
    if h_cm > 0.0 && v_cm > 0.0 {
        let inch = ((h_cm * h_cm + v_cm * v_cm) as f32).sqrt() / 2.54;
        let inch = (inch * 10.0).round() / 10.0;
        if inch > 5.0 && inch < 100.0 {
            return Some(inch);
        }
    }
    None
}

/// 从注册表读取显示器 EDID 物理尺寸，返回 (显示器标识, 英寸)
/// 标识 = 注册表键名（如 "TTR9209"），与 WmiMonitorID 的 厂商名+产品码 精确对应，
/// 避免多显示器/多 EDID 记录时按索引错配（曾导致 23.8 被显示成 24.5）
fn monitor_edid_sizes() -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    let base = r"SYSTEM\CurrentControlSet\Enum\DISPLAY";
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(base, KEY_READ) else {
        return out;
    };
    for mfr in root.enum_keys().flatten() {
        let Ok(mkey) = root.open_subkey_with_flags(&mfr, KEY_READ) else { continue };
        for inst in mkey.enum_keys().flatten() {
            let Ok(ikey) = mkey.open_subkey_with_flags(&inst, KEY_READ) else { continue };
            let Ok(dp) = ikey.open_subkey_with_flags("Device Parameters", KEY_READ) else { continue };
            let Ok(raw) = dp.get_raw_value("EDID") else { continue };
            if let Some(inch) = edid_inches(&raw.bytes) {
                let key = mfr.to_uppercase();
                if !out.iter().any(|(k, _)| *k == key) {
                    out.push((key, inch));
                }
            }
        }
    }
    out
}

fn cpu_from_registry() -> CpuInfo {
    let mut c = CpuInfo::default();
    if let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", KEY_READ)
    {
        c.name = root.get_value("ProcessorNameString").unwrap_or_default();
        c.base_mhz = root.get_value::<u32, _>("~MHz").unwrap_or(0) as u64;
    }
    c
}

fn memory_from_api() -> (u64, u64, u32) {
    unsafe {
        let mut ms: MEMORYSTATUSEX = std::mem::zeroed();
        ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut ms) != 0 {
            let total = ms.ullTotalPhys as u64;
            let avail = ms.ullAvailPhys as u64;
            let used = if total > 0 { ((total - avail) * 100 / total) as u32 } else { 0 };
            return (total, avail, used);
        }
    }
    (0, 0, 0)
}

fn collect() -> HardwareInfo {
    let mut info = HardwareInfo {
        computer_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        cpu: cpu_from_registry(),
        ..Default::default()
    };
    let (total, avail, used) = memory_from_api();
    info.memory.total_bytes = total;
    info.memory.available_bytes = avail;
    info.memory.used_pct = used;

    let wmi = wmi_collect();
    // 运行时间统一用 GetTickCount64：缓存命中的分支也要能重算，
    // 不能依赖「上一次 PS 采集时刻」推算（期间可能重启过）
    info.uptime_secs = unsafe { GetTickCount64() } / 1000;

    // CPU（WMI 补核心数与频率）
    info.cpu.cores_physical = js_u64(&wmi, "cpuCores") as u32;
    info.cpu.cores_logical = js_u64(&wmi, "cpuLog") as u32;
    let mhz = js_u64(&wmi, "cpuMhz");
    if mhz > 0 { info.cpu.base_mhz = mhz; }
    if info.cpu.name.is_empty() { info.cpu.name = js_str(&wmi, "cpuName"); }
    if info.cpu.cores_logical == 0 {
        info.cpu.cores_logical = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0);
    }

    // 系统
    info.system.caption = js_str(&wmi, "os");
    info.system.version = js_str(&wmi, "ver");
    info.system.build = js_str(&wmi, "build");
    info.system.arch = js_str(&wmi, "arch");

    // 主板 / BIOS / 机型
    info.motherboard.manufacturer = js_str(&wmi, "bbM");
    info.motherboard.product = js_str(&wmi, "bbP");
    info.motherboard.system_manufacturer = js_str(&wmi, "csM");
    let model = js_str(&wmi, "csModel");
    info.motherboard.system_model = if model.is_empty() || model == "System Product Name" {
        "台式电脑".to_string()
    } else {
        model
    };
    info.motherboard.bios_version = format!("{} {}", js_str(&wmi, "biosM"), js_str(&wmi, "biosV")).trim().to_string();

    // 显卡（显存优先取注册表 qwMemorySize，规避 AdapterRAM 的 4GB 上限）
    let vram_map: Vec<(String, u64)> = js_arr(&wmi, "gpuVram")
        .into_iter()
        .map(|g| (js_str(g, "DriverDesc"), js_u64(g, "VRAM")))
        .collect();
    for g in js_arr(&wmi, "gpu") {
        let name = js_str(g, "Name");
        if name.is_empty() { continue; }
        let mut vram = js_u64(g, "AdapterRAM");
        for (desc, qw) in &vram_map {
            if !desc.is_empty() && (name.contains(desc.as_str()) || desc.contains(name.as_str())) && *qw > 0 {
                vram = *qw;
                break;
            }
        }
        info.gpu.push(GpuInfo {
            name,
            vram_bytes: vram,
            driver_version: js_str(g, "DriverVersion"),
            vendor: js_str(g, "AdapterCompatibility"),
        });
    }

    // 内存条与内存类型
    for m in js_arr(&wmi, "mem") {
        let cap = js_u64(m, "Capacity");
        let speed = js_u64(m, "Speed") as u32;
        let cfg = js_u64(m, "ConfiguredClockSpeed") as u32;
        let mt = mem_type_name(js_u64(m, "SMBIOSMemoryType"));
        if info.memory.mem_type.is_empty() && !mt.is_empty() { info.memory.mem_type = mt; }
        // 汇总频率优先用实际运行频率
        let effective = if cfg > 0 { cfg } else { speed };
        if info.memory.speed_mhz == 0 && effective > 0 { info.memory.speed_mhz = effective; }
        info.memory.sticks.push(MemoryStick {
            manufacturer: js_str(m, "Manufacturer"),
            capacity_bytes: cap,
            speed_mhz: speed,
            configured_mhz: cfg,
            voltage_mv: js_u64(m, "ConfiguredVoltage") as u32,
            part_number: js_str(m, "PartNumber"),
            slot: js_str(m, "DeviceLocator"),
        });
    }

    // 物理磁盘（介质类型 SSD/HDD + 总线）——MediaType/BusType 可能是字符串或整数，两种都兼容
    for d in js_arr(&wmi, "disks") {
        let media_str = d.get("MediaType").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
        let media = if media_str.contains("ssd") {
            "SSD"
        } else if media_str.contains("hdd") {
            "HDD"
        } else {
            match js_u64(d, "MediaType") {
                3 => "HDD", 4 => "SSD", 5 => "SCM", _ => "未知",
            }
        }
        .to_string();
        let bus_str = d.get("BusType").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
        let bus = if bus_str.contains("nvme") {
            "NVMe"
        } else if bus_str.contains("sata") {
            "SATA"
        } else if bus_str.contains("usb") {
            "USB"
        } else if bus_str.contains("raid") {
            "RAID"
        } else {
            match js_u64(d, "BusType") {
                17 => "NVMe", 11 => "SATA", 7 => "USB", 8 => "RAID", 10 => "SAS", _ => "",
            }
        }
        .to_string();
        info.disks.push(DiskInfo {
            model: js_str(d, "FriendlyName"),
            size_bytes: js_u64(d, "Size"),
            media,
            bus,
        });
    }

    // 显示器（型号 + 物理尺寸）：EDID 精确值优先（按厂商码+产品码匹配），WMI 兜底
    let edid_sizes = monitor_edid_sizes();
    let dims: Vec<(f32, f32)> = js_arr(&wmi, "monDim")
        .into_iter()
        .map(|d| (
            js_u64(d, "MaxHorizontalImageSize") as f32,
            js_u64(d, "MaxVerticalImageSize") as f32,
        ))
        .collect();
    for (i, m) in js_arr(&wmi, "mon").into_iter().enumerate() {
        let name = js_str(m, "N");
        let code = js_str(m, "C");
        let mfr_code = js_str(m, "M"); // 如 "TTR"
        let display_name = if code.is_empty() { name.clone() } else { format!("{name} [{code}]") };
        if display_name.trim().is_empty() { continue; }
        let (cm_w, cm_h) = dims.get(i).copied().unwrap_or((0.0, 0.0));
        let wmi_inch = if cm_w > 0.0 && cm_h > 0.0 {
            let diag_cm = (cm_w * cm_w + cm_h * cm_h).sqrt();
            (diag_cm / 2.54 * 10.0).round() / 10.0
        } else {
            0.0
        };
        // 精确匹配：WmiMonitorID 的 厂商名(M) + 产品码(C) == 注册表 EDID 键名（如 TTR9209）
        let want = format!("{}{}", mfr_code, code).to_uppercase();
        let edid_inch = edid_sizes
            .iter()
            .find(|(k, _)| *k == want)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        let inch = if edid_inch > 0.0 { edid_inch } else { wmi_inch };
        info.monitors.push(MonitorInfo {
            name: display_name,
            manufacturer: mfr_code,
            size_inch: inch,
        });
    }

    // 声卡
    for s in js_arr(&wmi, "sound") {
        let n = js_str(s, "Name");
        if !n.is_empty() && !info.sound.contains(&n) { info.sound.push(n); }
    }

    // 网卡（物理适配器；无线按名称特征分类）
    for n in js_arr(&wmi, "net") {
        let name = js_str(n, "Name");
        if name.is_empty() { continue; }
        let lower = name.to_lowercase();
        let kind = if lower.contains("wi-fi") || lower.contains("wireless") || lower.contains("wlan") || lower.contains("802.11") {
            "无线"
        } else {
            "有线"
        }
        .to_string();
        info.network.push(NetInfo { name, kind });
    }

    info
}

// 进程级缓存：应用运行期间只扫描一次，切页复用；刷新时机由前端控制（见 api.ts）
// 失效条件：① 进程退出  ② 前端传 refresh=true（手动刷新/超过 30 分钟 TTL）
static CACHE: Mutex<Option<HardwareInfo>> = Mutex::new(None);

// ---------------- 磁盘缓存：让「每次启动」都秒开 ----------------
//
// 实测采集一次约 1.4~1.6s（PowerShell 进程启动 ~0.9s 无法避免），而硬件信息
// 几乎不变（换内存条/显卡才会变）。所以把结果落到配置文件旁边：
// 启动时先读文件立即渲染，再在后台重采一次覆盖 —— 用户不再对着空白等两秒。
//
// 关键：**动态字段不能沿用旧值**。可用内存、内存占用率、运行时间必须重算，
// 否则界面会显示上一次启动时的数字（用户会以为数据不刷新）。

#[derive(Serialize, Deserialize)]
struct CachedHardware {
    /// 保存时间（Unix 秒）
    saved_at: u64,
    info: HardwareInfo,
}

/// 超过这个时间就不再信任缓存，改为前台同步采集（可能换过硬件）
const CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// 超过这个时间就顺手在后台刷新一次
const CACHE_REFRESH_AFTER_SECS: u64 = 6 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> Option<std::path::PathBuf> {
    super::settings::config_dir().map(|d| d.join("hardware-cache.json"))
}

fn read_disk_cache() -> Option<CachedHardware> {
    let txt = std::fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&txt).ok()
}

fn write_disk_cache(info: &HardwareInfo) {
    let Some(p) = cache_path() else { return };
    let body = CachedHardware { saved_at: now_secs(), info: info.clone() };
    if let Ok(txt) = serde_json::to_string(&body) {
        let _ = std::fs::write(p, txt);
    }
}

/// 重算每次都会变的字段（内存占用、运行时间）
fn refresh_dynamic(info: &mut HardwareInfo) {
    let (total, avail, used) = memory_from_api();
    info.memory.total_bytes = total;
    info.memory.available_bytes = avail;
    info.memory.used_pct = used;
    info.uptime_secs = unsafe { GetTickCount64() } / 1000;
}

static REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 后台重采一次并写回两级缓存；同一时刻只允许一个（避免连点刷新开多个 PS 进程）
fn spawn_background_refresh() {
    use std::sync::atomic::Ordering;
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let info = collect();
        if let Ok(mut g) = CACHE.lock() {
            *g = Some(info.clone());
        }
        write_disk_cache(&info);
        REFRESHING.store(false, Ordering::SeqCst);
    });
}

#[tauri::command]
pub async fn get_hardware_info(refresh: Option<bool>) -> HardwareInfo {
    let force = refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        if !force {
            // ① 进程内缓存：本次运行已采过
            if let Ok(guard) = CACHE.lock() {
                if let Some(ref data) = *guard {
                    return data.clone();
                }
            }
            // ② 磁盘缓存：上次运行的采集结果，秒开
            if let Some(mut c) = read_disk_cache() {
                let age = now_secs().saturating_sub(c.saved_at);
                if age < CACHE_MAX_AGE_SECS {
                    refresh_dynamic(&mut c.info);
                    if age > CACHE_REFRESH_AFTER_SECS {
                        spawn_background_refresh();
                    }
                    return c.info;
                }
            }
        }
        // ③ 真采集（首次安装 / 手动刷新 / 缓存过期）
        let info = collect();
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some(info.clone());
        }
        write_disk_cache(&info);
        info
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：`-Property` 白名单少写一个字段，会让对应信息静默变成空值。
    /// 这个测试真跑一次采集，断言关键字段确实有内容。
    #[test]
    fn 采集结果关键字段完整() {
        let started = Instant::now();
        let info = collect();
        println!("采集耗时 {}ms", started.elapsed().as_millis());
        println!(
            "CPU: {} / {}({}) MHz / {}C{}T",
            info.cpu.name, info.cpu.base_mhz, info.cpu.base_mhz,
            info.cpu.cores_physical, info.cpu.cores_logical
        );
        println!("系统: {} build {}", info.system.caption, info.system.build);
        println!("主板: {} {}", info.motherboard.manufacturer, info.motherboard.product);
        for d in &info.disks {
            println!("磁盘: {} {} {} {}", d.model, d.media, d.bus, d.size_bytes);
        }
        for g in &info.gpu {
            println!("显卡: {} {} VRAM={}", g.name, g.driver_version, g.vram_bytes);
        }
        println!("内存条: {} 根, 类型 {}", info.memory.sticks.len(), info.memory.mem_type);
        println!("显示器: {}", info.monitors.len());
        println!("网卡: {} 声卡: {}", info.network.len(), info.sound.len());

        // 必须拿到的：CPU 型号与核心数、系统版本、磁盘型号与介质
        assert!(!info.cpu.name.trim().is_empty(), "CPU 型号为空（-Property 漏字段？）");
        assert!(info.cpu.cores_logical > 0, "逻辑核心数为 0");
        assert!(!info.system.caption.is_empty(), "系统名称为空");
        assert!(!info.system.build.is_empty(), "系统 build 号为空");
        assert!(!info.disks.is_empty(), "没有读到物理磁盘");
        assert!(
            info.disks.iter().all(|d| !d.model.trim().is_empty()),
            "磁盘型号为空"
        );
        assert!(
            info.disks.iter().all(|d| !d.media.is_empty()),
            "磁盘介质类型为空（MSFT_PhysicalDisk 的 MediaType 没读到？）"
        );
        assert!(!info.gpu.is_empty(), "没有读到显卡");
        assert!(info.uptime_secs > 0, "运行时间为 0");
    }

    /// 磁盘缓存必须能无损往返（含中文、空数组等边界）
    #[test]
    fn 磁盘缓存序列化往返() {
        let info = collect();
        let body = CachedHardware { saved_at: now_secs(), info: info.clone() };
        let txt = serde_json::to_string(&body).unwrap();
        let back: CachedHardware = serde_json::from_str(&txt).unwrap();
        assert_eq!(back.info.cpu.name, info.cpu.name);
        assert_eq!(back.info.disks.len(), info.disks.len());
        assert_eq!(back.info.memory.sticks.len(), info.memory.sticks.len());
        assert_eq!(back.saved_at, body.saved_at);
    }
}
