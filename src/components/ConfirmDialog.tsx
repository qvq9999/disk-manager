// ============ 危险操作二次确认弹窗 ============
//
// 为什么不用 window.confirm：
// 1. 它是浏览器原生对话框，字体、按钮、间距全部与产品界面不一致，看起来像网页而不是程序；
// 2. 原生框只能放纯文本，没法把「预计释放多少」「几类文件」这类关键数字排成一眼能扫的结构；
// 3. 原生框的按钮位置由系统决定，危险操作的主按钮可能落在一个更顺手的位置上。
//
// 用法（两步，避免把确认逻辑写进业务代码）：
//   const { ask, node } = useConfirm()
//   if (!(await ask({ ... }))) return
//   ...真正执行
//   return <>...{node}</>

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { AlertTriangle, Trash2, X } from 'lucide-react'

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
