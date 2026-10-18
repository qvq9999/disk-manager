import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Trash2, Lock, Check, Loader2, FileClock, Database, ScrollText,
  Code, FlaskConical, HardDrive, ChevronDown, ChevronRight, ScanSearch,
  Globe, MessageSquare, PlayCircle, Gamepad2, FileText, Download, PackageX,
  RefreshCw, TrendingDown, AlertTriangle, Layers, FolderMinus,
} from 'lucide-react'
import type { CleanCategory, Volume } from '../types'
import {
  getCleanCategories, runClean, invalidateAfterClean, getVolumes, peekVolumes,
  peekCleanCategories, getLastCleanDrives, cleanFetchedAt,
} from '../services/api'
import { fmtBytes } from '../utils/format'
import { InstallerClean } from './cleanup/InstallerClean'
import { EmptyDirClean } from './cleanup/EmptyDirClean'
import { EmptyArtIcon } from './cleanup/EmptyArtIcon'

/**
 * 页面状态机
 *
 * idle      → 从未扫描过（本次运行内无缓存）：显示盘符选择 + 「开始扫描」
 * scanning  → 正在扫描（用户点「开始扫描」或「重新扫描」）
 * ready     → 有扫描结果（可能来自缓存，也可能刚扫完）
 * cleaning  → 正在删除
 * done      → 删除完成，并已**重新扫描**，列表与数字均为清理后的真实值
 */
type Phase = 'idle' | 'scanning' | 'ready' | 'cleaning' | 'done'

const CAT_ICON: Record<string, typeof FileClock> = {
  FileClock, Database, ScrollText, Code, FlaskConical, Trash2, HardDrive,
  Globe, MessageSquare, PlayCircle, Gamepad2, FileText, Download, PackageX,
  Layers,
}

/** 空态插画（轻量内联 SVG，跟随主题色） */
function hhmm(ts: number | null): string {
  if (!ts) return '—'
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** 垃圾扫描（原清理页主体）。外层 `Cleanup` 负责三个子功能的页签切换。 */
function JunkScan() {
  // ---- 首次挂载：从会话缓存恢复，避免切页回来又变成空态、也不重复扫描 ----
  const boot = useMemo(() => {
    const scope = getLastCleanDrives()
    const cached = scope ? peekCleanCategories(scope) : null
    return { scope, cached, at: scope ? cleanFetchedAt(scope) : null }
  }, [])

  const [phase, setPhase] = useState<Phase>(boot.cached ? 'ready' : 'idle')
  const [drives, setDrives] = useState<Volume[]>(() => peekVolumes() ?? [])
  const [selectedDrives, setSelectedDrives] = useState<Set<string>>(
    () => new Set(boot.scope ?? (peekVolumes() ?? []).map((v) => v.letter)),
  )
  const [scanPct, setScanPct] = useState(0)
  const [scanMsg, setScanMsg] = useState('')
  const [cats, setCats] = useState<CleanCategory[]>(boot.cached ?? [])
  const [sel, setSel] = useState<Set<string>>(() =>
    new Set(
      (boot.cached ?? []).flatMap((c) =>
        c.items.filter((i) => i.safe && i.defaultOn !== false).map((i) => i.id),
      ),
    ),
  )
  const [progress, setProgress] = useState(0)
  const [freed, setFreed] = useState(0)
  const [failed, setFailed] = useState(0)
  const [cleanedCount, setCleanedCount] = useState(0)
  /** 清理前的「可清理总量」，用于展示「清理前 → 清理后」的减少量 */
  const [beforeTotal, setBeforeTotal] = useState<number | null>(null)
  const [scannedAt, setScannedAt] = useState<number | null>(boot.at)
  const [current, setCurrent] = useState('')
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  /** 清理完成后重新统计中的标记（此期间列表显示旧值，但顶部已标明状态） */
  const [refreshing, setRefreshing] = useState(false)
  /** 清理后的重扫是否失败：失败时必须明确告知「数字可能仍是清理前状态」 */
  const [refreshFailed, setRefreshFailed] = useState(false)

  /**
   * 清理进度的帧合并。
   *
   * 后端每删一项就推一次进度（含「正在删哪个路径」），一个软件缓存目录动辄几十项。
   * 直接 setState 会让整页在几十毫秒内连续重渲染，叠加列表本身的布局计算，
   * 视觉上就是「乱闪」。这里用 requestAnimationFrame 合并到每帧最多一次，
   * 并且跳过**内容未变**的文本更新（同一个路径重复推送很常见）。
   */
  const progressFrame = useRef(0)
  const pendingProgress = useRef<{ pct?: number; cur?: string }>({})
  const currentRef = useRef('')

  const pushProgress = (pct?: number, cur?: string) => {
    if (pct !== undefined) pendingProgress.current.pct = pct
    if (cur && cur !== currentRef.current) pendingProgress.current.cur = cur
    if (progressFrame.current) return
    progressFrame.current = requestAnimationFrame(() => {
      progressFrame.current = 0
      const p = pendingProgress.current
      pendingProgress.current = {}
      if (p.pct !== undefined) setProgress(p.pct)
      if (p.cur) {
        currentRef.current = p.cur
        setCurrent(p.cur)
      }
    })
  }

  // 卸载时取消挂起的帧，避免对已卸载组件 setState
  useEffect(() => () => {
    if (progressFrame.current) cancelAnimationFrame(progressFrame.current)
  }, [])

  // 盘符列表：仅枚举（会话级缓存），不触发清理扫描
  useEffect(() => {
    getVolumes()
      .then((vs) => {
        setDrives(vs)
        setSelectedDrives((prev) => {
          if (prev.size > 0) return prev
          // 没有任何选择时：有缓存范围则沿用缓存范围，否则默认全选
          return new Set(getLastCleanDrives() ?? vs.map((v) => v.letter))
        })
      })
      .catch(() => setDrives([]))
  }, [])

  // 已勾选盘符（保持盘符顺序）
  const chosen = useMemo(
    () => drives.filter((d) => selectedDrives.has(d.letter)).map((d) => d.letter),
    [drives, selectedDrives],
  )
  const allDrivesOn = drives.length > 0 && drives.every((d) => selectedDrives.has(d.letter))

  const toggleDrive = (letter: string) =>
    setSelectedDrives((s) => {
      const n = new Set(s)
      if (n.has(letter)) n.delete(letter)
      else n.add(letter)
      return n
    })

  const toggleAllDrives = () =>
    setSelectedDrives(allDrivesOn ? new Set() : new Set(drives.map((d) => d.letter)))

  /**
   * 把一批分类结果写回界面。
   * `reselect=true`：按「默认勾选」重置选择（首次扫描 / 手动重扫）
   * `reselect=false`：清空选择（清理完成后）—— 避免页面上重新出现一片「已勾选待删除」，
   *   让人误以为没清干净、或马上要再删一遍
   */
  const applyResult = (cs: CleanCategory[], reselect = true) => {
    setCats(cs)
    setSel(
      reselect
        ? new Set(
            cs.flatMap((c) => c.items.filter((i) => i.safe && i.defaultOn !== false).map((i) => i.id)),
          )
        : new Set(),
    )
  }

  /**
   * 扫描（手动触发：首次「开始扫描」或结果态「重新扫描」）
   * 一律 `force=true` —— 用户明确点了刷新，就必须拿到最新数据。
   */
  const startScan = async () => {
    if (chosen.length === 0) return
    setPhase('scanning')
    setScanPct(0)
    setScanMsg('')
    setBeforeTotal(null)
    setFreed(0)
    setFailed(0)
    setProgress(0)
    try {
      // 后端的真实进度：静态规则逐条推进 → 深度扫描应用缓存 → 卸载残留检查
      const cs = await getCleanCategories(true, chosen, (p) => {
        setScanPct(p.pct)
        setScanMsg(p.current)
      })
      applyResult(cs)
      setScannedAt(cleanFetchedAt(chosen))
      setPhase('ready')
    } catch {
      setCats([])
      setPhase('ready')
    }
  }

  const items = useMemo(
    () => cats.flatMap((c: CleanCategory) => c.items.map((i) => ({ ...i, cat: c.name }))),
    [cats],
  )
  // 只读项（系统占用）不计入「可清理总量」，否则汇总数字会虚高
  const totalScanned = items.filter((i) => i.safe).reduce((s, i) => s + i.size, 0)
  const usedTotal = items.filter((i) => !i.safe).reduce((s, i) => s + i.size, 0)
  const totalSel = items.filter((i) => sel.has(i.id)).reduce((s, i) => s + i.size, 0)
  const selPct = totalScanned > 0 ? Math.round((totalSel / totalScanned) * 100) : 0
  const selCount = items.filter((i) => sel.has(i.id)).length
  /** 清理后相比清理前减少的可清理量 */
  const reduced = beforeTotal !== null ? Math.max(0, beforeTotal - totalScanned) : 0

  const toggle = (id: string) =>
    setSel((s) => {
      const n = new Set(s)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })

  const toggleCatAll = (cat: CleanCategory) => {
    const ids = cat.items.filter((i) => i.safe).map((i) => i.id)
    const allOn = ids.length > 0 && ids.every((id) => sel.has(id))
    setSel((s) => {
      const n = new Set(s)
      ids.forEach((id) => (allOn ? n.delete(id) : n.add(id)))
      return n
    })
  }

  const toggleCollapse = (id: string) =>
    setCollapsed((s) => {
      const n = new Set(s)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })

  /**
   * 执行清理 → 立刻重新统计（关键：不能让界面停留在清理前的数字）
   *
   * 顺序：删除 → 记录「清理前总量」→ 失效缓存 → **强制重扫同盘符** → 用新结果覆盖列表与汇总
   */
  const runCleanNow = async () => {
    const chosenItems = items.filter((i) => sel.has(i.id))
    // 应用缓存只删目录内文件、保留目录本身，这一点必须在确认框里说清楚
    const hasAppCache = chosenItems.some((i) => i.id.startsWith('appcache:'))
    const confirmText =
      `将直接删除选中的 ${chosenItems.length} 类文件（共 ${fmtBytes(totalSel, 2)}），不进回收站且不可恢复。` +
      (hasAppCache
        ? '\n\n其中「应用缓存」只清空缓存目录内的文件，目录本身保留；应用下次启动会自动重建（首次打开可能略慢）。配置文件、登录数据与数据库会被自动跳过，个人设置不受影响。'
        : '') +
      '\n\n确定继续？'
    if (!window.confirm(confirmText)) {
      return
    }
    const snapshot = totalScanned // 清理前可清理总量
    const count = chosenItems.length
    setPhase('cleaning')
    setProgress(0)
    currentRef.current = ''
    try {
      const [actual, failedCount] = await runClean([...sel], (p) => {
        // 百分比优先按「已释放 / 预估总量」算：比后端的分段百分比更贴近实际
        const pct =
          typeof p.freed === 'number' && totalSel > 0
            ? Math.min(99, Math.round((p.freed / totalSel) * 100))
            : typeof p.pct === 'number'
              ? Math.min(99, p.pct)
              : undefined
        pushProgress(pct, p.current)
      }, chosen, totalSel)
      setProgress(100)
      setFreed(actual)
      setFailed(failedCount)
      setCleanedCount(count)
      setBeforeTotal(snapshot)
      setPhase('done')
      // ① 让缓存失效，避免下次进入页面看到清理前的旧数据
      invalidateAfterClean()
      // ② 立刻重扫同一批盘符，把「剩余垃圾量」与列表刷成清理后的真实状态
      setRefreshing(true)
      setRefreshFailed(false)
      setCurrent('正在重新统计清理结果…')
      try {
        const cs = await getCleanCategories(true, chosen)
        // 清理后不自动重新勾选：保持「干净」的完成态，用户要清理更多可自行勾选
        applyResult(cs, false)
        setScannedAt(cleanFetchedAt(chosen))
      } catch {
        // 重扫失败：保留删除结果，但必须标注下方数字可能过期
        setRefreshFailed(true)
      } finally {
        setRefreshing(false)
        setCurrent('')
      }
    } catch (e) {
      setPhase('ready')
      alert(`清理失败：${e}`)
    }
  }

  const resetToIdle = () => {
    setPhase('idle')
    setCats([])
    setSel(new Set())
    setCurrent('')
    setProgress(0)
    setBeforeTotal(null)
    setFreed(0)
    setFailed(0)
    setCleanedCount(0)
  }

  // ---- 空态 / 扫描中 ----
  if (phase === 'idle' || phase === 'scanning') {
    const scanning = phase === 'scanning'
    return (
      <div className='page-enter' style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', gap: 20, padding: '24px 0' }}>
          <EmptyArtIcon icon={Trash2} />
          <div style={{ textAlign: 'center' }}>
            <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 6 }}>选择要扫描的盘符，开始扫描</div>
            <div style={{ fontSize: 12.5, color: 'var(--muted)', lineHeight: 1.7 }}>
              默认不自动扫描；勾选盘符后点击「开始扫描」，仅统计可安全清理的项目<br />
              扫描结果会在本次运行内缓存，重复进入本页直接复用，不会重复扫描
            </div>
          </div>

          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, justifyContent: 'center', maxWidth: 660 }}>
            {drives.map((d) => {
              const on = selectedDrives.has(d.letter)
              return (
                <button
                  key={d.letter}
                  onClick={() => toggleDrive(d.letter)}
                  disabled={scanning}
                  title={`${d.letter}: ${d.label || '本地磁盘'}`}
                  style={{
                    display: 'inline-flex', alignItems: 'center', gap: 8,
                    padding: '8px 14px', borderRadius: 'var(--radius-md)',
                    border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                    background: on ? 'var(--accent-soft)' : 'var(--surface)',
                    color: on ? 'var(--accent)' : 'var(--fg-2)', fontSize: 13,
                    opacity: scanning ? 0.7 : 1,
                  }}
                >
                  <span className={`checkbox${on ? ' on' : ''}`} style={{ width: 16, height: 16 }}>
                    {on && <Check size={11} />}
                  </span>
                  <HardDrive size={14} />
                  <span style={{ fontWeight: 600 }}>{d.letter}:</span>
                  <span style={{ color: 'var(--meta)', fontSize: 11.5 }}>{d.label || '本地磁盘'}</span>
                </button>
              )
            })}
            {drives.length === 0 && <div className='status-line'>正在读取盘符…</div>}
          </div>

          <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <button className='btn btn-ghost' onClick={toggleAllDrives} disabled={scanning || drives.length === 0}>
              {allDrivesOn ? '取消全选' : '全选'}
            </button>
            <button className='btn btn-primary' onClick={startScan} disabled={scanning || selectedDrives.size === 0}>
              {scanning
                ? <><Loader2 size={15} className='spin' /> 扫描中…</>
                : <><ScanSearch size={15} /> 开始扫描</>}
            </button>
          </div>

          {scanning && (
            <div style={{ width: 'min(460px, 84%)' }}>
              <div className='progress-track'>
                <div className='progress-fill' style={{ width: `${Math.max(2, scanPct)}%` }} />
              </div>
              <div className='status-line' style={{ justifyContent: 'center', marginTop: 6, gap: 8 }}>
                <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 300 }}>
                  {scanMsg || '正在准备扫描…'}
                </span>
                <span className='mono' style={{ fontSize: 11, color: 'var(--meta)', flexShrink: 0 }}>
                  {scanPct}%
                </span>
              </div>
              <div className='status-line' style={{ justifyContent: 'center', marginTop: 2, fontSize: 11 }}>
                范围：{chosen.map((l) => `${l}:`).join(' ')} · 深度扫描含应用缓存，通常需要数秒
              </div>
            </div>
          )}
        </div>
      </div>
    )
  }

  // ---- 结果态 / 清理中 / 完成 ----
  const isDone = phase === 'done'
  return (
    <div className='page-enter' style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div className='summary-bar'>
        <div>
          {/* 完成态显示「本次释放」而不是「已勾选」：避免刚清完又显示一大笔待删除量 */}
          <div className='sum-num' style={isDone ? { color: 'var(--success)' } : undefined}>
            {fmtBytes(isDone ? freed : totalSel, 2)}
          </div>
          <div className='sum-label'>{isDone ? '本次清理已释放' : '已勾选 · 直接删除（不进回收站）'}</div>
        </div>
        <div style={{ width: 1, height: 42, background: 'var(--border-soft)' }} />
        <div>
          <div className='sum-num' style={{ fontSize: 20, color: 'var(--muted)' }}>{fmtBytes(totalScanned, 2)}</div>
          <div className='sum-label'>{isDone ? '清理后剩余可清理' : '扫描到可清理总量'}</div>
        </div>
        {usedTotal > 0 && (
          <>
            <div style={{ width: 1, height: 42, background: 'var(--border-soft)' }} />
            <div>
              <div className='sum-num' style={{ fontSize: 20, color: 'var(--meta)' }}>{fmtBytes(usedTotal, 2)}</div>
              <div className='sum-label'>系统占用（不可删）</div>
            </div>
          </>
        )}
        <div className='sum-spacer' />
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <button
            className='btn btn-ghost'
            onClick={startScan}
            disabled={phase === 'cleaning' || refreshing || chosen.length === 0}
            title='重新扫描所选盘符，获取最新数据（本次运行内的结果默认复用缓存，点这里才重扫）'
          >
            <RefreshCw size={14} /> 重新扫描
          </button>
          <button className='btn btn-ghost' onClick={resetToIdle} disabled={phase === 'cleaning'}>
            <HardDrive size={14} /> 重新选择盘符
          </button>
          <button
            className='btn btn-danger'
            disabled={totalSel === 0 || phase === 'cleaning' || refreshing}
            onClick={runCleanNow}
            title={selCount === 0 ? '先在上方勾选要清理的项目' : '删除已勾选的项目（不进回收站）'}
          >
            <Trash2 size={15} /> {isDone ? `清理选中的 ${selCount} 项` : '直接删除选中项'}
          </button>
        </div>
      </div>

      {/*
        清理进度独立成行，不再嵌在 .summary-bar 里。
        原来它和 .sum-spacer 都是 flex:1，进度块宽度随「当前正在删的路径」长度变化，
        会把右侧三个按钮推来推去 —— 这是清理时页面乱晃的直接原因之一。
      */}
      {phase === 'cleaning' && (
        <div style={{ marginBottom: 14 }}>
          <div className='progress-track' style={{ height: 6 }}>
            <div className='progress-fill' style={{ width: `${progress}%` }} />
          </div>
          <div className='status-line' style={{ marginTop: 6, gap: 8 }}>
            <Loader2 size={13} className='spin' />
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1, minWidth: 0 }}>
              {current || '正在删除…'}
            </span>
            <span className='mono' style={{ fontSize: 11.5, color: 'var(--meta)', flexShrink: 0 }}>
              {Math.round(progress)}%
            </span>
          </div>
        </div>
      )}

      {/* 清理完成摘要：释放空间 / 清理前后对比 / 刷新状态 */}
      {isDone && (
        <div
          className='panel'
          style={{
            marginBottom: 14, padding: '12px 16px',
            // .panel 默认是纵向 flex + overflow:hidden；这里显式改成横向并允许内容溢出显示。
            // 窄窗口下若不让子项换行而任其收缩，整条会被压扁到只剩文字上半截（实测 900×640）。
            display: 'flex', flexDirection: 'row', alignItems: 'center',
            flexWrap: 'wrap', columnGap: 18, rowGap: 6, overflow: 'visible',
            borderColor: 'rgba(22,163,74,.35)', background: 'rgba(22,163,74,.06)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: 'var(--success)', fontWeight: 600, fontSize: 13.5, flexShrink: 0 }}>
            <Check size={16} /> 清理完成
          </div>
          <div style={{ fontSize: 12.5, flexShrink: 0 }}>
            已释放 <b className='mono' style={{ color: 'var(--success)' }}>{fmtBytes(freed, 2)}</b>
            <span style={{ color: 'var(--meta)' }}> · 处理 {cleanedCount} 项</span>
            {failed > 0 && (
              <span style={{ color: 'var(--warn)' }}> · {failed} 项被占用或权限不足已跳过</span>
            )}
          </div>
          <div style={{ fontSize: 12.5, color: 'var(--muted)', display: 'flex', alignItems: 'center', gap: 6, flexShrink: 0 }}>
            <TrendingDown size={13} />
            可清理垃圾：{beforeTotal !== null ? fmtBytes(beforeTotal, 2) : '—'}
            {' → '}
            <b style={{ color: 'var(--fg)' }}>{fmtBytes(totalScanned, 2)}</b>
            {reduced > 0 && <span style={{ color: 'var(--success)' }}>（减少 {fmtBytes(reduced, 2)}）</span>}
          </div>
          {refreshing ? (
            <div className='status-line' style={{ color: 'var(--accent)', marginLeft: 'auto', flexShrink: 0 }}>
              <Loader2 size={13} className='spin' /> 正在重新统计清理结果…
            </div>
          ) : refreshFailed ? (
            <div className='status-line' style={{ color: 'var(--warn)', marginLeft: 'auto', flexShrink: 0 }}>
              <AlertTriangle size={13} /> 重新统计失败，下方数字可能仍是清理前状态，请点「重新扫描」
            </div>
          ) : (
            <span style={{ fontSize: 11.5, color: 'var(--meta)', marginLeft: 'auto', flexShrink: 0 }}>
              以上为清理后重新扫描的真实结果 · {hhmm(scannedAt)}
            </span>
          )}
        </div>
      )}

      {/*
        勾选进度：完成态且未再勾选时，改为说明文字（不再显示满格 100% 进度条）。
        清理过程中整块隐藏 —— 此时上方已有真实删除进度，两条进度条并排反而看着乱。
      */}
      {phase === 'cleaning' ? null : isDone && selCount === 0 ? (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 16, fontSize: 11.5, color: 'var(--meta)', flexWrap: 'wrap' }}>
          <Check size={13} color="var(--success)" />
          以下为清理后的剩余项目（默认不勾选）；需要继续清理时勾选后再点右侧按钮
          <span style={{ marginLeft: 'auto' }}>扫描于 {hhmm(scannedAt)}</span>
        </div>
      ) : (
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 16 }}>
          <div className='progress-track' style={{ height: 6 }}>
            <div className='progress-fill' style={{ width: `${selPct}%`, background: 'linear-gradient(90deg, var(--accent), #4e8df2)' }} />
          </div>
          <span className='mono' style={{ fontSize: 11.5, color: 'var(--meta)', flexShrink: 0 }}>已选 {selPct}%</span>
          <span style={{ fontSize: 11.5, color: 'var(--meta)', flexShrink: 0 }}>
            扫描于 {hhmm(scannedAt)}（本次运行内复用，点「重新扫描」刷新）
          </span>
        </div>
      )}

      {items.length === 0 ? (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', color: 'var(--meta)', fontSize: 13 }}>
          {isDone ? '已全部清理干净，未发现剩余可清理项' : '未发现可清理项（所选盘符已很干净）'}
        </div>
      ) : (
        // 单层 DOM + CSS 多列：分类顺序固定，数据变化时卡片不会跨列搬迁。
        // 重扫期间不再整体降透明度（原来 opacity 0.55 会让整页闪一下），
        // 改用顶部提示 + 禁用交互表达「正在忙」。
        <div
          className='clean-cols'
          style={refreshing ? { pointerEvents: 'none' } : undefined}
          aria-busy={refreshing}
        >
          {cats.filter((c) => c.items.length > 0).map((cat) => {
            const Icon = CAT_ICON[cat.icon] ?? HardDrive
            // 与顶部「可清理总量」口径一致：分类标题只统计可清理项，
            // 不可删项（pagefile 等）单独以小字标注，避免「卡片 8GB 而汇总 0B」的错觉
            const catSize = cat.items.filter((i) => i.safe).reduce((s, i) => s + i.size, 0)
            const catLocked = cat.items.filter((i) => !i.safe).reduce((s, i) => s + i.size, 0)
            const ids = cat.items.filter((i) => i.safe).map((i) => i.id)
            const allOn = ids.length > 0 && ids.every((id) => sel.has(id))
            const isCollapsed = collapsed.has(cat.id)
            return (
              <div key={cat.id} className='clean-card'>
                <div className='clean-card-head'>
                  <button
                    className={`checkbox${allOn ? ' on' : ''}`}
                    style={{ visibility: ids.length > 0 ? 'visible' : 'hidden' }}
                    onClick={() => toggleCatAll(cat)}
                    title={allOn ? '取消全选本类' : '全选本类'}
                  >
                    <Check size={12} />
                  </button>
                      <span className='clean-card-icon' style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}>
                        <Icon size={15} />
                      </span>
                      <span style={{ fontSize: 13.5, fontWeight: 600 }}>{cat.name}</span>
                      <span className='mono' style={{ marginLeft: 'auto', fontSize: 12.5, color: 'var(--muted)' }}>
                        {fmtBytes(catSize, 2)}
                        {catLocked > 0 && (
                          <span style={{ color: 'var(--meta)', fontWeight: 400 }}>
                            {catSize > 0 ? ' · ' : ''}不可删 {fmtBytes(catLocked, 2)}
                          </span>
                        )}
                      </span>
                      <button className='icon-btn' style={{ width: 26, height: 26 }} onClick={() => toggleCollapse(cat.id)}>
                        {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                      </button>
                    </div>
                    {!isCollapsed && (
                      <div className='clean-card-body'>
                        {cat.items.map((i) => {
                          const on = sel.has(i.id)
                          return (
                            <button
                              key={i.id}
                              className='check-row'
                              style={{ opacity: i.safe ? 1 : 0.6, paddingLeft: 16 }}
                              onClick={() => i.safe && toggle(i.id)}
                              disabled={!i.safe}
                              title={i.lockReason ?? i.warning ?? i.path}
                            >
                              <span className={`checkbox${i.safe ? (on ? ' on' : '') : ' locked'}`}>
                                {i.safe ? <Check size={12} /> : <Lock size={10} />}
                              </span>
                              <div className='c-name'>
                                {i.name}
                                {i.drive && (
                                  <span className='tag tag-ssd' style={{ marginLeft: 6 }}>{i.drive}:</span>
                                )}
                                {!i.safe && (
                                  <span className='tag tag-danger' style={{ marginLeft: 8 }}>
                                    {i.readOnly ? '不可删' : '禁止'}
                                  </span>
                                )}
                                {i.safe && (i.advanced || i.defaultOn === false) && (
                                  <span className='tag tag-warn' style={{ marginLeft: 8 }}>高级</span>
                                )}
                                {i.safe && i.warning && (
                                  <span className='tag tag-bundle' style={{ marginLeft: 6 }}>有风险</span>
                                )}
                                {isDone && i.size === 0 && (
                                  <span className='tag tag-safe' style={{ marginLeft: 6 }}>已清理</span>
                                )}
                                <div className='c-path' style={{ marginTop: 1 }}>{i.path}</div>
                              </div>
                              <div className='c-size' style={{ color: i.size === 0 ? 'var(--meta)' : undefined }}>
                                {i.size > 0 ? fmtBytes(i.size, 2) : '0 B'}
                              </div>
                            </button>
                          )
                        })}
                      </div>
                    )}
                  </div>
                )
              })}
        </div>
      )}

      {/* 说明：不再使用底部固定 toast —— 它会一直浮在列表上挡住内容，
          且与管理上方的「清理完成」摘要条信息重复 */}
    </div>
  )
}

/**
 * 清理页容器：三个子功能平级放在页签里。
 *
 * 为什么拆成三个而不是塞进同一张列表：
 *   · 「垃圾」是按规则/目录批量清，默认勾选；
 *   · 「安装包」「空文件夹」是逐个文件由用户挑选，默认一个都不勾，删除也不可恢复。
 *   混在一个列表里，很容易让用户误以为「安装包也默认选好了」。
 */
const TABS = [
  { id: 'junk', label: '垃圾扫描', icon: Trash2, hint: '系统与环境垃圾，按规则批量清理' },
  { id: 'installer', label: '安装包清理', icon: PackageX, hint: '你下载过的安装包，默认不勾选' },
  { id: 'emptydir', label: '空文件夹', icon: FolderMinus, hint: '完全为空的目录，默认不勾选' },
] as const

type TabId = (typeof TABS)[number]['id']

export function Cleanup() {
  const [tab, setTab] = useState<TabId>('junk')
  const active = TABS.find((t) => t.id === tab)!

  return (
    <div
      className='page-enter'
      style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 16, flexShrink: 0, flexWrap: 'wrap' }}>
        <div className='filter-tabs'>
          {TABS.map((t) => {
            const Icon = t.icon
            return (
              <button
                key={t.id}
                className={`filter-tab${tab === t.id ? ' active' : ''}`}
                onClick={() => setTab(t.id)}
                title={t.hint}
              >
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                  <Icon size={13} />
                  {t.label}
                </span>
              </button>
            )
          })}
        </div>
        <span style={{ fontSize: 11.5, color: 'var(--meta)' }}>{active.hint}</span>
      </div>

      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        {tab === 'junk' && <JunkScan />}
        {tab === 'installer' && <InstallerClean />}
        {tab === 'emptydir' && <EmptyDirClean />}
      </div>
    </div>
  )
}
