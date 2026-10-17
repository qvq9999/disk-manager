//! 物理硬盘信息与寿命识别（多来源组合，最大化取到真实数据）
//! ① Get-PhysicalDisk：型号/介质(SSD/HDD)/总线/健康状态
//! ② Get-StorageReliabilityCounter：官方可靠性计数器（磨损/温度/通电时长/读写错误）
//! ③ root\wmi MSStorageDriver_FailurePredictData：SMART 原始属性表（当 ② 无数据时兜底解析
//!    ID 9=通电小时、194=温度、231=SSD 剩余寿命、202=厂商剩余寿命）

use serde::Serialize;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalDisk {
    pub model: String,
    pub media: String,        // SSD / HDD / SCM / 未知
    pub bus: String,          // NVMe / SATA / USB / RAID ...
    pub size_bytes: u64,
    pub health: String,       // Healthy / Warning / Unhealthy / Unknown
    pub wear: Option<u32>,    // 磨损百分比（0-100，越大越接近寿命终点）
    pub temperature: Option<i32>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub read_errors: Option<u64>,
    pub write_errors: Option<u64>,
    /// 累计写入 / 读取量（估算，SMART 原始计数换算）
    pub total_written_bytes: Option<u64>,
    pub total_read_bytes: Option<u64>,
    /// NVMe 可用备用空间与临界警告
    pub available_spare: Option<u32>,
    pub critical_warning: Option<u32>,
    /// 序列号 / 固件版本 / 分区数（Get-Disk）
    pub serial: String,
    pub firmware: String,
    pub partition_count: u32,
    /// 分区表类型（GPT / MBR）
    pub partition_style: String,
    /// 该物理盘上的盘符（如 ["C:"]）
    pub volumes: Vec<String>,
    /// 转速（RPM）；SSD/NVMe 为 0
    pub rotation_rate: Option<u32>,
    /// 传输模式（SATA/600、SATA/300、NVMe ...）
    pub transfer_mode: String,
    /// NVMe 主机读/写命令数
    pub host_read_commands: Option<u64>,
    pub host_write_commands: Option<u64>,
    /// 是否读到 SMART 阈值表（0xD1）；未读到时属性表阈值不可信
    pub has_thresholds: bool,
    /// ATA SMART 完整属性表（CrystalDiskInfo 同款视图；NVMe 盘为空）
    pub attributes: Vec<crate::commands::smart::SmartAttribute>,
    /// NVMe 健康项列表（NVMe 无 ATA 式属性表，用这个视图呈现，对齐 CDI）
    pub health_items: Vec<crate::commands::smart::HealthItem>,
    /// 临界温度（部分盘经属性源提供）
    pub critical_temperature: Option<i32>,
    /// 数据来源标记（SMART直读(ATA/NVMe) / StorageReliability / 未读取到）
    pub source: String,
    /// 预测故障（SMART PredictFailure）
    pub predict_failure: Option<bool>,
}

const PS_DISKS: &str = r#"
$ErrorActionPreference='SilentlyContinue'
try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}
$OutputEncoding=[System.Text.Encoding]::UTF8

# ---- 物理盘元数据（序列号/固件/分区数/分区表类型，来自 Get-Disk）----
$meta = @(Get-Disk | Select-Object Number,SerialNumber,FirmwareVersion,NumberOfPartitions,PartitionStyle,Size)
# ---- 盘号 → 盘符映射（Get-Partition）----
$parts = @(Get-Partition | Where-Object { $_.DriveLetter } | Select-Object DiskNumber,DriveLetter)

# ---- SMART 原始数据（需要管理员权限）----
$smartData = @(Get-CimInstance -Namespace root\wmi -ClassName MSStorageDriver_FailurePredictData -ErrorAction SilentlyContinue)
$smartStat = @(Get-CimInstance -Namespace root\wmi -ClassName MSStorageDriver_FailurePredictStatus -ErrorAction SilentlyContinue)

function Find-SmartData($model, $list) {
  $key = ($model -replace '[^A-Za-z0-9]','').ToUpper()
  if ($key.Length -lt 6) { return $null }
  foreach ($s in $list) {
    $inst = ("$($s.InstanceName)" -replace '[^A-Za-z0-9]','').ToUpper()
    $probe = $key.Substring(0, [Math]::Min(10, $key.Length))
    if ($inst.Contains($probe)) { return $s }
  }
  return $null
}
function Attr($bytes, $id) {
  if ($null -eq $bytes) { return $null }
  for ($i = 2; $i -lt 360; $i += 12) {
    $aid = [int]$bytes[$i]
    if ($aid -eq $id) {
      $raw = @([int]$bytes[$i+5], [int]$bytes[$i+6], [int]$bytes[$i+7], [int]$bytes[$i+8], [int]$bytes[$i+9], [int]$bytes[$i+10])
      return @{ V = [int]$bytes[$i+3]; Raw = $raw }
    }
  }
  return $null
}

$out = @(Get-PhysicalDisk | ForEach-Object {
  $d = $_
  $rc = $null
  try { $rc = $d | Get-StorageReliabilityCounter -ErrorAction SilentlyContinue } catch { $rc = $null }
  $hours = $rc.PowerOnHours
  $temp  = $rc.Temperature
  $wear  = $rc.Wear
  $src   = $(if ($null -ne $hours -or $null -ne $temp -or $null -ne $wear) { "StorageReliability" } else { "" })

  $sd = Find-SmartData $d.FriendlyName $smartData
  $pf = $null
  if ($sd) {
    $b = $sd.VendorSpecific
    if ($null -eq $hours) {
      $a = Attr $b 9
      if ($a) { $hours = $a.Raw[0] + $a.Raw[1]*256 + $a.Raw[2]*65536 + $a.Raw[3]*16777216; $src = "SMART" }
    }
    if ($null -eq $temp) {
      $a = Attr $b 194
      if ($a) { $temp = $a.Raw[0]; if (-not $src) { $src = "SMART" } }
    }
    if ($null -eq $wear) {
      $a231 = Attr $b 231   # SSD Life Left（剩余寿命百分比）
      $a202 = Attr $b 202   # 部分厂商用 202 表示剩余寿命
      if ($a231 -and $a231.V -gt 0 -and $a231.V -le 100) { $wear = 100 - $a231.V; $src = "SMART(231)" }
      elseif ($a202 -and $a202.V -gt 0 -and $a202.V -le 100) { $wear = 100 - $a202.V; $src = "SMART(202)" }
    }
  }
  foreach ($st in $smartStat) {
    $inst = ("$($st.InstanceName)" -replace '[^A-Za-z0-9]','').ToUpper()
    $key = ($d.FriendlyName -replace '[^A-Za-z0-9]','').ToUpper()
    if ($key.Length -ge 6 -and $inst.Contains($key.Substring(0, [Math]::Min(10, $key.Length)))) {
      $pf = $st.PredictFailure; break
    }
  }

  [pscustomobject]@{
    Model=$d.FriendlyName
    Media="$($d.MediaType)"
    Bus="$($d.BusType)"
    Size=$d.Size
    Health="$($d.HealthStatus)"
    DeviceId="$($d.DeviceId)"
    Wear=$wear
    Temp=$temp
    Hours=$hours
    ReadErr=$rc.ReadErrorsTotal
    WriteErr=$rc.WriteErrorsTotal
    Source=$src
    PredictFailure=$pf
  }
})
$metaJson = $meta | ConvertTo-Json -Compress -Depth 3
$partsJson = $parts | ConvertTo-Json -Compress -Depth 3
[pscustomobject]@{ Disks=$out; Meta=$metaJson; Parts=$partsJson } | ConvertTo-Json -Compress -Depth 4
"#;

/// MediaType 可能是字符串（"SSD"/"HDD"）或整数枚举，两种都兼容
fn media_name(v: &serde_json::Value) -> String {
    if let Some(s) = v.get("Media").and_then(|x| x.as_str()) {
        let l = s.to_lowercase();
        if l.contains("ssd") { return "SSD".into(); }
        if l.contains("hdd") { return "HDD".into(); }
        if l.contains("scm") { return "SCM".into(); }
    }
    match js_opt_u64(v, "Media").unwrap_or(0) {
        3 => "HDD".into(),
        4 => "SSD".into(),
        5 => "SCM".into(),
        _ => "未知".into(),
    }
}

/// BusType 同样兼容字符串/整数
fn bus_name(v: &serde_json::Value) -> String {
    if let Some(s) = v.get("Bus").and_then(|x| x.as_str()) {
        let l = s.to_lowercase();
        for (k, name) in [("nvme", "NVMe"), ("sata", "SATA"), ("usb", "USB"), ("raid", "RAID"), ("sas", "SAS"), ("scsi", "SCSI")] {
            if l.contains(k) { return name.into(); }
        }
    }
    match js_opt_u64(v, "Bus").unwrap_or(0) {
        7 => "USB".into(),
        8 => "RAID".into(),
        10 => "SAS".into(),
        11 => "SATA".into(),
        17 => "NVMe".into(),
        _ => String::new(),
    }
}

fn js_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}

fn js_opt_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key)
        .and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

fn js_opt_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|x| x.as_bool().or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true"))))
}

fn js_arr<'a>(v: &'a serde_json::Value) -> Vec<&'a serde_json::Value> {
    match v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![v],
        _ => vec![],
    }
}

#[tauri::command]
pub async fn get_physical_disks() -> Vec<PhysicalDisk> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut result: Vec<PhysicalDisk> = Vec::new();
        let out = crate::commands::silent_command("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", PS_DISKS])
            .output();
        let Ok(o) = out else { return result };
        let text = crate::commands::decode_out(&o.stdout);
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else { return result };

        // 磁盘元数据：盘号 → (序列号, 固件, 分区数, 分区表类型)
        let mut meta: std::collections::HashMap<u32, (String, String, u32, String)> = std::collections::HashMap::new();
        if let Some(mstr) = root.get("Meta").and_then(|x| x.as_str()) {
            if let Ok(mv) = serde_json::from_str::<serde_json::Value>(mstr) {
                for m in js_arr(&mv) {
                    if let Some(num) = js_opt_u64(m, "Number") {
                        meta.insert(
                            num as u32,
                            (
                                js_str(m, "SerialNumber"),
                                js_str(m, "FirmwareVersion"),
                                js_opt_u64(m, "NumberOfPartitions").unwrap_or(0) as u32,
                                js_str(m, "PartitionStyle"),
                            ),
                        );
                    }
                }
            }
        }

        // 盘号 → 盘符列表
        let mut vols: std::collections::HashMap<u32, Vec<String>> = std::collections::HashMap::new();
        if let Some(pstr) = root.get("Parts").and_then(|x| x.as_str()) {
            if let Ok(pv) = serde_json::from_str::<serde_json::Value>(pstr) {
                for p in js_arr(&pv) {
                    let dn = js_opt_u64(p, "DiskNumber");
                    let letter = js_str(p, "DriveLetter");
                    if let (Some(dn), false) = (dn, letter.is_empty()) {
                        vols.entry(dn as u32).or_default().push(format!("{letter}:"));
                    }
                }
            }
        }

        let disks_val = root.get("Disks").cloned().unwrap_or(serde_json::Value::Null);
        let mut covered: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for d in js_arr(&disks_val) {
            let model = js_str(d, "Model");
            if model.is_empty() {
                continue;
            }
            let health_raw = js_str(d, "Health");
            let health = if health_raw.is_empty() || health_raw == "0" { "未知".to_string() } else { health_raw };
            let device_index = js_opt_u64(d, "DeviceId").map(|n| n as u32);

            let mut wear = js_opt_u64(d, "Wear").map(|n| n as u32);
            let mut temperature = js_opt_u64(d, "Temp").map(|n| n as i32);
            let mut power_on_hours = js_opt_u64(d, "Hours");
            let mut source = js_str(d, "Source");
            let mut predict = js_opt_bool(d, "PredictFailure");
            let mut power_cycles = None;
            let mut total_written = None;
            let mut total_read = None;
            let mut spare = None;
            let mut critical = None;
            let mut attributes = Vec::new();
            let mut read_errors = js_opt_u64(d, "ReadErr");
            let write_errors = js_opt_u64(d, "WriteErr");
            // SMART 直读补充的元数据（Get-Disk 取不到时兜底）
            let mut smart_serial: Option<String> = None;
            let mut smart_firmware: Option<String> = None;
            let mut rotation_rate: Option<u32> = None;
            let mut transfer_mode = String::new();
            let mut host_read_commands: Option<u64> = None;
            let mut host_write_commands: Option<u64> = None;
            let mut has_thresholds = false;
            let mut health_items: Vec<crate::commands::smart::HealthItem> = Vec::new();
            let mut critical_temperature: Option<i32> = None;

            // SMART 直读（CrystalDiskInfo 同款）：数据最准最全，覆盖 WMI 值
            if let Some(idx) = device_index {
                covered.insert(idx);
                let s = crate::commands::smart::read_smart(idx);
                let mut used = false;
                if s.wear.is_some() { wear = s.wear; used = true; }
                if s.temperature.is_some() { temperature = s.temperature; used = true; }
                if s.power_on_hours.is_some() { power_on_hours = s.power_on_hours; used = true; }
                if s.power_cycles.is_some() { power_cycles = s.power_cycles; used = true; }
                if s.total_written_bytes.is_some() { total_written = s.total_written_bytes; used = true; }
                if s.total_read_bytes.is_some() { total_read = s.total_read_bytes; used = true; }
                if s.available_spare.is_some() { spare = s.available_spare; used = true; }
                if s.critical_warning.is_some() { critical = s.critical_warning; used = true; }
                if !s.attributes.is_empty() { attributes = s.attributes.clone(); used = true; }
                if !s.kind.is_empty() && s.kind != "未知" { used = true; }
                if s.predict_failure.is_some() { predict = s.predict_failure; used = true; }
                if let Some(v) = s.serial.clone() { if !v.is_empty() { smart_serial = Some(v); } }
                if let Some(v) = s.firmware.clone() { if !v.is_empty() { smart_firmware = Some(v); } }
                if s.rotation_rate.is_some() { rotation_rate = s.rotation_rate; used = true; }
                if let Some(v) = s.transfer_mode.clone() { if !v.is_empty() { transfer_mode = v; } }
                if s.host_read_commands.is_some() { host_read_commands = s.host_read_commands; }
                if s.host_write_commands.is_some() { host_write_commands = s.host_write_commands; }
                if !s.health_items.is_empty() { health_items = s.health_items.clone(); }
                if s.critical_temperature.is_some() { critical_temperature = s.critical_temperature; }
                has_thresholds = s.has_thresholds;
                if used {
                    source = s.source.clone();
                    // SMART 直读的坏道数更准确
                    if s.reallocated_sectors.is_some() || s.pending_sectors.is_some() {
                        let bad = s.reallocated_sectors.unwrap_or(0) + s.pending_sectors.unwrap_or(0);
                        read_errors = Some(bad);
                    }
                } else if source.is_empty() && !s.message.is_empty() {
                    source = format!("未读取到（{}）", s.message);
                }
            }

            let (mut serial, mut firmware, partitions, partition_style) = meta
                .get(&device_index.unwrap_or(9999))
                .cloned()
                .unwrap_or((String::new(), String::new(), 0, String::new()));
            // Get-Disk 取不到时用 SMART 的 IDENTIFY / NVMe Identify 补全
            if serial.trim().is_empty() {
                if let Some(v) = smart_serial {
                    serial = v;
                }
            }
            if firmware.trim().is_empty() {
                if let Some(v) = smart_firmware {
                    firmware = v;
                }
            }
            let volumes = device_index
                .and_then(|idx| vols.get(&idx).cloned())
                .unwrap_or_default();

            result.push(PhysicalDisk {
                model,
                media: media_name(d),
                bus: bus_name(d),
                size_bytes: js_opt_u64(d, "Size").unwrap_or(0),
                health,
                wear,
                temperature,
                power_on_hours,
                power_cycles,
                read_errors,
                write_errors,
                total_written_bytes: total_written,
                total_read_bytes: total_read,
                available_spare: spare,
                critical_warning: critical,
                serial,
                firmware,
                partition_count: partitions,
                partition_style,
                volumes,
                rotation_rate,
                transfer_mode,
                host_read_commands,
                host_write_commands,
                has_thresholds,
                attributes,
                health_items,
                critical_temperature,
                source,
                predict_failure: predict,
            });
        }

        // ---- 兜底枚举：Get-PhysicalDisk 列不到的物理盘（部分 USB 桥接 / RAID / 虚拟盘）----
        // 直接按 PhysicalDriveN 探测容量，能取到容量就说明这块盘真实存在
        for n in 0..32u32 {
            if covered.contains(&n) {
                continue;
            }
            let Some(size) = crate::commands::smart::disk_length(n) else { continue };
            if size == 0 {
                continue;
            }
            let s = crate::commands::smart::read_smart(n);
            let model = s
                .model
                .clone()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| format!("物理磁盘 {n}"));
            let media = match s.rotation_rate {
                Some(0) => "SSD".to_string(),
                Some(_) => "HDD".to_string(),
                None => "未知".to_string(),
            };
            let source = if s.kind == "未知" {
                format!("未读取到（{}）", if s.message.is_empty() { "SMART 不可用" } else { &s.message })
            } else {
                s.source.clone()
            };
            result.push(PhysicalDisk {
                model,
                media,
                bus: match s.kind.as_str() {
                    "NVMe" => "NVMe".into(),
                    "ATA" => "SATA".into(),
                    _ => String::new(),
                },
                size_bytes: size,
                health: "未知".into(),
                wear: s.wear,
                temperature: s.temperature,
                power_on_hours: s.power_on_hours,
                power_cycles: s.power_cycles,
                read_errors: s.reallocated_sectors,
                write_errors: None,
                total_written_bytes: s.total_written_bytes,
                total_read_bytes: s.total_read_bytes,
                available_spare: s.available_spare,
                critical_warning: s.critical_warning,
                serial: s.serial.clone().unwrap_or_default(),
                firmware: s.firmware.clone().unwrap_or_default(),
                partition_count: vols.get(&n).map(|v| v.len() as u32).unwrap_or(0),
                partition_style: meta.get(&n).map(|m| m.3.clone()).unwrap_or_default(),
                volumes: vols.get(&n).cloned().unwrap_or_default(),
                rotation_rate: s.rotation_rate,
                transfer_mode: s.transfer_mode.clone().unwrap_or_default(),
                host_read_commands: s.host_read_commands,
                host_write_commands: s.host_write_commands,
                has_thresholds: s.has_thresholds,
                attributes: s.attributes.clone(),
                health_items: s.health_items.clone(),
                critical_temperature: s.critical_temperature,
                source,
                predict_failure: s.predict_failure,
            });
        }

        result
    })
    .await
    .unwrap_or_default()
}
