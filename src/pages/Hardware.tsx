import { useEffect, useState } from 'react'
import {
  Cpu, MemoryStick, Monitor, CircuitBoard, HardDrive, MonitorCog, Volume2, Wifi,
  RefreshCw, Clock, Info,
} from 'lucide-react'
import type { HardwareInfo } from '../types'
import { getHardwareInfo, hardwareCacheInfo, peekHardwareInfo } from '../services/api'
import { PageLoading } from '../components/Loading'
import { fmtBytes } from '../utils/format'

function fmtUptime(secs: number): string {
  if (!secs || secs <= 0) return '—'
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = Math.floor(secs % 60)
  return `${d}天${h}小时${m}分钟${s}秒`
}

/** 小胶囊标签：把「核心/线程/频率」这类并列参数压成一行 */
function Chip({ children }: { children: React.ReactNode }) {
  return (
    <span style={{
      fontSize: 11, padding: '2px 8px', borderRadius: 999,
      border: '1px solid var(--border-soft)', color: 'var(--muted)',
      whiteSpace: 'nowrap', lineHeight: 1.6,
    }}>{children}</span>
  )
}

/** 顶部摘要卡：型号 / 系统 / 运行时间 */
function SummaryCard({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="panel" style={{ padding: 'var(--sp-3) var(--sp-4)', minWidth: 200 }}>
      <div style={{ fontSize: 11, color: 'var(--accent)', fontWeight: 600, marginBottom: 4 }}>{label}</div>
      <div style={{ fontSize: 14, fontWeight: 600, letterSpacing: '-0.01em', wordBreak: 'break-word' }}>{value || '—'}</div>
      {sub && <div style={{ fontSize: 11, color: 'var(--meta)', marginTop: 3 }}>{sub}</div>}
    </div>
  )
}

/**
 * 部件卡片：主值大字号 + 参数胶囊，形成主次分明的层次
 * （这是与「单列长表」的核心区别：四大件的视觉权重高于外设）
 */
function SpecCard({ icon, label, primary, secondary, chips, children, minH }: {
  icon: React.ReactNode
  label: string
  primary: string
  secondary?: string
  chips?: string[]
  children?: React.ReactNode
  minH?: number
}) {
  const shown = (chips ?? []).filter(Boolean)
  return (
    <div className="panel" style={{ padding: 'var(--sp-4) var(--sp-5)', minHeight: minH, display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, color: 'var(--muted)', fontSize: 12 }}>
        <span style={{ color: 'var(--accent)', display: 'grid', placeItems: 'center' }}>{icon}</span>
        <span>{label}</span>
      </div>
      <div style={{ fontSize: 15, fontWeight: 600, letterSpacing: '-0.01em', lineHeight: 1.45, wordBreak: 'break-word' }}>
        {primary || '—'}
      </div>
      {secondary && <div style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.7, wordBreak: 'break-word' }}>{secondary}</div>}
      {shown.length > 0 && (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 2 }}>
          {shown.map((c) => <Chip key={c}>{c}</Chip>)}
        </div>
      )}
      {children}
    </div>
  )
}

/** 识别方法说明内容（标题行 Info 浮层内） */
function MethodsContent() {
  return (
    <div style={{ fontSize: 12.5, color: 'var(--muted)', lineHeight: 1.85 }}>
      <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--fg)', marginBottom: 8 }}>这些硬件信息是怎么识别出来的？</div>
      <div>· <b>处理器</b>：注册表 <span className="mono">HARDWARE\DESCRIPTION\System\CentralProcessor\0</span>（型号/基准频率）+ WMI <span className="mono">Win32_Processor</span>（物理核/逻辑核）</div>
      <div>· <b>内存</b>：Win32 API <span className="mono">GlobalMemoryStatusEx</span>（总量/可用）+ WMI <span className="mono">Win32_PhysicalMemory</span>（容量/标称与实际频率/电压/插槽/型号）</div>
      <div>· <b>显卡</b>：WMI <span className="mono">Win32_VideoController</span>（型号/驱动/厂商）+ 注册表 <span className="mono">Control\Class\（显卡设备类）\HardwareInformation.qwMemorySize</span>（真实显存，避开 32 位 API 的 4GB 上限）</div>
      <div>· <b>显示器</b>：ACPI 接口 <span className="mono">root\wmi WmiMonitorID</span>（型号/厂商编码）+ <span className="mono">WmiMonitorBasicDisplayParams</span>（物理尺寸换算英寸）</div>
      <div>· <b>硬盘</b>：WMI <span className="mono">Get-PhysicalDisk</span>（型号/介质 SSD-HDD/总线 NVMe-SATA）+ <span className="mono">Get-StorageReliabilityCounter</span>（SMART 磨损/温度/通电时长）</div>
      <div>· <b>主板/BIOS</b>：WMI <span className="mono">Win32_BaseBoard</span> / <span className="mono">Win32_BIOS</span> · <b>系统</b>：<span className="mono">Win32_OperatingSystem</span>（版本/Build/架构/开机时间）</div>
      <div>· <b>声卡/网卡</b>：WMI <span className="mono">Win32_SoundDevice</span> / <span className="mono">Win32_NetworkAdapter</span>（仅物理适配器）</div>
      <div style={{ marginTop: 8, color: 'var(--warn)' }}>
        说明：以上全部来自系统实时查询。若注册表被手动修改、驱动/厂商实现不标准，或未以管理员权限运行，
        个别字段可能识别不到或显示不全（此时显示为「—」而不会编造数值）。
      </div>
    </div>
  )
}

export function Hardware() {
  // 初始状态直接取缓存：切回本页时瞬时渲染，不再出现加载态闪烁
  const [info, setInfo] = useState<HardwareInfo | null>(() => peekHardwareInfo())
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [cacheAt, setCacheAt] = useState(() => hardwareCacheInfo().at)
  const [showMethods, setShowMethods] = useState(false)
  // 运行时间实时递增：以扫描时刻的 uptime 为基准 + 本地经过时间（不重新扫描）
  const [, forceTick] = useState(0)

  const load = (force = false) => {
    setRefreshing(true)
    getHardwareInfo(force)
      .then((d) => {
        setInfo(d)
        setCacheAt(hardwareCacheInfo().at)
      })
      .catch((e) => setError(String(e)))
      .finally(() => setRefreshing(false))
  }

  // 挂载时：命中缓存（30 分钟内）则零请求；否则自动重扫一次
  useEffect(() => { load(false) }, [])

  // 每秒刷新一次运行时间显示
  useEffect(() => {
    const t = setInterval(() => forceTick((n) => n + 1), 1000)
    return () => clearInterval(t)
  }, [])

  if (error) {
    return (
      <div className="page-enter" style={{ padding: 32 }}>
        <div style={{ color: 'var(--danger)', marginBottom: 12 }}>读取硬件信息失败：{error}</div>
        <button className="btn btn-ghost" onClick={() => { setError(null); load(true) }}><RefreshCw size={14} /> 重试</button>
      </div>
    )
  }
  if (!info) {
    return <PageLoading label="正在读取本机硬件信息" hint="首次扫描约 2–6 秒（WMI 查询）；之后切换页面直接读缓存" />
  }

  const memPct = info.memory.usedPct || 0
  const uptimeNow = info.uptimeSecs + (cacheAt > 0 ? (Date.now() - cacheAt) / 1000 : 0)
  const memSticks = info.memory.sticks
  const memPrimary = [
    fmtBytes(info.memory.totalBytes, 0),
    info.memory.memType,
    info.memory.speedMhz ? `${info.memory.speedMhz}MHz` : '',
  ].filter(Boolean).join(' ')
  const cpuChips = [
    info.cpu.coresPhysical > 0 ? `${info.cpu.coresPhysical} 物理核` : '',
    info.cpu.coresLogical > 0 ? `${info.cpu.coresLogical} 线程` : '',
    info.cpu.baseMhz ? `${(info.cpu.baseMhz / 1000).toFixed(2)} GHz` : '',
  ]
  const gpuChips = info.gpu.flatMap((g) => [
    g.vramBytes > 0 ? `显存 ${fmtBytes(g.vramBytes, 0)}` : '',
    g.vendor || '',
    g.driverVersion ? `驱动 ${g.driverVersion}` : '',
  ])

  return (
    <div className="page-enter">
      {/* 标题行：机器名 + 读取时间 + 识别方法 / 刷新 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 10, position: 'relative' }}>
        <div className="section-label" style={{ marginBottom: 0 }}>
          {info.computerName || '本机'} · 硬件识别结果
        </div>
        <span className="sum-spacer" />
        {cacheAt > 0 && (
          <span className="status-line" style={{ fontSize: 11.5, whiteSpace: 'nowrap' }}>
            <Clock size={12} />
            读取于 {new Date(cacheAt).toLocaleTimeString('zh-CN', { hour12: false })} · 缓存 30 分钟
          </span>
        )}
        <button
          className="icon-btn"
          title="这些硬件信息是怎么识别出来的？"
          aria-label="识别方法说明"
          aria-expanded={showMethods}
          onClick={() => setShowMethods((s) => !s)}
        >
          <Info size={16} color={showMethods ? 'var(--accent)' : undefined} />
        </button>
        <button className="icon-btn" title="强制重新扫描" onClick={() => load(true)} disabled={refreshing}>
          <RefreshCw size={15} className={refreshing ? 'spin' : ''} />
        </button>

        {showMethods && (
          <>
            {/* 点击浮层外部关闭 */}
            <div style={{ position: 'fixed', inset: 0, zIndex: 110 }} onClick={() => setShowMethods(false)} />
            <div
              role="dialog"
              style={{
                position: 'absolute', top: 40, right: 0, zIndex: 120,
                width: 'min(560px, 92vw)', maxHeight: '62vh', overflowY: 'auto',
                background: 'var(--surface)', border: '1px solid var(--border)',
                borderRadius: 'var(--radius-lg)', boxShadow: '0 12px 40px rgba(0,0,0,.20)',
                padding: 'var(--sp-4) var(--sp-5)',
              }}
            >
              <MethodsContent />
            </div>
          </>
        )}
      </div>

      {/* 顶部摘要：型号 / 系统 / 运行时间 */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(230px, 1fr))', gap: 12, marginBottom: 20 }}>
        <SummaryCard label="型号信息" value={`${info.motherboard.product || ''} ${info.motherboard.systemModel || ''}`.trim()} />
        <SummaryCard
          label="系统信息"
          value={`${(info.system.caption || '').replace(/Microsoft\s*/i, '')}${info.system.arch ? ` ${info.system.arch}` : ''}`.trim()}
          sub={info.system.version ? `版本 ${info.system.version}（Build ${info.system.build}）` : undefined}
        />
        <SummaryCard label="运行时间" value={fmtUptime(uptimeNow)} sub="本次开机以来 · 每秒刷新" />
      </div>

      {/* 核心部件：一行四张，主值大字 + 参数胶囊 */}
      <div className="section-label">核心部件</div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(228px, 1fr))', gap: 14, marginBottom: 20 }}>
        <SpecCard icon={<Cpu size={15} />} label="处理器" primary={info.cpu.name || '—'} chips={cpuChips} minH={124} />
        <SpecCard
          icon={<MemoryStick size={15} />}
          label="内存"
          primary={memPrimary || '—'}
          chips={memSticks.map((s) => `${s.slot || 'DIMM'} ${fmtBytes(s.capacityBytes, 0)}${s.configuredMhz ? ` @${s.configuredMhz}MHz` : ''}`)}
          minH={124}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 'auto', paddingTop: 6 }}>
            <div className="progress-track" style={{ height: 6, flex: 1 }}>
              <div className="progress-fill" style={{ width: `${memPct}%`, background: 'var(--cap-used)' }} />
            </div>
            <span className="mono" style={{ fontSize: 11.5, color: 'var(--muted)', flexShrink: 0 }}>已用 {memPct}%</span>
          </div>
        </SpecCard>
        <SpecCard
          icon={<Monitor size={15} />}
          label="显卡"
          primary={info.gpu.length > 0 ? info.gpu.map((g) => g.name).join(' / ') : '未检测到'}
          chips={gpuChips}
          minH={124}
        />
        <SpecCard
          icon={<CircuitBoard size={15} />}
          label="主板"
          primary={`${info.motherboard.manufacturer || ''} ${info.motherboard.product || ''}`.trim() || '—'}
          secondary={info.motherboard.biosVersion ? `BIOS ${info.motherboard.biosVersion}` : undefined}
          chips={info.motherboard.systemManufacturer ? [info.motherboard.systemManufacturer] : undefined}
          minH={124}
        />
      </div>

      {/* 存储与外设：信息密度更高，作为次要层级 */}
      <div className="section-label">存储与外设</div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))', gap: 14 }}>
        <SpecCard
          icon={<HardDrive size={15} />}
          label="磁盘"
          primary={info.disks.length > 0 ? `${info.disks.length} 块` : '—'}
          chips={info.disks.map((d) => `${d.media || '未知'}${d.bus ? `/${d.bus}` : ''}`)}
        >
          {info.disks.slice(0, 3).map((d, i) => (
            <div key={i} style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.7 }}>
              {d.model} · {fmtBytes(d.sizeBytes, 0)}
            </div>
          ))}
        </SpecCard>
        <SpecCard
          icon={<MonitorCog size={15} />}
          label="显示器"
          primary={info.monitors.length > 0 ? info.monitors.map((m) => m.name).join(' / ') : '未检测到'}
          secondary={info.monitors.filter((m) => m.sizeInch > 0).map((m) => `${m.sizeInch} 英寸`).join(' / ') || undefined}
        />
        <SpecCard icon={<Volume2 size={15} />} label="声卡" primary={info.sound.length > 0 ? `${info.sound.length} 个设备` : '—'}>
          {info.sound.slice(0, 3).map((s, i) => (
            <div key={i} style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.7 }}>{s}</div>
          ))}
        </SpecCard>
        <SpecCard icon={<Wifi size={15} />} label="网卡" primary={info.network.length > 0 ? `${info.network.length} 个适配器` : '—'}>
          {info.network.slice(0, 3).map((n, i) => (
            <div key={i} style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.7 }}>
              {n.name}{n.kind ? `（${n.kind}）` : ''}
            </div>
          ))}
        </SpecCard>
      </div>
    </div>
  )
}
