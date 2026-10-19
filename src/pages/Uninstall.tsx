import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Search, Package, PackageOpen, Store, Loader2, Check,
  ScanSearch, Trash2, AlertTriangle, AppWindow, X, ArrowUpDown, ListChecks,
} from 'lucide-react'
import type { InstalledApp, ResidualItem, AppType, CleanReport } from '../types'
import { getWin32Apps, getStoreApps, peekInstalledApps, uninstallApp, cancelUninstall, checkInstalled, forceRemoveApp, scanResidualsFor, cleanResiduals, getAppIcon, invalidateApps, getInstalledApps } from '../services/api'
import type { UninstallOutcome } from '../services/api'
import { PageLoading } from '../components/Loading'
import { fmtBytes } from '../utils/format'

type Filter = 'all' | AppType | 'bundle'
/**
 * waiting = 卸载器已启动但我们不再阻塞等待（多半弹了交互向导），改为轮询注册表判断完成
 *   —— 这是「卸载一直转圈关不掉」的解法：控制权立刻交还用户
 */
type Phase = 'idle' | 'uninstalling' | 'waiting' | 'scanning' | 'found'
type SortKey = 'name' | 'size' | 'date'

const RESIDUAL_LABEL: Record<ResidualItem['type'], string> = {
  registry: '注册表',
  folder: '目录',
  shortcut: '快捷方式',
  startup: '开机启动',
  startupfile: '启动文件夹',
  service: '服务',
  task: '计划任务',
  firewall: '防火墙规则',
  pathenv: 'PATH 变量',
  class: 'COM/文件关联',
  uninstallkey: '卸载登记',
}

const FILTERS: [Filter, string][] = [
  ['all', '全部'], ['win32', 'Win32'], ['store', '商店'],
  ['bundle', '疑似捆绑'],
]

const SORTS: [SortKey, string][] = [
  ['name', '名称'], ['size', '大小'], ['date', '安装时间'],
]

export function Uninstall() {
  // 首帧直接用缓存（5 分钟内）：避免每次进入都重跑 Get-AppxPackage（很慢）
  const [apps, setApps] = useState<InstalledApp[]>(() => peekInstalledApps() ?? [])
  const [icons, setIcons] = useState<Record<string, string>>({})
  const [q, setQ] = useState('')
  const [filter, setFilter] = useState<Filter>('all')
  const [sort, setSort] = useState<SortKey>('name')
  const [residualApp, setResidualApp] = useState<InstalledApp | null>(null)
  const [residuals, setResiduals] = useState<ResidualItem[]>([])
  const [phase, setPhase] = useState<Phase>('idle')
  const [statusMsg, setStatusMsg] = useState('')
  const [outcomeMsg, setOutcomeMsg] = useState('')
  const [stillInstalled, setStillInstalled] = useState(false)
  const [cleaning, setCleaning] = useState(false)
  /** 残留清理进度（在弹窗内显示，不另开窗口） */
  const [residProgress, setResidProgress] = useState<{ done: number; total: number; current: string } | null>(null)
  /** 残留清理的逐项结果：哪一项成功、哪一项要重启、哪一项为什么没删掉 */
  const [residReport, setResidReport] = useState<CleanReport | null>(null)
  // 批量卸载队列（BCU 式：多选 → 顺序执行）
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [batch, setBatch] = useState<InstalledApp[]>([])
  const [batchIndex, setBatchIndex] = useState(0)
  const batchCancel = useRef(false)

  // ---- 卸载等待控制 ----
  /** 用户已关闭弹窗：后续 await 回来的结果一律丢弃，避免弹窗又自己冒出来 */
  const aborted = useRef(false)
  /** 用户点了「停止等待」 */
  const waitCancel = useRef(false)
  /** 用户点了「我已经卸完了」 */
  const waitDone = useRef(false)
  /** 已等待秒数（轮询阶段展示） */
  const [waitSecs, setWaitSecs] = useState(0)

  useEffect(() => {
    let cancelled = false
    const sortByName = (list: InstalledApp[]) =>
      [...list].sort((a, b) => a.name.localeCompare(b.name, 'zh-Hans-CN'))

    // 图标懒加载（结果在数据层缓存，切页不重复提取）
    const loadIcons = (list: InstalledApp[]) => {
      let idx = 0
      const worker = async () => {
        while (idx < list.length) {
          const app = list[idx++]
          try {
            const url = await getAppIcon(app.id)
            if (url && !cancelled) setIcons((m) => ({ ...m, [app.id]: url }))
          } catch { /* 回落通用图标 */ }
        }
      }
      Promise.all([worker(), worker()])
    }

    // ① 先加载 Win32 软件（注册表枚举，很快）→ 列表先出来
    getWin32Apps().then((list) => {
      if (cancelled) return
      setApps((prev) => sortByName([...list, ...prev.filter((p) => p.type === 'store')]))
      loadIcons(list)
    }).catch(() => {})
    // ② 商店应用随后加载（Get-AppxPackage 较慢）→ 到达后合并
    getStoreApps().then((list) => {
      if (cancelled) return
      setApps((prev) => sortByName([...prev.filter((p) => p.type !== 'store'), ...list]))
      loadIcons(list)
    }).catch(() => {})

    return () => { cancelled = true }
  }, [])

  const visible = useMemo(() => {
    const base = apps.filter((a) => !a.uninstalled)
    const f = base.filter((a) =>
      filter === 'all' ? true
        : filter === 'bundle' ? a.bundle
        : a.type === filter,
    )
    const needle = q.trim().toLowerCase()
    const searched = needle ? f.filter((a) => a.name.toLowerCase().includes(needle)) : f
    const sorted = [...searched]
    if (sort === 'size') sorted.sort((a, b) => (b.sizeBytes || 0) - (a.sizeBytes || 0))
    else if (sort === 'date') sorted.sort((a, b) => (b.installDate || '').localeCompare(a.installDate || ''))
    else sorted.sort((a, b) => a.name.localeCompare(b.name, 'zh-Hans-CN'))
    return sorted
  }, [apps, q, filter, sort])

  const count = (t: Filter) => {
    const base = apps.filter((a) => !a.uninstalled)
    return t === 'all' ? base.length
      : t === 'bundle' ? base.filter((a) => a.bundle).length
      : base.filter((a) => a.type === t).length
  }

  // ---- 多选 / 批量 ----
  const toggleSelect = (a: InstalledApp) => {
    if (a.system) return
    setSelected((s) => {
      const n = new Set(s)
      if (n.has(a.id)) n.delete(a.id)
      else n.add(a.id)
      return n
    })
  }

  const selectedApps = useMemo(
    () => apps.filter((a) => selected.has(a.id) && !a.uninstalled),
    [apps, selected],
  )

  /**
   * 单条卸载 → 等卸载程序结束（或用户自己卸完）→ 自动扫描残留 → 弹出残留弹窗
   *
   * 关键改动：后端不再「卡住等卸载器退出」。
   * 交互式卸载器（NSIS 向导 / InstallShield）一旦弹窗就会等人操作，
   * 后端最多观察 20 秒即交还控制权并返回 pending，这里改为轮询注册表判断完成 ——
   * 界面全程可关闭、可停止等待，不会再出现「一直转圈只能强杀进程」。
   */
  const uninstallNow = async (app: InstalledApp) => {
    setResidualApp(app)
    setResiduals([])
    setResidReport(null)
    setOutcomeMsg('')
    setStillInstalled(false)
    setWaitSecs(0)
    aborted.current = false
    waitCancel.current = false
    waitDone.current = false
    setPhase('uninstalling')
    setStatusMsg(`正在卸载「${app.name}」：若弹出卸载向导，请按提示完成…`)

    let outcome: UninstallOutcome = { ok: false, message: '', stillInstalled: true }
    try {
      outcome = await uninstallApp(app.id)
    } catch (e) {
      outcome = { ok: false, message: `卸载调用失败：${e}`, stillInstalled: true }
    }
    if (aborted.current) return

    if (outcome.pending) {
      // 卸载器仍在运行：转为轮询，界面立刻恢复可操作
      setOutcomeMsg(outcome.message)
      setPhase('waiting')
      const finished = await pollUninstalled(app)
      if (aborted.current) return
      if (!finished) {
        setOutcomeMsg('已停止等待。若卸载向导仍未完成，请在里面完成后重新卸载，或直接点「强制移除」。')
        setStillInstalled(true)
        invalidateApps()
        setPhase('found')
        return
      }
      outcome = { ok: true, message: '卸载已完成', stillInstalled: false }
    }

    setOutcomeMsg(outcome.message)
    setStillInstalled(outcome.stillInstalled)
    if (outcome.ok && !outcome.stillInstalled) {
      setApps((s) => s.map((a) => (a.id === app.id ? { ...a, uninstalled: true } : a)))
    }
    invalidateApps()

    setPhase('scanning')
    setStatusMsg(`正在扫描「${app.name}」残留…`)
    try {
      const items = await scanResidualsFor(app)
      setResiduals(items)
    } catch {
      setResiduals([])
    }
    if (aborted.current) return
    setPhase('found')
  }

  /** 轮询注册表确认卸载完成：3 秒一次，最多 3 分钟 */
  const pollUninstalled = async (app: InstalledApp): Promise<boolean> => {
    for (let s = 3; s <= 180; s += 3) {
      await new Promise((r) => setTimeout(r, 3000))
      if (waitDone.current) return true
      if (waitCancel.current || aborted.current) return false
      setWaitSecs(s)
      if (!(await checkInstalled(app.id))) return true
    }
    return false
  }

  /** 关闭弹窗：任何阶段都可关闭，且会把后端从等待循环里叫停 */
  const closeDialog = () => {
    aborted.current = true
    waitCancel.current = true
    if (phase === 'uninstalling') cancelUninstall()
    setPhase('idle')
    setResidualApp(null)
    setResiduals([])
    setResidReport(null)
  }

  /** 批量卸载：按顺序逐个执行，支持跳过剩余并结束 */
  const runBatch = async (list: InstalledApp[]) => {
    if (list.length === 0) return
    if (!window.confirm(
      `将依次卸载所选 ${list.length} 个应用。卸载过程中请勿关闭窗口；若弹出卸载向导，请按提示完成。\n确定继续？`
    )) return
    setBatch(list)
    setBatchIndex(0)
    batchCancel.current = false
    const okIds = new Set<string>()
    let failed = 0
    let paused: string | null = null
    for (let i = 0; i < list.length; i++) {
      if (batchCancel.current) break
      setBatchIndex(i)
      try {
        const r = await uninstallApp(list[i].id, true)
        // 卸载器弹了交互向导：批处理必须让位，否则会同时开多个卸载器互相打架
        if (r.pending) {
          paused = list[i].name
          break
        }
        if (r.ok && !r.stillInstalled) okIds.add(list[i].id)
        else failed += 1
      } catch {
        failed += 1
      }
    }
    const cancelled = batchCancel.current
    const left = list.length - okIds.size - failed
    setBatch([])
    setBatchIndex(0)
    setApps((s) => s.map((a) => (okIds.has(a.id) ? { ...a, uninstalled: true } : a)))
    invalidateApps()
    setSelected(new Set())
    getInstalledApps(true).then(setApps).catch(() => {})
    if (paused) {
      alert(`「${paused}」的卸载向导仍在运行，批量卸载已暂停。\n\n请先完成它的卸载面板，再继续处理其余 ${left} 个。已完成 ${okIds.size} 个${failed ? `，失败 ${failed} 个` : ''}。`)
    } else if (cancelled) {
      alert(`已跳过剩余项：成功 ${okIds.size} 个，未处理 ${left} 个，失败 ${failed} 个。`)
    } else {
      alert(`批量卸载完成：成功 ${okIds.size} 个${failed ? `，失败 ${failed} 个` : ''}。`)
    }
  }

  const residSel = residuals.filter((r) => r.selected)
  const residSize = residSel.reduce((s, r) => s + r.size, 0)

  const forceRemoveNow = async () => {
    if (!residualApp) return
    if (!window.confirm(
      `强制卸载将直接删除「${residualApp.name}」的程序文件、注册表项与相关残留，不依赖其自带卸载器。\n`
      + '适用于卸载器损坏（如报"卸载入口参数无效"）的软件。此操作不可撤销，确定继续？'
    )) return
    setCleaning(true)
    setStatusMsg('正在强制卸载（删除程序文件与注册表）…')
    try {
      const r = await forceRemoveApp(residualApp.id)
      setOutcomeMsg(r.message)
      setStillInstalled(r.stillInstalled)
      if (!r.stillInstalled) {
        setApps((s) => s.map((a) => (a.id === residualApp.id ? { ...a, uninstalled: true } : a)))
      }
      invalidateApps()
      setStatusMsg('正在复核残留…')
      const items = await scanResidualsFor(residualApp)
      setResiduals(items)
    } catch (e) {
      alert(`强制卸载失败：${e}`)
    } finally {
      setCleaning(false)
      setStatusMsg('')
    }
  }

  const cleanResidualsNow = async () => {
    if (!residualApp) return
    setCleaning(true)
    setStatusMsg('正在清理残留…')
    setResidReport(null)
    setResidProgress({ done: 0, total: residSel.length, current: '正在准备…' })
    try {
      const r = await cleanResiduals(residuals, (p) =>
        setResidProgress({ done: p.done, total: p.total, current: p.current }),
      )
      setResidReport(r)
      // 清完立刻重扫：留下的就是"没删掉的那些"，比只报一个数字有用得多
      const items = await scanResidualsFor(residualApp)
      setResiduals(items)
      getInstalledApps(true).then(setApps).catch(() => {})
    } catch (e) {
      setResidReport({
        freed: 0,
        ok: 0,
        failed: residSel.length,
        pendingReboot: 0,
        items: [{ path: '', ok: false, freed: 0, pendingReboot: false, reason: String(e) }],
      })
    } finally {
      setCleaning(false)
      setResidProgress(null)
      setStatusMsg('')
    }
  }

  /** 关闭残留弹窗并复位 */
  const finishResiduals = () => {
    setPhase('idle')
    setResiduals([])
    setResidualApp(null)
    setStatusMsg('')
    setOutcomeMsg('')
    setStillInstalled(false)
    setResidReport(null)
    invalidateApps()
  }

  return (
    <div className="page-enter" style={{ display: 'flex', flexDirection: 'column', height: '100%', gap: 16 }}>
      {/* 筛选 / 排序 / 搜索 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
        <div className="filter-tabs">
          {FILTERS.map(([k, label]) => (
            <button key={k} className={`filter-tab${filter === k ? ' active' : ''}`} onClick={() => setFilter(k)}>
              {label}<span className="n">{count(k)}</span>
            </button>
          ))}
        </div>
        <div className="filter-tabs">
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, padding: '0 6px', color: 'var(--meta)', fontSize: 12 }}>
            <ArrowUpDown size={12} /> 排序
          </span>
          {SORTS.map(([k, label]) => (
            <button key={k} className={`filter-tab${sort === k ? ' active' : ''}`} onClick={() => setSort(k)}>{label}</button>
          ))}
        </div>
        <div className="sum-spacer" />
        <div className="search-box">
          <Search size={14} color="var(--meta)" />
          <input placeholder="搜索已安装软件…" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      </div>

      {/* 批量操作条 */}
      {selectedApps.length > 0 && (
        <div className="summary-bar" style={{ marginBottom: 0, padding: '10px 16px' }}>
          <span style={{ fontSize: 13 }}>
            已选 <b style={{ color: 'var(--accent)' }}>{selectedApps.length}</b> 个应用准备批量卸载
          </span>
          <div className="sum-spacer" />
          <button className="btn btn-ghost" onClick={() => setSelected(new Set())}>清空选择</button>
          <button className="btn btn-danger" onClick={() => runBatch(selectedApps)}>
            <ListChecks size={14} /> 批量卸载所选（{selectedApps.length}）
          </button>
        </div>
      )}

      {/* 应用列表（多选 + 排序） */}
      <div className="panel" style={{ flex: 1 }}>
        <div className="panel-head">
          <AppWindow size={15} /> 已安装软件
          <span className="count">{visible.length} 个 · 勾选可批量卸载 · 含微软商店应用</span>
        </div>
        <div className="panel-body">
          {apps.length === 0 && (
            <PageLoading label="正在枚举已安装软件" hint="Win32 注册表卸载键 + 微软商店应用（首次约 2–5 秒）" />
          )}
          {visible.map((a) => (
            <div key={a.id} className={`app-row${selected.has(a.id) ? ' selected' : ''}`}>
              <button
                className={`checkbox${selected.has(a.id) ? ' on' : ''}${a.system ? ' locked' : ''}`}
                onClick={() => toggleSelect(a)}
                disabled={a.system}
                title={a.system ? '系统组件，禁止批量卸载' : (selected.has(a.id) ? '取消选择' : '选择此项')}
                aria-label={selected.has(a.id) ? '取消选择' : '选择'}
              >
                {selected.has(a.id) && <Check size={12} />}
              </button>
              <div className="app-icon">
                {icons[a.id] ?? a.iconData
                  ? (
                    <img
                      src={icons[a.id] ?? a.iconData}
                      alt=""
                      /* 万一 data URL 解不出来：把这一项从缓存里摘掉，回落成通用图标，
                         避免界面上出现一个破图占位 */
                      onError={() =>
                        setIcons((m) => {
                          const n = { ...m }
                          delete n[a.id]
                          return n
                        })
                      }
                    />
                  )
                  : (a.type === 'store' ? <Store size={22} /> : <Package size={22} />)}
              </div>
              <div className="app-main">
                <div className="app-name">
                  {a.name}
                  {a.type === 'store' && <span className="tag tag-store"><Store size={9} style={{ verticalAlign: -1, marginRight: 3 }} />商店</span>}
                  {a.bundle && <span className="tag tag-bundle">疑似捆绑</span>}
                  {a.system && <span className="tag tag-safe">系统</span>}
                </div>
                <div className="app-sub">
                  {a.version}{a.version && ' · '}{a.publisher || '未知发布者'}
                  {a.uninstalling && <span style={{ color: 'var(--warn)' }}> · 正在卸载…</span>}
                </div>
              </div>
              <div className="app-date">{a.installDate || '—'}</div>
              <div className="app-size">{a.sizeBytes > 0 ? fmtBytes(a.sizeBytes, 1) : '—'}</div>
              <div className="app-actions">
                {a.uninstalling ? (
                  <Loader2 size={16} color="var(--warn)" className="spin" />
                ) : (
                  <button
                    className="btn btn-ghost"
                    style={{ padding: '5px 12px', fontSize: 12.5 }}
                    disabled={a.system}
                    title={a.system ? '系统组件，默认隐藏保护' : '卸载完成后自动扫描残留'}
                    onClick={() => uninstallNow(a)}
                  >
                    {a.system ? '禁止' : <><PackageOpen size={13} /> 卸载</>}
                  </button>
                )}
              </div>
            </div>
          ))}
          {apps.length > 0 && visible.length === 0 && (
            <div style={{ display: 'grid', placeItems: 'center', padding: 48, color: 'var(--meta)', fontSize: 13 }}>
              没有匹配的软件
            </div>
          )}
        </div>
      </div>

      {/* 批量卸载进度弹窗 */}
      {batch.length > 0 && (
        <div className="modal-mask">
          <div className="modal-panel">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
              <Loader2 size={18} className="spin" color="var(--accent)" />
              <span style={{ fontSize: 15, fontWeight: 600, flex: 1 }}>批量卸载中</span>
              <button className="icon-btn" onClick={() => { batchCancel.current = true }} title="跳过剩余并结束" aria-label="跳过剩余并结束">
                <X size={16} />
              </button>
            </div>
            <div className="mono" style={{ fontSize: 12.5, color: 'var(--muted)', marginBottom: 10 }}>
              {batchIndex + 1} / {batch.length} · {batch[batchIndex]?.name}
            </div>
            <div className="progress-track">
              <div className="progress-fill" style={{ width: `${Math.round((batchIndex / batch.length) * 100)}%` }} />
            </div>
            <div className="status-line" style={{ padding: '14px 0' }}>
              <ScanSearch size={15} /> 正在卸载「{batch[batchIndex]?.name}」：若弹出卸载向导，请按提示完成…
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
              <button className="btn btn-ghost" onClick={() => { batchCancel.current = true }}>跳过剩余并结束</button>
            </div>
          </div>
        </div>
      )}

      {/* 卸载 / 残留扫描：模态弹窗 */}
      {residualApp && phase !== 'idle' && (
        <div className="modal-mask">
          <div className="modal-panel">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
              {phase === 'found'
                ? <AlertTriangle size={18} color={residuals.length > 0 ? 'var(--warn)' : 'var(--success)'} />
                : <Loader2 size={18} className="spin" color="var(--accent)" />}
              <span style={{ fontSize: 15, fontWeight: 600, flex: 1 }}>
                {phase === 'uninstalling' && '正在卸载'}
                {phase === 'waiting' && '等待卸载完成'}
                {phase === 'scanning' && '正在扫描残留'}
                {phase === 'found' && (residuals.length > 0 ? `发现 ${residuals.length} 项残留` : '未发现残留')}
              </span>
              {/* 任何阶段都能关闭：这是「转圈关不掉只能强杀」的直接修复 */}
              <button className="icon-btn" onClick={closeDialog} aria-label="关闭" title="关闭（不会中断已启动的卸载程序）">
                <X size={16} />
              </button>
            </div>

            <div className="mono" style={{ fontSize: 12.5, color: 'var(--muted)', marginBottom: 10, wordBreak: 'break-all' }}>
              {residualApp.name}
              {residuals.length > 0 && ` · 共 ${fmtBytes(residSize, 2)}`}
            </div>

            {phase === 'found' && outcomeMsg && (
              <div style={{
                fontSize: 12.5, lineHeight: 1.6, marginBottom: 10, padding: '7px 10px', borderRadius: 8,
                background: stillInstalled ? 'rgba(217,119,6,.10)' : 'rgba(22,163,74,.10)',
                border: `1px solid ${stillInstalled ? 'rgba(217,119,6,.35)' : 'rgba(22,163,74,.30)'}`,
                color: stillInstalled ? 'var(--warn)' : 'var(--success)',
              }}>
                {stillInstalled ? '⚠ ' : '✓ '}{outcomeMsg}
              </div>
            )}

            {/* 残留清理进度：直接在弹窗内，和进度条同一个视区 */}
            {cleaning && residProgress && (
              <div className="confirm-progress" style={{ marginBottom: 10 }}>
                <div className="progress-track" style={{ height: 6, flex: 'unset', width: '100%' }}>
                  <div
                    className="progress-fill"
                    style={{
                      width: `${residProgress.total > 0 ? Math.min(99, Math.round((residProgress.done / residProgress.total) * 100)) : 0}%`,
                    }}
                  />
                </div>
                <div className="confirm-progress-meta">
                  <span className="mono" style={{ flexShrink: 0 }}>
                    {residProgress.done}/{residProgress.total} 项
                  </span>
                  <span className="confirm-progress-current" title={residProgress.current}>
                    {residProgress.current}
                  </span>
                </div>
              </div>
            )}

            {/* 清理结果：成功 / 需重启 / 失败原因，一项一项说清楚 */}
            {!cleaning && residReport && (
              <div
                style={{
                  marginBottom: 10, padding: '9px 11px', borderRadius: 8, fontSize: 12, lineHeight: 1.65,
                  background: residReport.failed > 0 ? 'rgba(217,119,6,.10)' : 'rgba(22,163,74,.10)',
                  border: `1px solid ${residReport.failed > 0 ? 'rgba(217,119,6,.35)' : 'rgba(22,163,74,.30)'}`,
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 6, fontWeight: 600, color: residReport.failed > 0 ? 'var(--warn)' : 'var(--success)' }}>
                  {residReport.failed > 0 ? <AlertTriangle size={14} /> : <Check size={14} />}
                  已清理 {residReport.ok} 项 · 释放 {fmtBytes(residReport.freed, 2)}
                  {residReport.failed > 0 && ` · ${residReport.failed} 项未删除`}
                  {residReport.pendingReboot > 0 && `（其中 ${residReport.pendingReboot} 项需重启后生效）`}
                </div>
                {residReport.items.filter((i) => !i.ok && i.reason).slice(0, 4).map((i, n) => (
                  <div key={`${i.path}-${n}`} style={{ color: 'var(--muted)', marginTop: 4, wordBreak: 'break-all' }}>
                    {i.reason}{i.path ? `：${i.path}` : ''}
                  </div>
                ))}
              </div>
            )}

            {phase === 'waiting' ? (
              <div style={{ padding: '6px 0 2px' }}>
                <div style={{ fontSize: 12.5, lineHeight: 1.75, color: 'var(--muted)', marginBottom: 10 }}>
                  {outcomeMsg || '卸载程序仍在运行。完成后会自动继续，也可以在里面完成后点下面的按钮。'}
                </div>
                <div className="status-line" style={{ marginBottom: 12 }}>
                  <Loader2 size={13} className="spin" />
                  {waitSecs === 0
                    ? '正在检查卸载状态…'
                    : `已等待 ${waitSecs} 秒 · 每 3 秒检查一次，最多 3 分钟`}
                </div>
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  <button
                    className="btn btn-primary"
                    style={{ padding: '6px 12px', fontSize: 12.5 }}
                    onClick={() => { waitDone.current = true }}
                  >
                    <Check size={14} /> 我已经卸完了
                  </button>
                  <button
                    className="btn btn-ghost"
                    style={{ padding: '6px 12px', fontSize: 12.5 }}
                    onClick={() => { waitCancel.current = true }}
                  >
                    停止等待
                  </button>
                </div>
              </div>
            ) : phase !== 'found' ? (
              <div className="status-line" style={{ padding: '18px 0' }}>
                <ScanSearch size={15} />
                {statusMsg || '处理中…'}
              </div>
            ) : residuals.length === 0 ? (
              <div style={{ fontSize: 13, color: stillInstalled ? 'var(--warn)' : 'var(--success)', display: 'flex', alignItems: 'flex-start', gap: 8, padding: '10px 0', lineHeight: 1.6 }}>
                {stillInstalled ? <AlertTriangle size={16} /> : <Check size={16} />}
                {stillInstalled
                  ? '未扫描到残留记录，但该软件仍在已安装列表中——卸载可能未真正完成，建议重新卸载，或手动检查其安装目录。'
                  : '未发现残留：该软件已清理干净。'}
              </div>
            ) : (
              <>
                <div style={{ fontSize: 12.5, color: 'var(--muted)', marginBottom: 6 }}>
                  以下为卸载完成后扫描到的残留，勾选后可一并清理（目录/快捷方式直接删除，注册表项直接移除）：
                </div>
                <div style={{ maxHeight: 300, overflowY: 'auto', border: '1px solid var(--border-soft)', borderRadius: 8 }}>
                  {residuals.map((r) => (
                    <button key={r.id + r.path} className="residual-item" style={{ width: '100%', textAlign: 'left' }}
                      onClick={() => setResiduals((s) => s.map((x) => (x.id === r.id && x.path === r.path ? { ...x, selected: !x.selected } : x)))}>
                      <span className={`checkbox${r.selected ? ' on' : ''}`}><Check size={12} /></span>
                      <span className="r-type">{RESIDUAL_LABEL[r.type] ?? r.type}</span>
                      {(r.confidence ?? 100) < 60 && (
                        <span className="tag tag-warn" style={{ flexShrink: 0 }} title="名称相似度较低，默认不勾选；确认后自行勾选">
                          疑似
                        </span>
                      )}
                      <span className="r-path">
                        {r.path}
                        {r.detail && <span style={{ color: 'var(--meta)', marginLeft: 8 }}>· {r.detail}</span>}
                      </span>
                      <span className="c-size" style={{ fontSize: 11.5, color: 'var(--meta)', flexShrink: 0 }}>
                        {r.size > 0 ? fmtBytes(r.size, 1) : '—'}
                      </span>
                    </button>
                  ))}
                </div>
                <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 14, flexWrap: 'wrap' }}>
                  {stillInstalled && (
                    <button
                      className="btn btn-ghost"
                      style={{ borderColor: 'var(--danger)', color: 'var(--danger)' }}
                      disabled={cleaning}
                      title="直接删除程序文件与注册表（Geek 式强制卸载，适用于卸载器损坏的软件）"
                      onClick={forceRemoveNow}
                    >
                      <Trash2 size={13} /> 强制卸载（删除文件+注册表）
                    </button>
                  )}
                  <span style={{ flex: 1 }} />
                  {residReport ? (
                    <button className="btn btn-primary" onClick={finishResiduals}>完成</button>
                  ) : (
                    <>
                      <button className="btn btn-ghost" onClick={finishResiduals} disabled={cleaning}>忽略</button>
                      <button className="btn btn-danger" disabled={residSel.length === 0 || cleaning} onClick={cleanResidualsNow}>
                        {cleaning ? <><Loader2 size={13} className="spin" /> 处理中…</> : <><Trash2 size={13} /> 清理选中残留（{residSel.length}）</>}
                      </button>
                    </>
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
