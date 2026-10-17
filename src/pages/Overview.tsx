import { useEffect, useState } from 'react'
import { HardDrive, AlertTriangle, X, Activity, Thermometer, Clock, ShieldCheck, ChevronUp, ChevronDown, RefreshCw } from 'lucide-react'
import type { Volume, PhysicalDisk, SmartAttribute } from '../types'
import { fmtBytes, fmtPct, capVar } from '../utils/format'
import { getPhysicalDisks, peekPhysicalDisks, disksFetchedAt } from '../services/api'
import { RingGauge, RingCenter } from '../components/RingGauge'
import { ScanTaskBanner } from '../components/ScanTask'
import { useScanTasks } from '../services/scanTasks'

/** 时间戳 → HH:MM（用于「数据读取于 …」） */
function hhmm(ts: number | null): string {
  if (!ts) return '—'
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <div style={{ fontSize: 11, color: 'var(--meta)', marginBottom: 4 }}>{label}</div>
      <div className="mono" style={{ fontSize: 20, fontWeight: 600, letterSpacing: '-0.02em', color: tone || 'var(--fg)' }}>
        {value}
      </div>
    </div>
  )
}

function healthTag(d: PhysicalDisk) {
  const h = (d.health || '').toLowerCase()
  if (h.includes('healthy')) return <span className="tag tag-safe">健康</span>
  if (h.includes('warning')) return <span className="tag tag-warn">警告</span>
  if (h.includes('unhealthy')) return <span className="tag tag-danger">异常</span>
  return <span className="tag tag-hdd">状态未知</span>
}

/** 温度语义色：<45 常规，45–54 偏暖，>=55 偏热 */
function tempColor(t?: number): string {
  if (typeof t !== 'number' || t <= 0) return 'var(--fg)'
  if (t >= 55) return 'var(--temp-hot)'
  if (t >= 45) return 'var(--temp-warm)'
  return 'var(--temp-cold)'
}

/** SMART 属性状态判定（与 CrystalDiskInfo 一致）：当前值<=阈值=异常；最差值<=阈值=注意；否则正常 */
function attrStatus(a: SmartAttribute): 'good' | 'caution' | 'bad' {
  if (a.threshold > 0 && a.current <= a.threshold) return 'bad'
  if (a.threshold > 0 && a.worst <= a.threshold) return 'caution'
  return 'good'
}

const STATUS_META: Record<'good' | 'caution' | 'bad', { color: string; label: string }> = {
  good: { color: 'var(--state-good)', label: '正常' },
  caution: { color: 'var(--state-caution)', label: '注意' },
  bad: { color: 'var(--state-bad)', label: '异常' },
}

/** 硬盘寿命：有磨损计数才显示百分比；读不到就如实显示"未知"，绝不用 100% 充数 */
function lifeInfo(d: PhysicalDisk) {
  if (typeof d.wear === 'number') {
    const remain = Math.max(0, Math.min(100, 100 - d.wear))
    return { label: '剩余寿命', pct: remain, known: true, desc: `已磨损 ${d.wear}%（数据来源：${d.source || 'SMART'}）` }
  }
  const h = (d.health || '').toLowerCase()
  if (h.includes('unhealthy')) return { label: '健康状态', pct: 20, known: true, desc: 'SMART 报告异常，请立即备份并考虑更换' }
  if (h.includes('warning')) return { label: '健康状态', pct: 55, known: true, desc: 'SMART 报告警告，建议尽快备份数据' }
  return {
    label: '寿命',
    pct: 0,
    known: false,
    desc: '未能读取磨损/寿命计数（该硬盘不支持该接口，或未以管理员权限运行）',
  }
}

/** 名称过长时截断，用于多盘标签 */
function shortModel(s: string): string {
  return s.length > 22 ? s.slice(0, 21) + '…' : s
}

/** 详细信息弹窗：顶部多盘标签切换 + CDI 风格主视觉 + 指标网格 + SMART 属性表 */
function DiskDetail({ disks, index, onSelect, onClose }: {
  disks: PhysicalDisk[]
  index: number
  onSelect: (i: number) => void
  onClose: () => void
}) {
  const [showAttrs, setShowAttrs] = useState(true)
  const disk = disks[index]
  if (!disk) return null

  const life = lifeInfo(disk)
  const unknown = '未读取到'
  const num = (v: number | undefined, fmt?: (n: number) => string) =>
    typeof v === 'number' && v > 0 ? (fmt ? fmt(v) : String(v)) : unknown

  // CrystalDiskInfo 风格的指标项
  const rows: [string, string][] = [
    ['容量', disk.sizeBytes > 0 ? fmtBytes(disk.sizeBytes, 1) : '—'],
    ['接口', [disk.media, disk.bus].filter((x) => x && x !== '未知').join(' · ') || '—'],
    ['传输模式', disk.transferMode || '—'],
    ['盘符', (disk.volumes ?? []).join(' ') || '—'],
    ['分区表', disk.partitionStyle || '—'],
    ['分区数', typeof disk.partitionCount === 'number' && disk.partitionCount > 0 ? `${disk.partitionCount} 个` : unknown],
    ['固件版本', disk.firmware || unknown],
    ['序列号', disk.serial || unknown],
    ['SMART 健康', disk.health || '未知'],
    ['预测故障', disk.predictFailure === true ? '是（建议立即备份）' : disk.predictFailure === false ? '否' : '未知'],
    ['磨损 / 已用寿命', typeof disk.wear === 'number' ? `${disk.wear}%（剩余 ${Math.max(0, 100 - disk.wear)}%）` : unknown],
    ['通电时长', typeof disk.powerOnHours === 'number' && disk.powerOnHours > 0
      ? `${disk.powerOnHours} 小时（约 ${(disk.powerOnHours / 24 / 365).toFixed(1)} 年）` : unknown],
    ['通电次数', num(disk.powerCycles, (n) => `${n} 次`)],
    ['温度', typeof disk.temperature === 'number' && disk.temperature > 0
      ? `${disk.temperature} °C${typeof disk.criticalTemperature === 'number' ? `（临界 ${disk.criticalTemperature} °C）` : ''}`
      : unknown],
    ['转速', typeof disk.rotationRate === 'number'
      ? (disk.rotationRate === 0 ? '固态（无转速）' : `${disk.rotationRate} RPM`) : '—'],
    ['累计写入量', num(disk.totalWrittenBytes, (n) => fmtBytes(n, 1))],
    ['累计读取量', num(disk.totalReadBytes, (n) => fmtBytes(n, 1))],
    ['可用备用空间', typeof disk.availableSpare === 'number' ? `${disk.availableSpare}%（阈值以下需更换）` : '—（仅 NVMe 提供）'],
    ['坏道计数', num(disk.readErrors, (n) => `${n} 个（重分配+待映射）`)],
    ['写入错误', num(disk.writeErrors)],
    ['主机读命令数', num(disk.hostReadCommands, (n) => n.toLocaleString())],
    ['主机写命令数', num(disk.hostWriteCommands, (n) => n.toLocaleString())],
    ['数据来源', disk.source || '未读取到'],
  ]

  const attrs = disk.attributes ?? []
  const health = disk.healthItems ?? []
  const hasTemp = typeof disk.temperature === 'number' && disk.temperature > 0

  /** NVMe 健康项的等级配色：0 正常 / 1 注意 / 2 异常 / 3 信息 */
  const HEALTH_TONE: Record<number, { color: string; label: string }> = {
    0: { color: 'var(--state-good)', label: '正常' },
    1: { color: 'var(--state-caution)', label: '注意' },
    2: { color: 'var(--state-bad)', label: '异常' },
    3: { color: 'var(--meta)', label: '信息' },
  }

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal-panel" onClick={(e) => e.stopPropagation()} style={{ width: 'min(680px, 100%)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
          <HardDrive size={18} color="var(--accent)" />
          <span style={{ fontSize: 15, fontWeight: 600, flex: 1, minWidth: 0, wordBreak: 'break-word' }}>{disk.model}</span>
          {healthTag(disk)}
          <button className="icon-btn" onClick={onClose} aria-label="关闭"><X size={16} /></button>
        </div>

        {/* 顶部：多盘标签切换（CrystalDiskInfo 顶部磁盘标签） */}
        {disks.length > 1 && (
          <div style={{ display: 'flex', gap: 6, marginBottom: 14, flexWrap: 'wrap' }}>
            {disks.map((d, i) => (
              <button
                key={i}
                onClick={() => onSelect(i)}
                title={d.model}
                style={{
                  display: 'inline-flex', alignItems: 'center', gap: 6,
                  padding: '4px 12px', fontSize: 12.5, borderRadius: 'var(--radius-md)',
                  border: `1px solid ${i === index ? 'var(--accent)' : 'var(--border-soft)'}`,
                  background: i === index ? 'var(--accent-soft)' : 'var(--surface)',
                  color: i === index ? 'var(--accent)' : 'var(--muted)',
                  fontWeight: i === index ? 600 : 400,
                }}
              >
                <span style={{ width: 7, height: 7, borderRadius: '50%', flexShrink: 0, background: i === index ? 'var(--accent)' : 'var(--border)' }} />
                {shortModel(d.model || `磁盘 ${i + 1}`)}
              </button>
            ))}
          </div>
        )}

        {/* 主视觉：寿命环 + 健康/寿命说明 + 温度大字（CDI 风格关键指标） */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 20, marginBottom: 16, flexWrap: 'wrap' }}>
          <div className="ring-wrap">
            <RingGauge pct={life.pct} color={life.pct > 60 ? 'var(--cap-free)' : life.pct > 30 ? 'var(--cap-warn)' : 'var(--cap-danger)'} size={72} />
            <RingCenter pct={life.pct} free={0} sub="剩余" />
          </div>
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <span style={{ fontSize: 13.5, fontWeight: 600 }}>{life.label}：{life.pct}%</span>
              {healthTag(disk)}
            </div>
            <div style={{ fontSize: 12, color: 'var(--muted)', marginTop: 3, maxWidth: 340, lineHeight: 1.6 }}>{life.desc}</div>
          </div>
          <div style={{ marginLeft: 'auto', textAlign: 'right' }}>
            <div className="mono" style={{ fontSize: 30, fontWeight: 600, letterSpacing: '-0.02em', color: tempColor(disk.temperature) }}>
              {hasTemp ? `${disk.temperature}°C` : '—'}
            </div>
            <div style={{ fontSize: 11, color: 'var(--meta)', display: 'inline-flex', alignItems: 'center', gap: 4 }}>
              <Thermometer size={11} /> 当前温度
            </div>
          </div>
        </div>

        {/* 指标网格：两列，CDI 信息密度 */}
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '0 20px', maxHeight: 300, overflowY: 'auto', paddingRight: 4 }}>
          {rows.map(([k, v]) => (
            <div key={k} style={{ display: 'flex', gap: 10, padding: '6px 0', borderBottom: '1px solid var(--border-soft)' }}>
              <span style={{ fontSize: 12, color: 'var(--muted)', width: 92, flexShrink: 0 }}>{k}</span>
              <span style={{ fontSize: 12, wordBreak: 'break-word', flex: 1 }}>{v}</span>
            </div>
          ))}
        </div>

        {/* SMART 属性表（CrystalDiskInfo 的核心视图） */}
        {attrs.length > 0 && (
          <div style={{ marginTop: 14 }}>
            <button className="btn btn-ghost" style={{ padding: '4px 10px', fontSize: 12.5 }} onClick={() => setShowAttrs((s) => !s)} aria-expanded={showAttrs}>
              {showAttrs ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
              SMART 属性表（{attrs.length} 项）
            </button>
            {showAttrs && (
              <div style={{ marginTop: 10, maxHeight: 280, overflowY: 'auto', border: '1px solid var(--border-soft)', borderRadius: 8 }}>
                <div style={{
                  display: 'grid', gridTemplateColumns: '36px 1fr 44px 44px 44px 1fr 64px', gap: 0,
                  fontSize: 11.5, color: 'var(--meta)', padding: '6px 10px',
                  borderBottom: '1px solid var(--border-soft)', position: 'sticky', top: 0, background: 'var(--surface)', zIndex: 1,
                }}>
                  <span>ID</span><span>属性名</span><span>当前</span><span>最差</span><span>阈值</span><span>原始值</span><span>状态</span>
                </div>
                {attrs.map((a) => {
                  const st = attrStatus(a)
                  const meta = STATUS_META[st]
                  return (
                    <div key={a.id} className="mono" style={{
                      display: 'grid', gridTemplateColumns: '36px 1fr 44px 44px 44px 1fr 64px', gap: 0,
                      fontSize: 11.5, padding: '5px 10px', borderBottom: '1px solid var(--border-soft)',
                    }}>
                      <span>{a.id}</span>
                      <span style={{ fontFamily: 'var(--font-body)' }}>{a.name}</span>
                      <span>{a.current}</span>
                      <span>{a.worst}</span>
                      <span>{a.threshold > 0 ? a.threshold : '—'}</span>
                      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{a.raw}</span>
                      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 5, fontFamily: 'var(--font-body)' }}>
                        <span style={{ width: 7, height: 7, borderRadius: '50%', background: meta.color, flexShrink: 0 }} />
                        <span style={{ color: meta.color }}>{meta.label}</span>
                      </span>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        )}

        {/* NVMe 健康状态（NVMe 无 ATA 式属性表，改用 CDI 的健康列表视图） */}
        {health.length > 0 && (
          <div style={{ marginTop: 14 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 8, display: 'flex', alignItems: 'center', gap: 6 }}>
              <Activity size={13} color="var(--accent)" /> NVMe 健康状态（{health.length} 项）
            </div>
            <div style={{ border: '1px solid var(--border-soft)', borderRadius: 8, maxHeight: 280, overflowY: 'auto' }}>
              {health.map((h) => {
                const meta = HEALTH_TONE[h.level] ?? HEALTH_TONE[3]
                return (
                  <div key={h.label} style={{ display: 'flex', gap: 10, alignItems: 'center', padding: '5px 10px', fontSize: 11.5, borderBottom: '1px solid var(--border-soft)' }}>
                    <span style={{ width: 8, height: 8, borderRadius: '50%', background: meta.color, flexShrink: 0 }} />
                    <span style={{ fontFamily: 'var(--font-body)', color: 'var(--muted)' }}>{h.label}</span>
                    <span style={{ flex: 1, textAlign: 'right', color: meta.color, fontWeight: 600 }}>{h.value}</span>
                  </div>
                )
              })}
            </div>
          </div>
        )}

        <div style={{ fontSize: 11, color: 'var(--meta)', marginTop: 10, lineHeight: 1.7 }}>
          数据通过 <b>SMART 直读</b>（DeviceIoControl，与 CrystalDiskInfo 同原理）获取，WMI 兜底；
          若显示「未读取到」，说明该盘未返回对应数据（部分 USB/RAID 桥接设备会拦截 SMART 请求）。
          {attrs.length > 0 && disk.hasThresholds === false && (
            <div style={{ color: 'var(--warn)', marginTop: 4 }}>
              注：该盘未返回 SMART 阈值表（0xD1），「阈值」列显示为 —，状态列仅按当前值/最差值推断。
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

export function Overview({ volumes, onOpenDrive }: {
  volumes: Volume[]
  onOpenDrive: (v: Volume) => void
}) {
  const [disks, setDisks] = useState<PhysicalDisk[]>(() => peekPhysicalDisks() ?? [])
  const [picked, setPicked] = useState<number | null>(null)
  // 物理硬盘 + SMART 在本次运行内只扫一次（缓存），这里记录取数时刻用于界面提示
  const [fetchedAt, setFetchedAt] = useState<number | null>(() => disksFetchedAt())
  const [loading, setLoading] = useState(() => (peekPhysicalDisks() ?? []).length === 0)
  const [refreshing, setRefreshing] = useState(false)
  const scanTasks = useScanTasks()
  const scanning = (letter: string) =>
    scanTasks.find((t) => t.letter === letter.toUpperCase() && (t.status === 'scanning' || t.status === 'cancelling'))

  /**
   * 读取物理硬盘数据。
   * - `force=false`：命中会话缓存直接返回，不触碰磁盘（切页回来不重复扫描）
   * - `force=true`：用户点「重新读取」时才真正重扫
   */
  const load = async (force: boolean) => {
    if (force) setRefreshing(true)
    try {
      const d = await getPhysicalDisks(force)
      setDisks(d)
      setFetchedAt(disksFetchedAt())
    } catch {
      if (disks.length === 0) setDisks([])
    } finally {
      setLoading(false)
      setRefreshing(false)
    }
  }

  useEffect(() => {
    // 已有缓存（如启动预热或上次进入本页）→ 直接复用，不再扫盘
    if (peekPhysicalDisks()) {
      setDisks(peekPhysicalDisks() as PhysicalDisk[])
      setFetchedAt(disksFetchedAt())
      setLoading(false)
      return
    }
    load(false)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  if (volumes.length === 0) {
    return (
      <div className="page-enter status-line" style={{ padding: 48, justifyContent: 'center' }}>
        正在读取磁盘信息…
      </div>
    )
  }

  const total = volumes.reduce((s, v) => s + v.totalBytes, 0)
  const used = volumes.reduce((s, v) => s + v.usedBytes, 0)
  const free = volumes.reduce((s, v) => s + v.freeBytes, 0)
  const tight = volumes.filter((v) => fmtPct(v.usedBytes, v.totalBytes) > 85)

  return (
    <div className="page-enter">
      {/* 后台空间分析任务：可见、可停止、可稍后查看结果 */}
      <ScanTaskBanner volumes={volumes} onOpen={onOpenDrive} />

      {/* 汇总：磁盘数量按物理硬盘计数（一块盘可分多个盘符） */}
      <div className="panel" style={{ padding: 'var(--sp-5)', marginBottom: 18 }}>
        <div style={{ display: 'flex', gap: 40, flexWrap: 'wrap', alignItems: 'flex-end' }}>
          <Stat label="物理硬盘" value={disks.length > 0 ? `${disks.length} 块` : '—'} />
          <Stat label="分区盘符" value={`${volumes.length} 个`} />
          <Stat label="总容量" value={fmtBytes(total, 1)} />
          <Stat label="已用" value={fmtBytes(used, 1)} tone="var(--cap-used)" />
          <Stat label="可用" value={fmtBytes(free, 1)} tone="var(--cap-free)" />
          {tight.length > 0 && (
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginLeft: 'auto', color: 'var(--warn)', fontSize: 12.5 }}>
              <AlertTriangle size={14} />
              {tight.map((v) => `${v.letter}:`).join(' ')} 空间偏低
            </div>
          )}
        </div>
      </div>

      {/* 盘符卡片 */}
      <div className="section-label">分区盘符（点击查看空间分布）</div>
      <div className="drive-grid">
        {volumes.map((v) => {
          const pct = fmtPct(v.usedBytes, v.totalBytes)
          const color = capVar(pct)
          const mediaOk = v.media === 'SSD' || v.media === 'HDD'
          return (
            <button key={v.letter} className="drive-card" onClick={() => onOpenDrive(v)}>
              <div className="ring-wrap">
                <RingGauge pct={pct} color={color} />
                <RingCenter pct={pct} free={v.freeBytes} />
              </div>
              <div className="drive-meta">
                <div className="drive-name">
                  <span className="drive-letter">{v.letter}: </span>
                  <span className="drive-label">{v.label || '本地磁盘'}</span>
                  {mediaOk && <span className={`tag ${v.media === 'SSD' ? 'tag-ssd' : 'tag-hdd'}`}>{v.media}</span>}
                  {scanning(v.letter) && <span className="tag tag-ssd">扫描中</span>}
                </div>
                <div className="drive-vol">{v.fs} · 总容量 {fmtBytes(v.totalBytes, 0)}</div>
                <div className="drive-nums">
                  <div>
                    <div className="dnum-label">已用</div>
                    <div className="dnum-val" style={{ color }}>{fmtBytes(v.usedBytes, 1)}</div>
                  </div>
                  <div>
                    <div className="dnum-label">可用</div>
                    <div className="dnum-val">{fmtBytes(v.freeBytes, 1)}</div>
                  </div>
                </div>
              </div>
            </button>
          )
        })}
      </div>

      {/* 物理硬盘卡片（点击查看详情与寿命） */}
      <div className="section-label" style={{ marginTop: 24, display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
        <span>物理硬盘（点击查看详情与寿命）</span>
        <span style={{ fontSize: 11.5, color: 'var(--meta)', fontWeight: 400 }}>
          数据读取于 {hhmm(fetchedAt)} · 本次运行只扫描一次
        </span>
        <span style={{ flex: 1 }} />
        <button
          className="btn btn-ghost"
          style={{ padding: '3px 10px', fontSize: 12 }}
          onClick={() => load(true)}
          disabled={refreshing}
          title="重新读取全部物理硬盘的 SMART 数据（会重新扫描磁盘，约数秒）"
        >
          <RefreshCw size={12} className={refreshing ? 'spin' : undefined} />
          {refreshing ? ' 正在重新读取…' : ' 重新读取'}
        </button>
      </div>
      <div className="drive-grid">
        {disks.map((d, i) => {
          const life = lifeInfo(d)
          const isSsd = (d.media || '').toUpperCase().includes('SSD')
          return (
            <button key={i} className="drive-card" onClick={() => setPicked(i)} style={{ alignItems: 'flex-start' }}>
              <div className="quick-icon" style={{ width: 40, height: 40, background: 'var(--accent-soft)', color: 'var(--accent)' }}>
                <HardDrive size={19} />
              </div>
              <div className="drive-meta">
                <div className="drive-name">
                  <span style={{ fontSize: 13.5, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{d.model}</span>
                  {healthTag(d)}
                </div>
                <div className="drive-vol">
                  {[d.media, d.bus].filter((x) => x && x !== '未知').join(' · ') || '介质未知'} · {fmtBytes(d.sizeBytes, 1)}
                  {(d.volumes ?? []).length > 0 && ` · ${(d.volumes ?? []).join(' ')}`}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 6 }}>
                  <div className="progress-track" style={{ height: 6, flex: 1 }}>
                    <div className="progress-fill" style={{
                      width: life.known ? `${life.pct}%` : '100%',
                      background: !life.known
                        ? 'var(--border)'
                        : life.pct > 60 ? 'var(--cap-free)' : life.pct > 30 ? 'var(--cap-warn)' : 'var(--cap-danger)',
                    }} />
                  </div>
                  <span className="mono" style={{ fontSize: 11, color: life.known ? 'var(--muted)' : 'var(--meta)', flexShrink: 0 }}>
                    {life.known
                      ? (typeof d.wear === 'number' ? `寿命 ${life.pct}%` : `SMART ${life.pct === 100 ? '正常' : '异常'}`)
                      : '寿命/健康未读取'}
                  </span>
                </div>
                <div style={{ display: 'flex', gap: 14, marginTop: 6, fontSize: 11, color: 'var(--meta)' }}>
                  {typeof d.temperature === 'number' && <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}><Thermometer size={11} />{d.temperature} °C</span>}
                  {typeof d.powerOnHours === 'number' && <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}><Clock size={11} />{Math.round(d.powerOnHours / 24)} 天通电</span>}
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}><ShieldCheck size={11} />累计读错 {d.readErrors ?? 0}</span>
                </div>
              </div>
            </button>
          )
        })}
        {disks.length === 0 && (
          <div className="panel" style={{ padding: 'var(--sp-5)', color: 'var(--meta)', display: 'flex', alignItems: 'center', gap: 8 }}>
            <Activity size={14} />
            {loading ? '正在读取物理硬盘信息（含 SMART 寿命）…' : '未读取到物理硬盘信息，可点右上角「重新读取」重试'}
          </div>
        )}
      </div>

      {picked !== null && (
        <DiskDetail disks={disks} index={picked} onSelect={setPicked} onClose={() => setPicked(null)} />
      )}
    </div>
  )
}
