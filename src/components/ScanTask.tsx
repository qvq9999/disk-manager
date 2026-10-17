import { ArrowLeft, Square, RotateCcw, CheckCircle2, AlertTriangle, Eye, X, FolderSearch } from 'lucide-react'
import type { Volume } from '../types'
import type { ScanTask } from '../services/scanTasks'
import { dismissScan, markScanSeen, stopScan, useScanTasks } from '../services/scanTasks'
import { fmtDuration } from '../utils/format'

/** 大数字加千分位，进度计数用 */
function nfmt(n: number): string {
  return n.toLocaleString('zh-CN')
}

/**
 * 空间分析的等待态。
 *
 * 关键点：扫描不会再因为「用户离开页面」而中断，因此这里必须同时给出
 *   ① 明确的进行中反馈（已遍历多少、正在扫哪个目录）
 *   ② 主动停止入口（停止后台扫描）
 *   ③ 「先回总览、稍后再看」的出口（后台继续跑，结果保留）
 */
export function ScanWaitCard({ volume, task, onBack, onRetry }: {
  volume: Volume
  task: ScanTask | null
  onBack: () => void
  onRetry: () => void
}) {
  const letter = volume.letter.toUpperCase()
  const status = task?.status ?? 'scanning'
  const running = status === 'scanning' || status === 'cancelling'

  if (status === 'stopped') {
    return (
      <div className="page-enter loading-wrap" style={{ minHeight: 360 }}>
        <div className="quick-icon" style={{ width: 44, height: 44, background: 'var(--surface-warm)', color: 'var(--muted)' }}>
          <Square size={20} />
        </div>
        <div className="loading-label">已停止扫描 {letter}: 盘</div>
        <div className="loading-hint" style={{ textAlign: 'center', maxWidth: 460, lineHeight: 1.7 }}>
          扫描在中途被停止，本次未生成结果。
          <br />
          残缺的目录树会让各目录的体积占比失真，因此不展示半成品数据。
        </div>
        <div style={{ display: 'flex', gap: 8, marginTop: 4 }}>
          <button className="btn btn-primary" onClick={onRetry}><RotateCcw size={14} /> 重新扫描</button>
          <button className="btn btn-ghost" onClick={onBack}><ArrowLeft size={14} /> 返回总览</button>
        </div>
      </div>
    )
  }

  if (status === 'error') {
    return (
      <div className="page-enter loading-wrap" style={{ minHeight: 360 }}>
        <div className="quick-icon" style={{ width: 44, height: 44, background: 'rgba(220,38,38,.10)', color: 'var(--danger)' }}>
          <AlertTriangle size={20} />
        </div>
        <div className="loading-label">扫描 {letter}: 盘失败</div>
        <div className="loading-hint" style={{ textAlign: 'center', maxWidth: 520, lineHeight: 1.7, wordBreak: 'break-word' }}>
          {task?.error ?? '未知错误'}
        </div>
        <div style={{ display: 'flex', gap: 8, marginTop: 4 }}>
          <button className="btn btn-primary" onClick={onRetry}><RotateCcw size={14} /> 重试</button>
          <button className="btn btn-ghost" onClick={onBack}><ArrowLeft size={14} /> 返回总览</button>
        </div>
      </div>
    )
  }

  // scanning / cancelling / 尚未建立任务的首帧
  const entries = task?.entries ?? 0
  return (
    <div className="page-enter loading-wrap" style={{ minHeight: 360 }}>
      <div className="loading-spinner" />
      <div className="loading-label">
        {status === 'cancelling' ? `正在停止扫描 ${letter}: 盘…` : `正在扫描 ${letter}: 盘`}
      </div>

      <div style={{
        display: 'flex', alignItems: 'center', gap: 10, marginTop: 2,
        padding: '7px 14px', borderRadius: 'var(--radius-md)',
        background: 'var(--surface-warm)', border: '1px solid var(--border-soft)',
        maxWidth: 520, minWidth: 320,
      }}>
        <span className="mono" style={{ fontSize: 12, color: 'var(--fg-2)', flexShrink: 0 }}>
          已遍历 {nfmt(entries)} 项
        </span>
        <span style={{ width: 1, height: 12, background: 'var(--border)', flexShrink: 0 }} />
        <span className="mono" style={{
          fontSize: 11.5, color: 'var(--meta)', flex: 1, minWidth: 0,
          overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
        }} title={task?.current}>
          {task?.current || '正在准备遍历…'}
        </span>
      </div>

      <div className="loading-hint" style={{ textAlign: 'center', maxWidth: 470, lineHeight: 1.7 }}>
        目录遍历模式，大盘可能需要数十秒。可以随时停止；
        也可以先回总览，扫描会在后台继续，稍后回来看结果。
      </div>

      <div style={{ display: 'flex', gap: 8, marginTop: 4, flexWrap: 'wrap', justifyContent: 'center' }}>
        <button
          className="btn btn-ghost"
          onClick={() => stopScan(letter)}
          disabled={status === 'cancelling'}
          title="立即停止后台扫描（不做强制杀线程，遍历会在下一个检查点退出）"
        >
          <Square size={13} /> {status === 'cancelling' ? '正在停止…' : '停止扫描'}
        </button>
        <button className="btn btn-ghost" onClick={onBack} title="返回总览，扫描在后台继续，稍后可回来查看">
          <ArrowLeft size={14} /> 返回总览（后台继续）
        </button>
      </div>

      <div className="skeleton-list">
        <div className="skeleton-bar" style={{ width: '62%' }} />
        <div className="skeleton-bar" style={{ width: '78%' }} />
        <div className="skeleton-bar" style={{ width: '46%' }} />
      </div>
    </div>
  )
}

/**
 * 总览页顶部的后台扫描任务条。
 *
 * 存在的意义：「返回后还在后台加载」这件事本身是合理的（用户不该被迫等待），
 * 但必须变成**可见、可控**的 —— 用户要能一眼看出还在扫、能停止、能稍后查看结果。
 */
export function ScanTaskBanner({ volumes, onOpen }: {
  volumes: Volume[]
  onOpen: (v: Volume) => void
}) {
  const tasks = useScanTasks()
  const visible = tasks.filter((t) => {
    if (t.status === 'scanning' || t.status === 'cancelling') return true
    if (t.status === 'done' && !t.seen) return true
    return false
  })
  if (visible.length === 0) return null

  const volOf = (letter: string) => volumes.find((v) => v.letter.toUpperCase() === letter.toUpperCase())

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
      {visible.map((t) => {
        const v = volOf(t.letter)
        const running = t.status === 'scanning' || t.status === 'cancelling'
        const done = t.status === 'done'
        return (
          <div
            key={t.letter}
            style={{
              display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap',
              padding: '9px 14px', borderRadius: 'var(--radius-lg)',
              background: done ? 'rgba(22,163,74,.06)' : 'var(--accent-soft)',
              border: `1px solid ${done ? 'rgba(22,163,74,.22)' : 'var(--border-soft)'}`,
              fontSize: 12.5,
            }}
          >
            {done
              ? <CheckCircle2 size={15} color="var(--success)" style={{ flexShrink: 0 }} />
              : <span className="loading-spinner small" style={{ flexShrink: 0 }} />}

            <span style={{ fontWeight: 600, flexShrink: 0 }}>{t.letter.toUpperCase()}: 盘空间分析</span>

            {running ? (
              <>
                <span style={{ color: 'var(--muted)' }}>
                  {t.status === 'cancelling' ? '正在停止…' : '后台进行中'}
                </span>
                {t.entries > 0 && (
                  <span className="mono" style={{ fontSize: 11.5, color: 'var(--meta)' }}>
                    已遍历 {nfmt(t.entries)} 项
                  </span>
                )}
                {t.current && (
                  <span className="mono" style={{
                    fontSize: 11.5, color: 'var(--meta)', maxWidth: 260,
                    overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                  }} title={t.current}>{t.current}</span>
                )}
              </>
            ) : (
              <span style={{ color: 'var(--muted)' }}>
                已完成 · 用时 {fmtDuration((t.endedAt ?? Date.now()) - t.startedAt)}
              </span>
            )}

            <span style={{ flex: 1 }} />

            {running ? (
              <>
                <button
                  className="btn btn-ghost"
                  style={{ padding: '3px 10px', fontSize: 12 }}
                  onClick={() => v && onOpen(v)}
                >
                  <FolderSearch size={12} /> 查看进度
                </button>
                <button
                  className="btn btn-ghost"
                  style={{ padding: '3px 10px', fontSize: 12 }}
                  onClick={() => stopScan(t.letter)}
                  disabled={t.status === 'cancelling'}
                  title="停止这个后台扫描"
                >
                  <Square size={12} /> 停止
                </button>
              </>
            ) : (
              <>
                <button
                  className="btn btn-ghost"
                  style={{ padding: '3px 10px', fontSize: 12, borderColor: 'var(--success)', color: 'var(--success)' }}
                  onClick={() => { if (v) { markScanSeen(t.letter); onOpen(v) } }}
                >
                  <Eye size={12} /> 查看结果
                </button>
                <button
                  className="icon-btn"
                  style={{ width: 24, height: 24 }}
                  onClick={() => dismissScan(t.letter)}
                  title="忽略这条提示（结果仍保留）"
                  aria-label="忽略"
                >
                  <X size={13} />
                </button>
              </>
            )}
          </div>
        )
      })}
    </div>
  )
}
