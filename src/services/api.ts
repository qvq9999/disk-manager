// ============ 数据服务层（双模式） ============
// Tauri 环境: invoke Rust 命令（真实数据）
// 浏览器预览: 回落 Mock（与 Tauri 命令同契约）
import type { Volume, FileNode, CleanCategory, InstalledApp, ResidualItem, HardwareInfo, PhysicalDisk, AppSettings, CloseAction, InstallerItem, EmptyDirItem, IdentifyResult, AiAnalysis, AiConfigPublic, AiMessage, AiBatchResult } from '../types'
import { Channel } from '@tauri-apps/api/core'

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
const raw = window as unknown as { __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }

async function inv<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!raw.__TAURI_INTERNALS__) throw new Error('Tauri invoke unavailable')
  return raw.__TAURI_INTERNALS__.invoke(cmd, args) as Promise<T>
}

// ---------------- Mock 数据（浏览器预览用） ----------------
const GB = 1024 ** 3

const mockVolumes: Volume[] = [
  { letter: 'C', label: '系统', totalBytes: 200.8 * GB, usedBytes: 127.2 * GB, freeBytes: 73.6 * GB, media: 'SSD', fs: 'NTFS' },
  { letter: 'D', label: '数据盘', totalBytes: 247.0 * GB, usedBytes: 218.3 * GB, freeBytes: 28.7 * GB, media: 'HDD', fs: 'NTFS' },
  { letter: 'E', label: '工作区', totalBytes: 476.9 * GB, usedBytes: 385.1 * GB, freeBytes: 91.8 * GB, media: 'SSD', fs: 'NTFS' },
]

function mk(name: string, kind: 'dir' | 'file', sizeMB: number, category: FileNode['category'], children?: FileNode[]): FileNode {
  const size = kind === 'dir' && children ? children.reduce((s, c) => s + c.size, 0) : sizeMB * 1024 ** 2
  return { name, kind, size, category, mtime: '2026-09-28', children: kind === 'dir' ? children : undefined }
}

const cDrive: FileNode = mk('C:\\', 'dir', 0, 'system', [
  mk('Windows', 'dir', 0, 'system', [
    mk('WinSxS', 'dir', 0, 'system', [mk('组件存储（不可手动删除）', 'file', 8240, 'system')]),
    mk('System32', 'dir', 0, 'system', [mk('驱动与系统库', 'file', 3860, 'system')]),
    mk('SoftwareDistribution', 'dir', 0, 'system', [mk('Download', 'dir', 0, 'system', [mk('更新缓存', 'file', 2140, 'system')])]),
  ]),
  mk('Program Files', 'dir', 0, 'app', [
    mk('Google', 'dir', 0, 'app', [mk('Chrome', 'dir', 0, 'app', [mk('chrome.exe 及运行库', 'file', 720, 'app')])]),
    mk('Microsoft VS Code', 'dir', 0, 'app', [mk('程序文件', 'file', 690, 'app')]),
  ]),
  mk('Users', 'dir', 0, 'other', [
    mk('User', 'dir', 0, 'other', [
      mk('AppData', 'dir', 0, 'other', [
        mk('Local', 'dir', 0, 'other', [
          mk('Temp', 'dir', 0, 'other', [mk('临时文件（可清理）', 'file', 2860, 'other')]),
        ]),
      ]),
      mk('Downloads', 'dir', 0, 'archive', [mk('安装包与压缩包', 'file', 8940, 'archive')]),
      mk('Documents', 'dir', 0, 'doc', [mk('文档', 'file', 1280, 'doc')]),
    ]),
  ]),
  mk('ProgramData', 'dir', 0, 'other', [mk('应用共享数据', 'file', 980, 'other')]),
  mk('pagefile.sys', 'file', 8192, 'system'),
])

const dDrive: FileNode = mk('D:\\', 'dir', 0, 'app', [
  mk('Games', 'dir', 0, 'video', [mk('游戏本体与存档', 'file', 98600, 'video')]),
  mk('Media', 'dir', 0, 'video', [
    mk('Movies', 'dir', 0, 'video', [mk('影片', 'file', 42300, 'video')]),
    mk('Photos', 'dir', 0, 'image', [mk('照片库', 'file', 12800, 'image')]),
  ]),
  mk('Backup', 'dir', 0, 'archive', [mk('历史备份压缩包', 'file', 36700, 'archive')]),
])

const eDrive: FileNode = mk('E:\\', 'dir', 0, 'other', [
  mk('Projects', 'dir', 0, 'doc', [mk('项目工作区', 'file', 21600, 'doc')]),
  mk('VMs', 'dir', 0, 'app', [mk('虚拟机磁盘', 'file', 148000, 'app')]),
  mk('Downloads', 'dir', 0, 'archive', [mk('下载内容', 'file', 67800, 'archive')]),
])

const mockTrees: Record<string, FileNode> = { C: cDrive, D: dDrive, E: eDrive }

export function findNode(root: FileNode, path: string[]): FileNode | null {
  let cur: FileNode = root
  for (const seg of path) {
    const next = cur.children?.find((c) => c.name === seg)
    if (!next) return null
    cur = next
  }
  return cur
}

const mockCleanCategories: CleanCategory[] = [
  {
    id: 'temp', name: '临时文件', icon: 'FileClock',
    items: [
      { id: 't1', name: '用户临时文件', path: '%LOCALAPPDATA%\\Temp', size: 2.86 * GB, safe: true },
      { id: 't2', name: '系统临时文件', path: 'C:\\Windows\\Temp', size: 1.12 * GB, safe: true },
      { id: 't3', name: 'Windows 更新缓存', path: 'C:\\Windows\\SoftwareDistribution\\Download', size: 2.14 * GB, safe: true },
    ],
  },
  {
    id: 'cache', name: '缓存', icon: 'Database',
    items: [
      { id: 'c1', name: 'Edge / Chrome 浏览器缓存', path: '%LOCALAPPDATA%\\...\\Cache', size: 4.13 * GB, safe: true },
      { id: 'c2', name: '缩略图缓存', path: 'thumbcache_*.db', size: 0.38 * GB, safe: true },
      { id: 'c3', name: 'DirectX 着色器缓存', path: '%LOCALAPPDATA%\\D3DSCache', size: 0.62 * GB, safe: true },
    ],
  },
  {
    id: 'appcache', name: '应用缓存（深度扫描）', icon: 'Layers',
    items: [
      { id: 'ac1', name: 'Doubao 缓存', path: '%LOCALAPPDATA%\\Doubao\\User Data\\Default\\Cache', size: 0.09 * GB, safe: true },
      { id: 'ac2', name: 'QuarkCloudDrive 缓存', path: '%LOCALAPPDATA%\\QuarkCloudDrive\\User Data\\component_crx_cache', size: 0.18 * GB, safe: true },
      { id: 'ac3', name: 'NVIDIA Corporation 缓存', path: '%LOCALAPPDATA%\\NVIDIA Corporation\\NVIDIA App\\CefCache', size: 0.09 * GB, safe: true },
      { id: 'ac4', name: 'duowan 缓存', path: '%APPDATA%\\duowan\\yy\\cache', size: 0.09 * GB, safe: true },
      { id: 'ac5', name: 'echo-music 缓存', path: '%APPDATA%\\echo-music\\Cache', size: 0.05 * GB, safe: true },
    ],
  },
  {
    id: 'log', name: '日志与报告', icon: 'ScrollText',
    items: [
      { id: 'l1', name: 'Windows 错误报告', path: 'C:\\ProgramData\\Microsoft\\Windows\\WER', size: 0.44 * GB, safe: true },
      { id: 'l2', name: '系统事件日志归档', path: 'C:\\Windows\\Logs', size: 0.29 * GB, safe: true },
    ],
  },
  {
    id: 'bin', name: '回收站', icon: 'Trash2',
    items: [
      { id: 'b1', name: '回收站（全部盘）', path: '*:\\$Recycle.Bin', size: 12.5 * GB, safe: true },
      { id: 'b2', name: 'pagefile.sys（系统页面文件）', path: 'C:\\pagefile.sys', size: 8.19 * GB, safe: false, lockReason: '系统关键文件，删除会导致系统异常' },
    ],
  },
]

const mockApps: InstalledApp[] = [
  { id: 'a1', name: '网易云音乐', version: '3.1.2', publisher: '网易 Corp.', installDate: '2026-08-14', sizeBytes: 0.62 * GB, type: 'win32', bundle: false, system: false },
  { id: 'a2', name: '网易云音乐助手', version: '1.0.4', publisher: 'NetEase Helper', installDate: '2026-08-14', sizeBytes: 0.04 * GB, type: 'win32', bundle: true, system: false },
  { id: 'a3', name: 'Google Chrome', version: '131.0', publisher: 'Google LLC', installDate: '2026-03-02', sizeBytes: 0.71 * GB, type: 'win32', bundle: false, system: false },
  { id: 'a4', name: 'Microsoft Visual Studio Code', version: '1.96.0', publisher: 'Microsoft', installDate: '2026-05-20', sizeBytes: 0.69 * GB, type: 'win32', bundle: false, system: false },
  { id: 'a5', name: 'Microsoft To Do', version: '2.94', publisher: 'Microsoft', installDate: '2026-04-18', sizeBytes: 0.09 * GB, type: 'store', bundle: false, system: false },
  { id: 'a6', name: 'Spotify - Music and Podcasts', version: '1.231.0', publisher: 'Spotify AB', installDate: '2026-06-30', sizeBytes: 0.16 * GB, type: 'store', bundle: false, system: false },
  { id: 'a7', name: 'Xbox', version: '2509.1', publisher: 'Microsoft', installDate: '2025-11-08', sizeBytes: 0.31 * GB, type: 'store', bundle: false, system: true },
]

/** 浏览器预览用：模拟「已清理」的项，让预览也能看到清理前后的变化 */
const mockCleaned = new Set<string>()
function mockCleanNow(): CleanCategory[] {
  return mockCleanCategories
    .map((c) => ({ ...c, items: c.items.filter((i) => !mockCleaned.has(i.id)) }))
    .filter((c) => c.items.length > 0)
}

function mockScanResiduals(app: InstalledApp): ResidualItem[] {
  const key = app.name.split(' ')[0]
  return app.type === 'store'
    ? [
        { id: 'r1', path: `%LOCALAPPDATA%\\Packages\\${key} (遗留数据)`, type: 'folder', size: app.sizeBytes * 0.42, selected: true },
      ]
    : [
        { id: 'r1', path: `HKCU\\Software\\${key}`, type: 'registry', size: 0, selected: true },
        { id: 'r2', path: `C:\\Program Files\\${key}`, type: 'folder', size: app.sizeBytes * 0.18, selected: true },
        { id: 'r3', path: `%AppData%\\${key}`, type: 'folder', size: app.sizeBytes * 0.32, selected: true },
        { id: 'r4', path: `HKCU\\Run\\${key}Helper`, type: 'startup', size: 0, selected: true },
        { id: 'r5', path: `%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\${key}.lnk`, type: 'shortcut', size: 0, selected: true },
      ]
}

// ---------------- 对外 API（双模式同签名 + 缓存复用） ----------------
// 核心稳定性策略：所有重数据（硬件/软件列表/清理项/目录树）都做「模块级缓存 + 单飞去重」。
// 反复切换侧边栏时直接复用上次结果，绝不重复触发耗时的后端扫描（这是切页卡顿/无响应的根因）。

interface Entry<T> { data: T; at: number }
const store = new Map<string, Entry<unknown>>()
const inflight = new Map<string, Promise<unknown>>()

/**
 * 「会话级」有效期：本次应用运行期间永不过期。
 *
 * 需求：硬盘详细信息、垃圾清理的扫描结果**每次启动只扫一次**，
 * 重复进入页面直接复用，不再重复扫描（扫描一次要读盘/枚举注册表，很贵）。
 * 刷新方式只有两种：① 用户点页面上的「重新扫描 / 重新读取」；② 清理完成后自动失效。
 */
const SESSION_MS = Number.POSITIVE_INFINITY

/** 命中缓存则直接返回；否则发起请求；同一 key 并发请求自动去重（单飞） */
function cached<T>(key: string, ttlMs: number, loader: () => Promise<T>, force = false): Promise<T> {
  const hit = store.get(key) as Entry<T> | undefined
  if (!force && hit && Date.now() - hit.at < ttlMs) return Promise.resolve(hit.data)
  const flying = inflight.get(key) as Promise<T> | undefined
  if (flying) return flying
  const p = loader()
    .then((data) => {
      store.set(key, { data, at: Date.now() })
      return data
    })
    .finally(() => { inflight.delete(key) })
  inflight.set(key, p)
  return p
}

/** 同步读取缓存（组件首帧直接用，避免加载态闪烁） */
function peek<T>(key: string, ttlMs: number): T | null {
  const hit = store.get(key) as Entry<T> | undefined
  return hit && Date.now() - hit.at < ttlMs ? hit.data : null
}

export const TTL = {
  /** 会话级：本次运行只取一次，除非用户手动刷新或清理后失效 */
  volumes: SESSION_MS,   // 盘符容量：启动时枚举一次
  driveTree: 5 * 60_000, // 目录扫描：5 分钟（按需，不预取）
  cleanItems: SESSION_MS, // 清理项体积统计：启动后按盘符组合只扫一次
  apps: 5 * 60_000,      // 已安装软件列表：5 分钟（Get-AppxPackage 很慢）
  hardware: 30 * 60_000, // 硬件信息：30 分钟（用户指定）
  disks: SESSION_MS,     // 物理硬盘 + SMART：启动后只读一次
}

export async function getVolumes(force = false): Promise<Volume[]> {
  if (!isTauri) return mockVolumes
  return cached('volumes', TTL.volumes, () => inv<Volume[]>('get_volumes'), force)
}

export function peekVolumes(): Volume[] | null {
  return isTauri ? peek<Volume[]>('volumes', TTL.volumes) : mockVolumes
}

// ---- 物理硬盘（含 SMART 寿命） ----
const mockPhysicalDisks: PhysicalDisk[] = [
  { model: '示例 SSD 512', media: 'SSD', bus: 'NVMe', sizeBytes: 512 * GB, health: 'Healthy', wear: 3, temperature: 38, powerOnHours: 4210, readErrors: 0, writeErrors: 0 },
  { model: 'KINGSTON SA400S37480G', media: 'SSD', bus: 'SATA', sizeBytes: 480 * GB, health: 'Healthy', wear: 7, temperature: 34, powerOnHours: 9800, readErrors: 0, writeErrors: 0 },
]

export async function getPhysicalDisks(force = false): Promise<PhysicalDisk[]> {
  const loader = isTauri
    ? () => inv<PhysicalDisk[]>('get_physical_disks')
    : () => Promise.resolve(mockPhysicalDisks)
  return cached('disks', TTL.disks, loader, force)
}

export function peekPhysicalDisks(): PhysicalDisk[] | null {
  return peek<PhysicalDisk[]>('disks', TTL.disks)
}

/** 物理硬盘数据是哪一刻读的（用于界面显示「数据读取于 …」）；未读过返回 null */
export function disksFetchedAt(): number | null {
  return store.get('disks')?.at ?? null
}

export function peekDriveTree(letter: string): FileNode | null {
  // 浏览器预览：故意不返回现成结果，让「扫描进度 → 后台继续 → 稍后查看」这段流程也能演示
  if (!isTauri) return null
  return peek<FileNode>(`tree:${letter}`, TTL.driveTree)
}

// ---- 空间分析扫描：可中止 + 可查询进度 ----

/** 扫描进度快照（后端全局原子量，轮询成本极低） */
export interface ScanStatus {
  active: boolean
  entries: number
  current: string
  /** 真正在遍历的盘符：前端据此区分「我在扫」和「我在排队」 */
  letter: string
}

/**
 * 发起/复用一次目录扫描（唯一的扫描入口）。
 *
 * 语义要点：
 * - 同一盘符的并发请求走 inflight 去重，不会真的扫两遍；
 * - 被用户停止时后端返回固定错误码 SCAN_CANCELLED，这里**原样向上抛**，
 *   由扫描任务层区分「用户主动停止」与「真失败」——两者在 UI 上是完全不同的反馈。
 */
export async function requestDriveScan(letter: string, force = false): Promise<FileNode> {
  if (!isTauri) return mockScanRun(letter)
  return cached(`tree:${letter}`, TTL.driveTree, () => inv<FileNode>('scan_drive', { letter }), force)
}

/** 查询当前扫描进度；无扫描在跑时 active=false */
export async function queryScanStatus(): Promise<ScanStatus> {
  if (!isTauri) {
    return { active: mockScan.active, entries: mockScan.entries, current: mockScan.current, letter: 'C' }
  }
  return inv<ScanStatus>('scan_status')
}

/** 请求停止扫描（后端在下一个检查点退出，返回 SCAN_CANCELLED） */
export async function cancelDriveScan(): Promise<void> {
  if (!isTauri) {
    mockScan.cancelled = true
    return
  }
  await inv<void>('cancel_scan')
}

// 浏览器预览：模拟一段可中止的扫描，让「停止 / 后台继续 / 稍后再看」在当前环境下也能验证
const mockScan = { active: false, entries: 0, current: '', cancelled: false }
const MOCK_SCAN_PATHS = [
  'C:\\Windows\\WinSxS',
  'C:\\Program Files',
  'C:\\Users\\User\\AppData',
  'C:\\ProgramData',
  'C:\\Windows\\System32',
]

function mockScanRun(letter: string): Promise<FileNode> {
  mockScan.active = true
  mockScan.entries = 0
  mockScan.cancelled = false
  mockScan.current = `${letter.toUpperCase()}:\\`
  const tree = mockTrees[letter] ?? mockTrees.C
  return new Promise((resolve, reject) => {
    const timer = setInterval(() => {
      if (mockScan.cancelled) {
        clearInterval(timer)
        mockScan.active = false
        reject(new Error('SCAN_CANCELLED'))
        return
      }
      mockScan.entries += 420 + Math.round(Math.random() * 260)
      mockScan.current = MOCK_SCAN_PATHS[Math.floor(mockScan.entries / 900) % MOCK_SCAN_PATHS.length]
      if (mockScan.entries >= 4200) {
        clearInterval(timer)
        mockScan.active = false
        resolve(tree)
      }
    }, 300)
  })
}

/** 清理范围缓存键：按盘符组合区分，`clean:all` 表示默认全盘 */
function cleanKey(drives?: string[]): string {
  return `clean:${drives && drives.length > 0 ? drives.slice().sort().join(',') : 'all'}`
}

/**
 * 最近一次清理扫描使用的盘符组合。
 * 组件切页会被卸载，重新进入时要靠它恢复「上次扫的是哪些盘」，才能命中同一份缓存。
 */
let lastCleanDrives: string[] | null = null

/** 读取上次清理扫描的盘符组合（未扫描过返回 null） */
export function getLastCleanDrives(): string[] | null {
  return lastCleanDrives ? [...lastCleanDrives] : null
}

export async function getCleanCategories(
  force = false,
  drives?: string[],
  onProgress?: (p: { pct: number; current: string }) => void,
): Promise<CleanCategory[]> {
  // 按勾选盘符生成缓存 key，避免不同扫描范围互相污染；不传 drives = 默认全盘
  if (drives && drives.length > 0) lastCleanDrives = drives.slice().sort()
  const loader = isTauri
    ? () => {
        // 深度扫描应用缓存需要数秒，用 Channel 实时汇报进度
        const ch = new Channel<{ pct?: number; current?: string }>()
        if (onProgress) {
          ch.onmessage = (m) => onProgress({ pct: m.pct ?? 0, current: m.current ?? '' })
        }
        return inv<CleanCategory[]>('get_clean_items', {
          scope: drives && drives.length > 0 ? { drives } : null,
          onProgress: ch,
        })
      }
    : async () => {
        // 浏览器预览：模拟深度扫描的进度节奏，便于验证进度 UI
        const steps = [
          '正在扫描：浏览器缓存',
          '正在扫描：应用缓存',
          '正在深度扫描应用缓存…',
          '正在检查已卸载软件的残留…',
          '扫描完成',
        ]
        for (let i = 0; i < steps.length; i++) {
          await new Promise((r) => setTimeout(r, 220))
          onProgress?.({ pct: Math.round(((i + 1) / steps.length) * 100), current: steps[i] })
        }
        return mockCleanNow()
      }
  return cached(cleanKey(drives), TTL.cleanItems, loader, force)
}

export function peekCleanCategories(drives?: string[]): CleanCategory[] | null {
  return peek<CleanCategory[]>(cleanKey(drives), TTL.cleanItems)
}

/** 清理结果是什么时候统计出来的（用于界面显示「上次扫描 …」）；未扫过返回 null */
export function cleanFetchedAt(drives?: string[]): number | null {
  return store.get(cleanKey(drives))?.at ?? null
}

/**
 * 清理完成后失效缓存（下次进入重新统计体积）
 *
 * ⚠️ 只失效「清理项」与「盘符容量」：
 *   - 清理项体积必须重算（这是需求要求的「实时刷新」）
 *   - 盘符可用空间变了，需要重取
 *   - **物理硬盘 SMART 不失效**（清理不会改变固件计数器，没必要重扫）
 */
export function invalidateAfterClean() {
  // 清理项缓存 key 形如 clean:all / clean:C,E —— 全部按前缀清掉
  for (const k of Array.from(store.keys())) {
    if (k === 'clean' || k.startsWith('clean:')) store.delete(k)
  }
  store.delete('volumes')
}

export async function runClean(
  ids: string[],
  onProgress?: (p: { pct?: number; current?: string; freed?: number }) => void,
  drives?: string[],
  /** 前端已算好的待清理总量：传了就不必在后端重扫一遍（深度扫描后这次重扫要 1~4 秒） */
  expectedTotal?: number,
): Promise<[number, number]> {
  if (isTauri) {
    // 真实删除进度：Rust 端通过 Channel 实时推送 {freed, current}
    const ch = new Channel<{ pct?: number; current?: string; freed?: number }>()
    if (onProgress) {
      ch.onmessage = (msg) => onProgress({ pct: msg.pct ?? undefined, current: msg.current, freed: msg.freed ?? undefined })
    }
    // 清理范围必须与扫描范围一致，否则「扫描 E: 却删 C: 的项」
    const scope = drives && drives.length > 0 ? { drives } : null
    return inv<[number, number]>('run_clean', {
      ids,
      scope,
      onProgress: ch,
      expectedTotal: expectedTotal ?? null,
    })
  }
  // 浏览器预览：模拟平滑进度（并记录已清理项，使「清理后重新统计」能看到变化）
  return new Promise((resolve) => {
    const total = mockCleanCategories.flatMap((c) => c.items).filter((i) => ids.includes(i.id)).reduce((s, i) => s + i.size, 0)
    let p = 0
    const timer = setInterval(() => {
      p = Math.min(p + 8, 96)
      onProgress?.({ pct: p })
      if (p >= 96) {
        clearInterval(timer)
        ids.forEach((id) => mockCleaned.add(id))
        resolve([total, 0])
      }
    }, 120)
  })
}

// ---------------- 安装包专项清理 ----------------

/** 浏览器预览用的假数据（真实环境走 Rust 扫描） */
const mockInstallers: InstallerItem[] = [
  { path: 'C:\\Users\\User\\Downloads\\WeChatSetup.exe', name: 'WeChatSetup.exe', size: 182 * 1024 ** 2, mtime: 1758000000, kind: 'EXE' },
  { path: 'C:\\Users\\User\\Downloads\\VSCodeSetup-x64.exe', name: 'VSCodeSetup-x64.exe', size: 96 * 1024 ** 2, mtime: 1757000000, kind: 'EXE' },
  { path: 'D:\\装机备份\\Office2019_x64.msi', name: 'Office2019_x64.msi', size: 1240 * 1024 ** 2, mtime: 1755000000, kind: 'MSI' },
  { path: 'D:\\装机备份\\DriverBoostSetup.exe', name: 'DriverBoostSetup.exe', size: 310 * 1024 ** 2, mtime: 1753000000, kind: 'EXE' },
]

/** 扫描所选盘符下的安装包（.msi/.msix/.appx/.msu + 文件名含 setup/install 的 exe） */
export async function scanInstallers(
  drives: string[],
  onProgress?: (p: { pct: number; current: string }) => void,
): Promise<InstallerItem[]> {
  if (!isTauri) {
    return new Promise((resolve) => {
      let p = 0
      const timer = setInterval(() => {
        p = Math.min(p + 20, 100)
        onProgress?.({ pct: p, current: `正在扫描 ${drives[0] ?? 'C'}:\\…` })
        if (p >= 100) {
          clearInterval(timer)
          resolve(mockInstallers)
        }
      }, 120)
    })
  }
  const ch = new Channel<{ pct?: number; current?: string }>()
  if (onProgress) ch.onmessage = (m) => onProgress({ pct: m.pct ?? 0, current: m.current ?? '' })
  return inv<InstallerItem[]>('scan_installers', {
    scope: drives.length > 0 ? { drives } : null,
    onProgress: ch,
  })
}

/** 让后端停止扫描（已扫到一半的结果不会返回，避免半成品列表） */
export async function cancelInstallerScan(): Promise<void> {
  if (!isTauri) return
  try {
    await inv<void>('cancel_installer_scan')
  } catch { /* 可能刚好已结束 */ }
}

export async function deleteInstallers(
  paths: string[],
  onProgress?: (p: { pct: number; current: string; freed: number }) => void,
): Promise<[number, number]> {
  if (!isTauri) {
    const total = mockInstallers.filter((i) => paths.includes(i.path)).reduce((s, i) => s + i.size, 0)
    return [total, 0]
  }
  const ch = new Channel<{ pct?: number; current?: string; freed?: number }>()
  if (onProgress) {
    ch.onmessage = (m) => onProgress({ pct: m.pct ?? 0, current: m.current ?? '', freed: m.freed ?? 0 })
  }
  return inv<[number, number]>('delete_installers', { paths, onProgress: ch })
}

// ---------------- 空文件夹扫描 ----------------

const mockEmptyDirs: EmptyDirItem[] = [
  { path: 'C:\\Users\\User\\AppData\\Local\\Temp\\old-cache', name: 'old-cache', depth: 5, mtime: 1750000000 },
  { path: 'D:\\Games\\Leftover\\Empty', name: 'Empty', depth: 3, mtime: 1749000000 },
  { path: 'D:\\Backup\\2024\\截图', name: '截图', depth: 3, mtime: 1748000000 },
]

export async function scanEmptyDirs(
  drives: string[],
  onProgress?: (p: { pct: number; current: string }) => void,
): Promise<EmptyDirItem[]> {
  if (!isTauri) {
    return new Promise((resolve) => {
      let p = 0
      const timer = setInterval(() => {
        p = Math.min(p + 16, 100)
        onProgress?.({ pct: p, current: `正在检查 ${drives[0] ?? 'C'}:\\…` })
        if (p >= 100) {
          clearInterval(timer)
          resolve(mockEmptyDirs)
        }
      }, 110)
    })
  }
  const ch = new Channel<{ pct?: number; current?: string }>()
  if (onProgress) ch.onmessage = (m) => onProgress({ pct: m.pct ?? 0, current: m.current ?? '' })
  return inv<EmptyDirItem[]>('scan_empty_dirs', {
    scope: drives.length > 0 ? { drives } : null,
    onProgress: ch,
  })
}

export async function cancelEmptyDirScan(): Promise<void> {
  if (!isTauri) return
  try {
    await inv<void>('cancel_empty_dir_scan')
  } catch { /* 可能刚好已结束 */ }
}

export async function deleteEmptyDirs(
  paths: string[],
  onProgress?: (p: { pct: number; current: string }) => void,
): Promise<[number, number]> {
  if (!isTauri) return [paths.length, 0]
  const ch = new Channel<{ pct?: number; current?: string }>()
  if (onProgress) ch.onmessage = (m) => onProgress({ pct: m.pct ?? 0, current: m.current ?? '' })
  return inv<[number, number]>('delete_empty_dirs', { paths, onProgress: ch })
}

export async function getInstalledApps(force = false): Promise<InstalledApp[]> {
  if (!isTauri) return mockApps
  return cached('apps', TTL.apps, () => inv<InstalledApp[]>('list_apps'), force)
}

/** Win32 软件（注册表枚举，很快）：先渲染它，商店应用随后异步补齐 */
export async function getWin32Apps(force = false): Promise<InstalledApp[]> {
  if (!isTauri) return mockApps.filter((a) => a.type === 'win32')
  return cached('apps:win32', TTL.apps, () => inv<InstalledApp[]>('list_win32_apps'), force)
}

/** 微软商店应用（Get-AppxPackage，较慢） */
export async function getStoreApps(force = false): Promise<InstalledApp[]> {
  if (!isTauri) return mockApps.filter((a) => a.type === 'store')
  return cached('apps:store', TTL.apps, () => inv<InstalledApp[]>('list_store_apps_only'), force)
}

export function peekInstalledApps(): InstalledApp[] | null {
  if (!isTauri) return mockApps
  const win32 = peek<InstalledApp[]>('apps:win32', TTL.apps)
  const store = peek<InstalledApp[]>('apps:store', TTL.apps)
  if (!win32 && !store) return peek<InstalledApp[]>('apps', TTL.apps)
  return [...(win32 ?? []), ...(store ?? [])]
}

/** 卸载/清理残留后失效软件列表缓存 */
export function invalidateApps() {
  store.delete('apps')
  store.delete('apps:win32')
  store.delete('apps:store')
}

export interface UninstallOutcome {
  ok: boolean
  message: string
  stillInstalled: boolean
  /** 卸载程序仍在运行（多半是交互向导）：前端应改为轮询 checkInstalled，而不是继续等待 */
  pending?: boolean
}

export async function uninstallApp(id: string, silentFirst = false): Promise<UninstallOutcome> {
  if (!isTauri) {
    // 浏览器预览：模拟「卸载器弹了交互向导」的情形，好把等待/取消这段 UI 也跑出来
    await new Promise((r) => setTimeout(r, 700))
    return {
      ok: false,
      message: '卸载程序已启动，正在等待你在卸载向导中完成操作（预览环境模拟）',
      stillInstalled: true,
      pending: true,
    }
  }
  return inv<UninstallOutcome>('uninstall_app', { id, silentFirst })
}

/** 让后端停止「等待卸载完成」（不会终止交互式卸载器，避免装到一半） */
export async function cancelUninstall(): Promise<void> {
  if (!isTauri) return
  try {
    await inv<void>('cancel_uninstall')
  } catch { /* 忽略：可能刚好已经结束了 */ }
}

/** 按 id 精确查询软件是否还装着（轮询用，比重新拉整个列表快得多） */
export async function checkInstalled(id: string): Promise<boolean> {
  if (!isTauri) return false
  try {
    return await inv<boolean>('check_installed', { id })
  } catch {
    return true // 查询失败时保守认为仍装着，避免误报「已卸载」
  }
}

/** Geek 式强制卸载：删除程序文件与注册表（用于自带卸载器损坏的软件） */
export async function forceRemoveApp(id: string): Promise<UninstallOutcome> {
  if (!isTauri) return { ok: true, message: '（浏览器预览：模拟强制卸载完成）', stillInstalled: false }
  return inv<UninstallOutcome>('force_remove_app', { id })
}

export async function scanResidualsFor(app: InstalledApp): Promise<ResidualItem[]> {
  return isTauri ? inv<ResidualItem[]>('scan_app_residuals', { appName: app.name }) : mockScanResiduals(app)
}

export async function cleanResiduals(items: ResidualItem[]): Promise<[number, number]> {
  if (!isTauri) {
    return [items.filter((i) => i.selected).reduce((s, i) => s + i.size, 0), 0]
  }
  return inv<[number, number]>('clean_residuals', { items })
}

/** 软件图标缓存：同一软件只提取一次，切页不重复请求 */
const iconCache = new Map<string, string | null>()

export async function getAppIcon(id: string): Promise<string | null> {
  if (!isTauri) return null
  if (iconCache.has(id)) return iconCache.get(id) ?? null
  try {
    const url = await inv<string | null>('get_app_icon', { id })
    iconCache.set(id, url)
    return url
  } catch {
    iconCache.set(id, null)
    return null
  }
}

const mockHardware: HardwareInfo = {
  computerName: 'DESKTOP-PREVIEW',
  uptimeSecs: 37864,
  cpu: { name: '示例 CPU（浏览器预览为模拟数据）', coresPhysical: 8, coresLogical: 16, baseMhz: 3600 },
  memory: {
    totalBytes: 32 * GB, availableBytes: 18 * GB, usedPct: 44, memType: 'DDR4', speedMhz: 3600,
    sticks: [
      { manufacturer: '示例厂商', capacityBytes: 16 * GB, speedMhz: 3200, configuredMhz: 2667, voltageMv: 1200, partNumber: 'SAMPLE-16G-3200', slot: 'DIMM 0' },
      { manufacturer: '示例厂商', capacityBytes: 16 * GB, speedMhz: 3200, configuredMhz: 2667, voltageMv: 1200, partNumber: 'SAMPLE-16G-3200', slot: 'DIMM 1' },
    ],
  },
  gpu: [{ name: '示例显卡 RTX 3060 Ti', vramBytes: 8 * GB, driverVersion: '00.0.0000.0', vendor: 'NVIDIA' }],
  monitors: [{ name: 'P2410R3', manufacturer: '示例显示器厂', sizeInch: 24 }],
  disks: [{ model: '示例 SSD 1TB', sizeBytes: 1000 * GB, media: 'SSD', bus: 'NVMe' }],
  // 故意让 systemModel 与 product 相同：部分主板（如 JGINYUE B450I）的 BIOS 就是这么写的，
  // 预览里保留这个情形，防止「型号显示两遍」的修复被后来的改动悄悄改回去。
  motherboard: { manufacturer: '示例主板厂', product: 'B450I-PLUS/ARGB', biosVersion: 'AMI v1.0', systemModel: 'B450I-PLUS/ARGB', systemManufacturer: '示例主板厂' },
  system: { caption: 'Microsoft Windows 11 专业版', version: '10.0.26100', build: '26100', arch: '64 位' },
  sound: ['High Definition Audio 设备', '示例 USB 耳机'],
  network: [{ name: '示例 Wi-Fi 6E 网卡', kind: '无线' }, { name: '示例 2.5GbE 网卡', kind: '有线' }],
}

// ---- 硬件信息缓存策略（用户指定规则）----
// 有效期：30 分钟。失效/刷新条件：① 关闭重开软件 ② 手动刷新按钮（force） ③ 超 30 分钟自动重扫。
// 命中缓存零扫描、零等待（与上方通用 cached() 共用同一 store）。
const HW_TTL_MS = TTL.hardware

/** 缓存时间戳（页面显示"数据读取于 …"用） */
export function hardwareCacheInfo(): { cached: boolean; at: number; ttlMs: number } {
  const hit = store.get('hardware') as Entry<HardwareInfo> | undefined
  return { cached: !!hit, at: hit?.at ?? 0, ttlMs: HW_TTL_MS }
}

/** 同步读取已缓存结果（组件首帧直接用，避免加载态闪烁） */
export function peekHardwareInfo(): HardwareInfo | null {
  return isTauri ? peek<HardwareInfo>('hardware', HW_TTL_MS) : mockHardware
}

/** 本机真实硬件信息（CPU/内存/显卡/显示器/磁盘/主板/系统/声卡/网卡） */
export async function getHardwareInfo(force = false): Promise<HardwareInfo> {
  if (!isTauri) return mockHardware
  return cached('hardware', HW_TTL_MS, () => inv<HardwareInfo>('get_hardware_info', { refresh: force }), force)
}

/** 当前是否以管理员权限运行 */
export async function isElevated(): Promise<boolean> {
  if (!isTauri) return false
  try {
    return await inv<boolean>('is_elevated')
  } catch {
    return false
  }
}

/** 以管理员身份重启程序（触发 UAC） */
export async function restartAsAdmin(): Promise<string> {
  if (!isTauri) return '（浏览器预览不支持提权）'
  return inv<string>('restart_as_admin')
}

/** 应用启动预热：立即后台预取重数据，用户切页时通常已就绪（不阻塞界面） */
export function warmUp() {
  if (!isTauri) return
  // 先快后慢：盘符 + 硬件信息（两者都是单次 PS 调用）
  setTimeout(() => {
    getHardwareInfo().catch(() => {})
    getVolumes().catch(() => {})
    getPhysicalDisks().catch(() => {})
  }, 60)
  // 软件列表随后预热（Get-AppxPackage 较慢，放到最后）
  setTimeout(() => {
    getWin32Apps().catch(() => {})
    getStoreApps().catch(() => {})
  }, 1500)
}

// ---------------- 应用设置（关闭行为 / 开机自启动） ----------------
//
// 真值在 Rust 侧（见 commands/settings.rs）：点 ✕ 的那一刻必须由 Rust 同步决策，
// 这里只负责把设置页的读写转过去。

const MOCK_SETTINGS_KEY = 'dm.mock.settings'

function mockSettings(): AppSettings {
  let closeAction: CloseAction = 'ask'
  let autostart = false
  let autoAnalyzeLayer = false
  let allowPageContext = true
  // 预览环境同样先弹一次，方便验证免责声明交互；清 localStorage 即复位
  let disclaimerAccepted = false
  try {
    const raw = localStorage.getItem(MOCK_SETTINGS_KEY)
    if (raw) {
      const v = JSON.parse(raw) as {
        closeAction?: CloseAction
        autostart?: boolean
        autoAnalyzeLayer?: boolean
        allowPageContext?: boolean
        disclaimerAccepted?: boolean
      }
      if (v.closeAction === 'ask' || v.closeAction === 'background' || v.closeAction === 'exit') closeAction = v.closeAction
      autostart = !!v.autostart
      if (typeof v.autoAnalyzeLayer === 'boolean') autoAnalyzeLayer = v.autoAnalyzeLayer
      if (typeof v.allowPageContext === 'boolean') allowPageContext = v.allowPageContext
      if (typeof v.disclaimerAccepted === 'boolean') disclaimerAccepted = v.disclaimerAccepted
    }
  } catch { /* 隐私模式下 localStorage 不可用，用默认值 */ }
  return {
    closeAction,
    autostart,
    // 预览环境没有真实托盘；这里给 true 只是为了让设置页能把完整交互渲染出来
    trayAvailable: true,
    appVersion: __APP_VERSION__,
    exePath: '（浏览器预览，无真实程序路径）',
    autoAnalyzeLayer,
    allowPageContext,
    disclaimerAccepted,
  }
}

function mockWriteSettings(patch: Partial<AppSettings>) {
  const cur = mockSettings()
  const next = { ...cur, ...patch }
  try {
    localStorage.setItem(MOCK_SETTINGS_KEY, JSON.stringify(next))
  } catch { /* 忽略 */ }
}

export async function getAppSettings(): Promise<AppSettings> {
  if (!isTauri) return mockSettings()
  return inv<AppSettings>('get_app_settings')
}

export async function updateCloseAction(action: CloseAction): Promise<void> {
  if (!isTauri) {
    mockWriteSettings({ closeAction: action })
    return
  }
  await inv<void>('update_close_action', { action })
}

/** 更新 AI 相关的两个开关，返回写入后的真实状态 */
export async function updatePrefs(patch: {
  autoAnalyzeLayer?: boolean
  allowPageContext?: boolean
}): Promise<AppSettings> {
  if (!isTauri) {
    const cur = mockSettings()
    mockWriteSettings({ ...cur, ...patch })
    return { ...cur, ...patch }
  }
  return inv<AppSettings>('update_prefs', {
    autoAnalyzeLayer: patch.autoAnalyzeLayer,
    allowPageContext: patch.allowPageContext,
  })
}

/** 记录「已阅读并同意免责声明」；返回写入后的真实状态 */
export async function acceptDisclaimer(): Promise<boolean> {
  if (!isTauri) {
    mockWriteSettings({ disclaimerAccepted: true })
    return true
  }
  return inv<boolean>('accept_disclaimer')
}

/** 返回写入后的真实状态（Rust 会回读注册表，不假设写成功） */export async function updateAutostart(enabled: boolean): Promise<boolean> {
  if (!isTauri) {
    mockWriteSettings({ autostart: enabled })
    return enabled
  }
  return inv<boolean>('update_autostart', { enabled })
}

/** 关闭弹窗里用户做出选择后调用 */
export async function resolveClose(action: 'background' | 'exit', remember: boolean): Promise<void> {
  if (!isTauri) {
    if (remember) mockWriteSettings({ closeAction: action })
    return
  }
  await inv<void>('resolve_close', { action, remember })
}

/**
 * 监听「用户点了窗口 ✕」。
 * Rust 已 prevent_close，窗口还在，等前端给出选择。
 */
export function onCloseRequested(cb: () => void): () => void {
  if (!isTauri) return () => {}
  let unlisten: (() => void) | null = null
  let disposed = false
  import('@tauri-apps/api/event')
    .then(({ listen }) => listen('close-requested', () => cb()))
    .then((un) => {
      // 组件可能在 listen 完成前就卸载了
      if (disposed) un()
      else unlisten = un
    })
    .catch(() => { /* 事件系统不可用时静默降级 */ })
  return () => {
    disposed = true
    unlisten?.()
  }
}

// ---------------- 「这是什么」本地识别 ----------------
//
// 结果按路径缓存：同一个目录反复点开不重复扫盘。
// 注意缓存不过期是**有意**的 —— 识别结论来自路径特征与文件头，短时间内不会变；
// 用户真想重算就刷新页面。缓存上限 300 条，防止长时间浏览把内存撑大。

const identifyCache = new Map<string, IdentifyResult>()

export async function identifyNode(path: string, kind: 'dir' | 'file', size: number): Promise<IdentifyResult> {
  const key = `${kind}:${path}`
  const hit = identifyCache.get(key)
  if (hit) return hit

  const result = isTauri
    ? await inv<IdentifyResult>('identify_node', { path, kind, size })
    : mockIdentify(path, kind, size)

  if (identifyCache.size > 300) {
    const first = identifyCache.keys().next().value
    if (first) identifyCache.delete(first)
  }
  identifyCache.set(key, result)
  return result
}

/** 预览环境：只按路径里最明显的几个特征给结论，够把界面跑通 */
function mockIdentify(path: string, kind: 'dir' | 'file', size: number): IdentifyResult {
  const p = path.toLowerCase()
  const mk = (
    label: string, verdict: IdentifyResult['verdict'], what: string, consequence: string, conf: 0 | 1 | 2 = 2,
  ): IdentifyResult => ({
    label, confidence: conf, verdict, what, consequence,
    evidence: ['（浏览器预览：模拟判定）'], source: 'local', summary: '', actionHint: verdict === 'safe' ? 'clean' : 'none', truncated: false,
  })

  if (kind === 'file') {
    const fn = path.split(/[\\/]/).pop()?.toLowerCase() ?? ''
    if (fn === 'pagefile.sys') {
      return mk('系统页面文件（虚拟内存）', 'danger', 'Windows 把内存里暂时不用的数据换到这里的磁盘文件', '不能删除。删了会失去虚拟内存、大程序容易崩。要调整大小请到「系统属性 - 高级 - 性能 - 虚拟内存」里改。')
    }
    if (fn === 'hiberfil.sys') {
      return mk('休眠文件', 'danger', '休眠时把整个内存内容写到磁盘，用于快速恢复现场', '不能直接删。不用休眠的话，可在管理员命令行执行 powercfg -h off 回收空间。')
    }
    const ext = fn.includes('.') ? fn.split('.').pop() ?? '' : ''
    return mk(`${ext || '未知'} 文件`, 'caution', '一个普通文件', '删前确认路径与用途。', 1)
  }
  if (/^[a-z]:\\?$/i.test(path)) {
    const sys = path.toLowerCase().startsWith('c:')
    return mk(
      sys ? '系统盘根目录' : '分区根目录',
      'caution',
      sys ? 'Windows 所在的分区根目录，系统、程序与用户数据都在它下面' : '这个分区的顶层目录',
      '根目录本身不能整体处理，展开到子目录逐个判断。',
    )
  }
  if (p.includes('steamapps')) return mk('Steam 游戏本体', 'danger', 'Steam 下载安装的游戏文件', '删了要重新下载几十 GB。')
  if (p.includes('node_modules')) return mk('Node.js 项目依赖', 'safe', 'npm / pnpm / yarn 安装的第三方包', '可安全删除，重新装依赖即可恢复。')
  if (p.includes('temp')) return mk('临时文件', 'safe', '程序运行时的临时文件', '可以删，被占用的会自动跳过。')
  if (p.includes('node_modules') || p.includes('cache')) return mk('缓存目录', 'safe', '应用缓存', '可以删，下次启动会重新生成。')
  return mk('普通文件夹', 'unknown', '没有命中已知特征', '判不出来它属于什么用途，删前请展开确认。', 0)
}

/** 在资源管理器中定位（真实窗口操作，不是网页行为） */
export async function revealInExplorer(path: string): Promise<void> {
  if (!isTauri) return
  await inv<void>('reveal_in_explorer', { path })
}

/** 用系统默认浏览器打开链接 */
export async function openExternal(url: string): Promise<void> {
  if (!isTauri) {
    window.open(url, '_blank', 'noopener,noreferrer')
    return
  }
  await inv<void>('open_external', { url })
}

// ---------------- AI ----------------

const MOCK_AI_KEY = 'dm.mock.ai'

const MOCK_BUILTIN: AiConfigPublic = {
  provider: 'builtin',
  baseUrl: 'https://api.agnes-ai.cn/v1',
  model: 'agnes-3.0-flash',
  enabled: true,
  hasKey: true,
  keyHint: '内置额度',
  isBuiltin: true,
}

function mockAiConfig(): AiConfigPublic {
  try {
    const raw = localStorage.getItem(MOCK_AI_KEY)
    if (raw) return { ...MOCK_BUILTIN, ...(JSON.parse(raw) as Partial<AiConfigPublic>) }
  } catch { /* 忽略 */ }
  return MOCK_BUILTIN
}

export async function aiGetConfig(): Promise<AiConfigPublic> {
  if (!isTauri) return mockAiConfig()
  return inv<AiConfigPublic>('ai_get_config')
}

export async function aiSaveConfig(patch: {
  provider: string
  baseUrl: string
  model: string
  enabled: boolean
  /** 传 undefined 表示不修改已保存的 Key；传空串表示清除 */
  apiKey?: string
}): Promise<AiConfigPublic> {
  if (!isTauri) {
    const prev = mockAiConfig()
    const builtin = patch.provider === 'builtin'
    const next: AiConfigPublic = {
      provider: patch.provider,
      baseUrl: patch.baseUrl,
      model: patch.model,
      enabled: patch.enabled,
      hasKey: builtin ? true : patch.apiKey === undefined ? prev.hasKey : patch.apiKey.length > 0,
      keyHint: builtin ? '内置额度' : prev.keyHint,
      isBuiltin: builtin,
    }
    try { localStorage.setItem(MOCK_AI_KEY, JSON.stringify(next)) } catch { /* 忽略 */ }
    return next
  }
  return inv<AiConfigPublic>('ai_save_config', {
    provider: patch.provider,
    baseUrl: patch.baseUrl,
    model: patch.model,
    enabled: patch.enabled,
    apiKey: patch.apiKey ?? null,
  })
}

export async function aiClearKey(): Promise<AiConfigPublic> {
  if (!isTauri) {
    try { localStorage.removeItem(MOCK_AI_KEY) } catch { /* 忽略 */ }
    return mockAiConfig()
  }
  return inv<AiConfigPublic>('ai_clear_key')
}

export async function aiTestConnection(baseUrl: string, model: string, apiKey?: string): Promise<string> {
  if (!isTauri) {
    await new Promise((r) => setTimeout(r, 600))
    return '（浏览器预览：模拟连接成功）'
  }
  return inv<string>('ai_test_connection', { baseUrl, model, apiKey: apiKey ?? null })
}

export async function aiAnalyze(req: {
  path: string
  kind: 'dir' | 'file'
  size: number
  summary: string
  localLabel: string
  localVerdict: string
}): Promise<AiAnalysis> {
  if (!isTauri) {
    await new Promise((r) => setTimeout(r, 700))
    return {
      what: '（浏览器预览的模拟结果）这个目录看起来像某个软件的本地数据。',
      canDelete: 'caution',
      consequence: '删掉之后该软件的登录状态与个人设置可能会丢失。',
      suggestion: '建议先用该软件自带的清理功能，不要直接删目录。',
    }
  }
  return inv<AiAnalysis>('ai_analyze', { req })
}

export async function aiChat(messages: AiMessage[], context: string): Promise<string> {
  if (!isTauri) {
    await new Promise((r) => setTimeout(r, 600))
    return '（浏览器预览环境没有真实模型）当前问题没有命中本地问答库。配置 AI 后可以问更复杂的问题。'
  }
  return inv<string>('ai_chat', { messages, context })
}

/**
 * 流式问答：`onDelta` 每收到一小段就被调用一次，前端边收边渲染。
 * 返回完整文本，用于在流结束时校正界面（丢包时以返回值为准）。
 */
export async function aiChatStream(
  messages: AiMessage[],
  context: string,
  onDelta: (t: string) => void,
): Promise<string> {
  if (!isTauri) {
    // 浏览器预览：模拟打字机，让渲染逻辑也能被验证
    const demo =
      '（浏览器预览环境没有真实模型）这是一段模拟的流式输出，' +
      '用来验证逐字渲染、自动滚动和结束态是否正确。真实回答会由模型逐字推送。'
    let acc = ''
    for (const ch of demo) {
      acc += ch
      onDelta(ch)
      await new Promise((r) => setTimeout(r, 16))
    }
    return acc
  }
  const { Channel } = await import('@tauri-apps/api/core')
  const ch = new Channel<string>()
  ch.onmessage = (t: string) => onDelta(t)
  return inv<string>('ai_chat_stream', { messages, context, onChunk: ch })
}

/** 生成「接下来可能会问」的问题，失败时返回空数组（不打扰用户） */
export async function aiSuggest(question: string, answer: string): Promise<string[]> {
  if (!isTauri) {
    return ['这个目录删了会怎样', '怎么安全地清理它']
  }
  try {
    return await inv<string[]>('ai_suggest', { question, answer })
  } catch {
    return []
  }
}

/** 一次分析同一层下的多个子项（比逐个调用省 token，且模型能看到同层其它项） */
export async function aiAnalyzeBatch(items: {
  name: string
  path: string
  kind: 'dir' | 'file'
  size: number
  summary: string
}[]): Promise<AiBatchResult[]> {
  if (!isTauri) {
    await new Promise((r) => setTimeout(r, 900))
    return items.map((i) => ({
      name: i.name,
      what: '（浏览器预览的模拟结论）未接入真实模型',
      canDelete: 'unknown' as const,
      consequence: '',
      suggestion: '在真实环境中这里会是模型给出的判断',
    }))
  }
  return inv<AiBatchResult[]>('ai_analyze_batch', { items })
}
