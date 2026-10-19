// ============ 危险操作二次确认弹窗 ============
//
// 为什么不用 window.confirm：
// 1. 它是浏览器原生对话框，字体、按钮、间距全部与产品界面不一致，看起来像网页而不是程序；
// 2. 原生框只能放纯文本，没法把「预计释放多少」「几类文件」这类关键数字排成一眼能扫的结构；
// 3. 原生框的按钮位置由系统决定，危险操作的主按钮可能落在一个更顺手的位置上。
//
// 两种用法：
//
// ① 纯确认（点确认即关闭，由调用方自己去做事）：
//      const { ask, node } = useConfirm()
//      if (!(await ask({ ... }))) return
//      ...真正执行
//      return <>...{node}</>
//
// ② 确认 + 执行 + 结果三态（v1.3.6 新增）：
//      const { run, node } = useTaskDialog()
//      const report = await run(spec, async (ctx) => { ... ctx.onProgress({...}) ... })
//
// 为什么要有 ②：清理进度画在页面上时，进度条的宽度会随「当前正在删的路径」长度变化，
// 把旁边的按钮推来推去（页面乱晃）；而且用户点完确认后视线还在弹窗上，
// 进度跑到页面顶部反而要多找一眼。把「确认 → 执行 → 结果」放在同一个弹窗里，
// 视觉上是一条连续的线：我确认了 → 它正在做 → 做完了。

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { AlertTriangle, CheckCircle2, Loader2, Trash2, X } from 'lucide-react'

export interface ConfirmSpec {
  title: string
  subtitle?: string
  /** 关键数字，通常两三项：预计释放 / 数量 */
  stats?: { label: string; value: string; strong?: boolean }[]
  /** 风险提示（红底），一句话讲清「不可恢复」这类后果 */
  warning?: ReactNode
  /** 中性说明（灰底）：口径、边界、不会影响什么 */
  note?: ReactNode
  confirmText?: string
  cancelText?: string
}

export function ConfirmDialog({
  spec,
  onCancel,
  onConfirm,
}: {
  spec: ConfirmSpec
  onCancel: () => void
  onConfirm: () => void
}) {
  // Esc 等同取消：危险操作只允许被「主动确认」触发
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  return (
    <div className="modal-mask" onClick={onCancel} role="dialog" aria-modal="true" aria-labelledby="cfm-title">
      <div className="confirm-panel" onClick={(e) => e.stopPropagation()}>
        <div className="confirm-head">
          <span className="confirm-badge"><Trash2 size={18} /></span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div id="cfm-title" style={{ fontSize: 14.5, fontWeight: 600, letterSpacing: '-0.01em' }}>
              {spec.title}
            </div>
            {spec.subtitle && (
              <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 3 }}>{spec.subtitle}</div>
            )}
          </div>
          <button className="icon-btn" onClick={onCancel} aria-label="取消">
            <X size={15} />
          </button>
        </div>

        {spec.stats && spec.stats.length > 0 && (
          <div className="confirm-stats">
            {spec.stats.map((s) => (
              <div key={s.label} className="confirm-stat">
                <span className="confirm-stat-label">{s.label}</span>
                <span className={`confirm-stat-value${s.strong ? ' strong' : ''}`}>{s.value}</span>
              </div>
            ))}
          </div>
        )}

        {spec.warning && (
          <div className="confirm-warn">
            <AlertTriangle size={13} style={{ flexShrink: 0, marginTop: 2 }} />
            <span>{spec.warning}</span>
          </div>
        )}

        {spec.note && <div className="confirm-note">{spec.note}</div>}

        <div className="confirm-foot">
          <button className="btn btn-ghost" style={{ padding: '6px 14px', fontSize: 12.5 }} onClick={onCancel} autoFocus>
            {spec.cancelText ?? '取消'}
          </button>
          <button className="btn btn-danger" style={{ padding: '6px 15px', fontSize: 12.5 }} onClick={onConfirm}>
            {spec.confirmText ?? '确认'}
          </button>
        </div>
      </div>
    </div>
  )
}

/**
 * 把「弹窗等用户点确认」变成一次 await。
 *
 * 用 ref 保存 resolver 而不是在 setState 的 updater 里 resolve：
 * StrictMode 下 updater 可能被调用两次，副作用会重复执行。
 */
export function useConfirm(): {
  ask: (spec: ConfirmSpec) => Promise<boolean>
  node: ReactNode
} {
  const [spec, setSpec] = useState<ConfirmSpec | null>(null)
  const resolver = useRef<((ok: boolean) => void) | null>(null)

  const ask = useCallback((s: ConfirmSpec) => {
    return new Promise<boolean>((resolve) => {
      resolver.current = resolve
      setSpec(s)
    })
  }, [])

  const settle = useCallback((ok: boolean) => {
    setSpec(null)
    const r = resolver.current
    resolver.current = null
    r?.(ok)
  }, [])

  const node = spec ? (
    <ConfirmDialog spec={spec} onCancel={() => settle(false)} onConfirm={() => settle(true)} />
  ) : null

  return { ask, node }
}

// ============ 三态任务弹窗（确认 → 执行 → 结果） ============

export interface TaskProgress {
  /** 0~100，缺省表示"不确定进度"，用流动条表示 */
  pct?: number
  /** 当前正在处理的路径 / 动作 */
  current?: string
  /** 累计已释放字节 */
  freed?: number
  /** 已完成项数 / 总数（可选，比百分比更稳定） */
  done?: number
  total?: number
}

export interface TaskSpec<T> extends ConfirmSpec {
  /** 执行态标题，默认沿用 title */
  runningTitle?: string
  /** 结果态标题 */
  doneTitle?: string
  /** 执行态的次按钮文案（点它会触发 ctx.cancel()） */
  stopText?: string
  /** 结果态摘要。返回 null 表示用内置文案。 */
  renderDone?: (result: T, err: string | null) => ReactNode
}

export interface TaskCtx {
  /** 上报进度（内部按帧合并，频繁调用不会让界面抖） */
  onProgress: (p: TaskProgress) => void
  /** 用户点了「停止」——由任务自行决定如何中断（通常是通知后端） */
  signal: { readonly aborted: boolean }
  cancel: () => void
}

type Phase = 'confirm' | 'running' | 'done'

interface State<T> {
  spec: TaskSpec<T> | null
  phase: Phase
  progress: TaskProgress
  result: T | null
  error: string | null
  stopping: boolean
}

/**
 * 用法：
 *   const { run, node } = useTaskDialog<CleanReport>()
 *   const report = await run(spec, async (ctx) => {
 *     const r = await runClean(ids, (p) => ctx.onProgress(p))
 *     return r
 *   })
 *   // 弹窗此时停在「结果态」，用户点完成后关闭；report 已经可用于后续逻辑
 *
 * 返回值在任务结束时就 resolve（不阻塞调用方去刷新列表），弹窗的关闭是独立的一步。
 */
export function useTaskDialog<T>(): {
  run: (spec: TaskSpec<T>, task: (ctx: TaskCtx) => Promise<T>) => Promise<T | null>
  node: ReactNode
} {
  const [state, setState] = useState<State<T>>({
    spec: null,
    phase: 'confirm',
    progress: {},
    result: null,
    error: null,
    stopping: false,
  })
  const resolver = useRef<((v: T | null) => void) | null>(null)
  const aborted = useRef(false)
  const frame = useRef(0)
  const pending = useRef<TaskProgress>({})

  const flush = useCallback(() => {
    frame.current = 0
    const p = pending.current
    pending.current = {}
    if (Object.keys(p).length === 0) return
    setState((s) => (s.phase === 'running' ? { ...s, progress: { ...s.progress, ...p } } : s))
  }, [])

  // 卸载时取消挂起的帧
  useEffect(() => () => {
    if (frame.current) cancelAnimationFrame(frame.current)
  }, [])

  const onProgress = useCallback((p: TaskProgress) => {
    pending.current = { ...pending.current, ...p }
    if (frame.current) return
    frame.current = requestAnimationFrame(flush)
  }, [flush])

  // 待执行的任务：确认按钮按下之前只保存引用，绝不提前启动
  const taskRef = useRef<((ctx: TaskCtx) => Promise<T>) | null>(null)

  const startTask = useCallback(
    async (task: (ctx: TaskCtx) => Promise<T>) => {
      const ctx: TaskCtx = {
        onProgress,
        signal: { get aborted() { return aborted.current } },
        cancel: () => {
          aborted.current = true
          setState((s) => ({ ...s, stopping: true }))
        },
      }
      setState((s) => ({ ...s, phase: 'running', stopping: false, progress: {} }))
      try {
        const r = await task(ctx)
        aborted.current = false
        setState((s) => ({ ...s, phase: 'done', result: r, error: null, stopping: false }))
        resolver.current?.(r)
      } catch (e) {
        aborted.current = false
        const msg = e instanceof Error ? e.message : String(e)
        setState((s) => ({ ...s, phase: 'done', error: msg, stopping: false }))
        resolver.current?.(null)
      }
      resolver.current = null
    },
    [onProgress],
  )

  /** 打开弹窗（停在确认态）。只有用户点确认后才会执行 task。 */
  const run = useCallback((spec: TaskSpec<T>, task: (ctx: TaskCtx) => Promise<T>) => {
    taskRef.current = task
    return new Promise<T | null>((resolve) => {
      aborted.current = false
      resolver.current = resolve
      setState({ spec, phase: 'confirm', progress: {}, result: null, error: null, stopping: false })
    })
  }, [])

  const close = useCallback(() => {
    const r = resolver.current
    resolver.current = null
    setState((s) => ({ ...s, spec: null }))
    // 确认态关闭 = 用户取消，返回 null
    r?.(null)
  }, [])

  const node = state.spec ? (
    <TaskDialog
      spec={state.spec}
      phase={state.phase}
      progress={state.progress}
      result={state.result}
      error={state.error}
      stopping={state.stopping}
      onCancel={close}
      onConfirm={() => {
        const t = taskRef.current
        if (t) void startTask(t)
      }}
      onStop={() => {
        aborted.current = true
        setState((s) => ({ ...s, stopping: true }))
      }}
    />
  ) : null

  // run 与 open 同义，保留 run 名字给调用方
  return { run, node }
}

function fmtBytes(n: number): string {
  if (!n) return '0 B'
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(4, Math.floor(Math.log(n) / Math.log(1024)))
  return `${(n / 1024 ** i).toFixed(2)} ${u[i]}`
}

function TaskDialog<T>({
  spec,
  phase,
  progress,
  result,
  error,
  stopping,
  onCancel,
  onConfirm,
  onStop,
}: {
  spec: TaskSpec<T>
  phase: Phase
  progress: TaskProgress
  result: T | null
  error: string | null
  stopping: boolean
  onCancel: () => void
  onConfirm: () => void
  onStop: () => void
}) {
  // 执行态禁止用 Esc 关闭：一旦开始删除，关掉弹窗只会让人以为"是不是没执行"
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      if (phase === 'confirm') onCancel()
      else if (phase === 'running' && !stopping) onStop()
      else if (phase === 'done') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [phase, stopping, onCancel, onStop])

  const icon =
    phase === 'done' ? (
      error ? <AlertTriangle size={18} /> : <CheckCircle2 size={18} />
    ) : phase === 'running' ? (
      <Loader2 size={18} className="spin" />
    ) : (
      <Trash2 size={18} />
    )

  const title =
    phase === 'running'
      ? spec.runningTitle ?? spec.title
      : phase === 'done'
        ? error
          ? '操作未完成'
          : spec.doneTitle ?? '已完成'
        : spec.title

  return (
    <div
      className="modal-mask"
      onClick={() => {
        if (phase === 'confirm') onCancel()
        else if (phase === 'done') onCancel()
      }}
      role="dialog"
      aria-modal="true"
      aria-labelledby="task-title"
    >
      <div className="confirm-panel" onClick={(e) => e.stopPropagation()}>
        <div className="confirm-head">
          <span
            className="confirm-badge"
            style={
              phase === 'done' && !error
                ? { background: 'rgba(22,163,74,.10)', borderColor: 'rgba(22,163,74,.22)', color: 'var(--success)' }
                : phase === 'running'
                  ? { background: 'var(--accent-soft)', borderColor: 'var(--accent-soft)', color: 'var(--accent)' }
                  : undefined
            }
          >
            {icon}
          </span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div id="task-title" style={{ fontSize: 14.5, fontWeight: 600, letterSpacing: '-0.01em' }}>
              {title}
            </div>
            {phase === 'confirm' && spec.subtitle && (
              <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 3 }}>{spec.subtitle}</div>
            )}
          </div>
          {/* 执行态不给关闭按钮：只能"停止"，避免误以为删完了 */}
          {phase !== 'running' && (
            <button className="icon-btn" onClick={onCancel} aria-label="关闭">
              <X size={15} />
            </button>
          )}
        </div>

        {phase === 'confirm' && (
          <>
            {spec.stats && spec.stats.length > 0 && (
              <div className="confirm-stats">
                {spec.stats.map((s) => (
                  <div key={s.label} className="confirm-stat">
                    <span className="confirm-stat-label">{s.label}</span>
                    <span className={`confirm-stat-value${s.strong ? ' strong' : ''}`}>{s.value}</span>
                  </div>
                ))}
              </div>
            )}
            {spec.warning && (
              <div className="confirm-warn">
                <AlertTriangle size={13} style={{ flexShrink: 0, marginTop: 2 }} />
                <span>{spec.warning}</span>
              </div>
            )}
            {spec.note && <div className="confirm-note">{spec.note}</div>}
            <div className="confirm-foot">
              <button
                className="btn btn-ghost"
                style={{ padding: '6px 14px', fontSize: 12.5 }}
                onClick={onCancel}
                autoFocus
              >
                {spec.cancelText ?? '取消'}
              </button>
              <button
                className="btn btn-danger"
                style={{ padding: '6px 15px', fontSize: 12.5 }}
                onClick={onConfirm}
              >
                {spec.confirmText ?? '确认'}
              </button>
            </div>
          </>
        )}

        {phase === 'running' && (
          <>
            <div className="confirm-progress">
              <div className="progress-track" style={{ height: 6, flex: 'unset', width: '100%' }}>
                <div
                  className={`progress-fill${progress.pct === undefined ? ' indeterminate' : ''}`}
                  style={progress.pct === undefined ? undefined : { width: `${progress.pct}%` }}
                />
              </div>
              <div className="confirm-progress-meta">
                {progress.done !== undefined && progress.total ? (
                  <span className="mono" style={{ flexShrink: 0 }}>
                    {progress.done}/{progress.total} 项
                  </span>
                ) : null}
                <span className="confirm-progress-current" title={progress.current}>
                  {stopping ? '正在停止…' : progress.current || '正在准备…'}
                </span>
                {progress.pct !== undefined && (
                  <span className="mono" style={{ flexShrink: 0 }}>{Math.round(progress.pct)}%</span>
                )}
              </div>
              {progress.freed !== undefined && progress.freed > 0 && (
                <div className="confirm-progress-meta">
                  <span style={{ color: 'var(--success)' }}>
                    已释放 <b className="mono">{fmtBytes(progress.freed)}</b>
                  </span>
                </div>
              )}
            </div>
            <div className="confirm-foot">
              <button
                className="btn btn-ghost"
                style={{ padding: '6px 14px', fontSize: 12.5 }}
                onClick={onStop}
                disabled={stopping}
              >
                {stopping ? '正在停止…' : spec.stopText ?? '停止'}
              </button>
            </div>
          </>
        )}

        {phase === 'done' && (
          <>
            {error ? (
              <div className="confirm-warn" style={{ marginTop: 12 }}>
                <AlertTriangle size={13} style={{ flexShrink: 0, marginTop: 2 }} />
                <span>{error}</span>
              </div>
            ) : (
              spec.renderDone?.(result as T, error) ?? (
                <div className="confirm-note" style={{ marginTop: 12 }}>
                  操作已完成。
                </div>
              )
            )}
            <div className="confirm-foot">
              <button
                className="btn btn-primary"
                style={{ padding: '6px 16px', fontSize: 12.5 }}
                onClick={onCancel}
                autoFocus
              >
                完成
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
