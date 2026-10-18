// ============ 类型定义（与未来 Tauri 命令契约对齐） ============

// 'ai' 是设置的二级页，不出现在侧边栏导航里
export type Page = 'hardware' | 'overview' | 'drive' | 'clean' | 'uninstall' | 'settings' | 'ai'

/** 点窗口 ✕ 时的行为：每次询问 / 退到后台（托盘） / 直接关闭 */
export type CloseAction = 'ask' | 'background' | 'exit'

/** 设置页读取的应用设置（真实值来自 Rust 侧，重启后保持） */
export interface AppSettings {
  closeAction: CloseAction
  autostart: boolean
  /** 托盘是否创建成功；为 false 时「退到后台」不可选 */
  trayAvailable: boolean
  appVersion: string
  /** 当前程序路径（自启动指向的文件，便于确认是不是开发态路径） */
  exePath: string
}

export interface Volume {
  letter: string // "C"
  label: string
  totalBytes: number
  usedBytes: number
  freeBytes: number
  media: 'SSD' | 'HDD'
  fs: 'NTFS' | 'exFAT' | 'FAT32'
}

export type NodeKind = 'dir' | 'file'

export interface FileNode {
  name: string
  kind: NodeKind
  size: number // bytes（dir 为子树合计）
  children?: FileNode[]
  category: Category
  mtime: string
}

export type Category =
  | 'video' | 'image' | 'audio' | 'doc'
  | 'archive' | 'app' | 'system' | 'other'

export interface CleanCategory {
  id: string
  name: string
  icon: string
  items: CleanItem[]
}

/** 清理扫描范围：用户勾选的盘符（如 ['C','E']）；不传=默认全盘 */
export interface CleanScope {
  drives: string[]
}

export interface CleanItem {
  id: string
  name: string
  path: string
  size: number
  safe: boolean // false = 系统关键/使用中，不可勾选
  lockReason?: string
  defaultOn?: boolean // false = 高级项，默认不勾选
  drive?: string // 所属盘符（如 "E"）
  readOnly?: boolean // 只读项（系统占用，不可删除）
  warning?: string // 危险提示
  advanced?: boolean // 高级项标记
}

export type AppType = 'win32' | 'store'

export interface InstalledApp {
  id: string
  name: string
  version: string
  publisher: string
  installDate: string // YYYY-MM-DD
  sizeBytes: number
  type: AppType
  bundle: boolean // 疑似捆绑
  system: boolean // 系统组件，默认隐藏
  iconData?: string // 商店应用内联 Logo（data URL）
  uninstalling?: boolean
  uninstalled?: boolean
}

/** 残留类型（Geek / HiBit 全套） */
export type ResidualType =
  | 'registry'   // 注册表键
  | 'folder'     // 程序目录 / 数据目录
  | 'shortcut'   // 开始菜单 / 桌面快捷方式
  | 'startup'    // 注册表开机启动项（Run）
  | 'startupfile'// 启动文件夹内的快捷方式
  | 'service'    // 后台服务键
  | 'task'       // 计划任务
  | 'firewall'   // 防火墙规则
  | 'pathenv'    // PATH 环境变量残留段
  | 'class'      // COM / 文件关联注册表键

export interface ResidualItem {
  id: string
  path: string
  type: ResidualType
  size: number
  selected: boolean
  detail?: string // 残留内容明细
}

// ---- 物理硬盘（含 SMART 寿命） ----
// ---- 物理硬盘（含 SMART 详细数据，对齐 CrystalDiskInfo 能力） ----
export interface SmartAttribute {
  id: number
  name: string
  current: number
  worst: number
  threshold: number
  raw: number
}

export interface PhysicalDisk {
  model: string
  media: string
  bus: string
  sizeBytes: number
  health: string
  wear?: number
  temperature?: number
  powerOnHours?: number
  powerCycles?: number
  readErrors?: number
  writeErrors?: number
  totalWrittenBytes?: number
  totalReadBytes?: number
  availableSpare?: number
  criticalWarning?: number
  serial?: string
  firmware?: string
  partitionCount?: number
  /** 分区表类型 GPT / MBR */
  partitionStyle?: string
  /** 该物理盘上的盘符，如 ["C:"] */
  volumes?: string[]
  /** 转速 RPM；SSD/NVMe 为 0 */
  rotationRate?: number
  /** 传输模式 SATA/600、NVMe ... */
  transferMode?: string
  hostReadCommands?: number
  hostWriteCommands?: number
  /** 是否读到 SMART 阈值表（0xD1）；false 时阈值列不可信 */
  hasThresholds?: boolean
  attributes?: SmartAttribute[]
  /** NVMe 健康项列表（NVMe 无 ATA 属性表时用这个视图） */
  healthItems?: HealthItem[]
  /** 临界温度（部分盘经 STORAGE 属性源提供） */
  criticalTemperature?: number
  source?: string
  predictFailure?: boolean
}

/** NVMe 健康项（CDI 对 NVMe 盘的呈现方式） */
export interface HealthItem {
  label: string
  value: string
  /** 0=正常 1=注意 2=异常 3=信息 */
  level: 0 | 1 | 2 | 3
  raw?: number
}

// ---- 硬件信息 ----
export interface CpuInfo { name: string; coresPhysical: number; coresLogical: number; baseMhz: number }
export interface MemoryStick {
  manufacturer: string
  capacityBytes: number
  speedMhz: number          // 标称频率
  configuredMhz: number     // 实际运行频率
  voltageMv: number         // 当前电压（mV）
  partNumber: string        // 型号编码
  slot: string              // 插槽位置
}
export interface MemoryInfo { totalBytes: number; availableBytes: number; usedPct: number; memType: string; speedMhz: number; sticks: MemoryStick[] }
export interface GpuInfo { name: string; vramBytes: number; driverVersion: string; vendor: string }
export interface MonitorInfo { name: string; manufacturer: string; sizeInch: number }
export interface DiskInfo { model: string; sizeBytes: number; media: string; bus: string }
export interface BoardInfo { manufacturer: string; product: string; biosVersion: string; systemModel: string; systemManufacturer: string }
export interface SystemInfo { caption: string; version: string; build: string; arch: string }
export interface NetInfo { name: string; kind: string }
export interface HardwareInfo {
  computerName: string
  uptimeSecs: number
  cpu: CpuInfo
  memory: MemoryInfo
  gpu: GpuInfo[]
  monitors: MonitorInfo[]
  disks: DiskInfo[]
  motherboard: BoardInfo
  system: SystemInfo
  sound: string[]
  network: NetInfo[]
}

/** 安装包专项清理：一个可删的安装包文件 */
export interface InstallerItem {
  /** 真实绝对路径（删除时原样回传，后端会重新校验） */
  path: string
  name: string
  size: number
  /** 修改时间（Unix 秒） */
  mtime: number
  /** MSI / MSIX / APPX / MSU / EXE */
  kind: string
}

/** 空文件夹扫描：一个完全空的目录 */
export interface EmptyDirItem {
  path: string
  name: string
  depth: number
  mtime: number
}

// ---- 「这是什么」本地识别 ----
/** 判定档位：safe 可清理 / caution 有代价 / danger 不能删 / unknown 判不出来 */
export type Verdict = 'safe' | 'caution' | 'danger' | 'unknown'

export interface IdentifyResult {
  /** 结论标题，如「Steam 游戏本体」 */
  label: string
  /** 2 确定 / 1 较可能 / 0 判不出来 */
  confidence: 0 | 1 | 2
  evidence: string[]
  verdict: Verdict
  /** 这是什么 */
  what: string
  /** 删了会怎样 */
  consequence: string
  source: 'local' | 'ai'
  /** 供云端分析的摘要（不含文件内容） */
  summary: string
  actionHint: 'clean' | 'locate' | 'none'
  /** 采样是否被截断 */
  truncated: boolean
}

// ---- AI ----
export interface AiAnalysis {
  what: string
  canDelete: Verdict
  consequence: string
  suggestion: string
}

export interface AiConfigPublic {
  provider: string
  baseUrl: string
  model: string
  enabled: boolean
  hasKey: boolean
  /** 形如 sk-…a1b2；内置额度时是「内置额度」 */
  keyHint: string
  /** 当前是否在用内置额度 */
  isBuiltin: boolean
}

export interface AiMessage {
  role: 'user' | 'assistant'
  content: string
}
