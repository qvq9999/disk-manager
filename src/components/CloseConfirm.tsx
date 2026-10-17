import { useEffect, useState } from 'react'
import { Minimize2, LogOut, AlertTriangle, X, Loader2 } from 'lucide-react'
import { getAppSettings, resolveClose } from '../services/api'
import { isTaskRunning, useScanTasks } from '../services/scanTasks'

/**
 * 点窗口 ✕ 时的选择弹窗。
 *
 * 之所以要用自绘弹窗而不是系统对话框：需要在关闭前把「正在后台跑什么」一起告知用户
 * （退出会中断扫描、清理），系统对话框做不到这种上下文。
 *
 * Rust 侧已经 prevent_close，窗口还活着；这里给出选择后才真正执行。
 */
export function CloseConfirm({ onCancel }: { onCancel: () => void }) {
  const [remember, setRemember] = useState(false)
  const [busy, setBusy] = useState<'background' | 'exit' | null>(null)
  const [trayAvailable, setTrayAvailable] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const tasks = useScanTasks()
  const running = tasks.filter(isTaskRunning)

  useEffect(() => {
    let alive = true
    getAppSettings()
      .then((s) => { if (alive) setTrayAvailable(s.trayAvailable) })
      .catch(() => { /* 取不到就按可用处理，至少不挡住关闭 */ })
    return () => { alive = false }
  }, [])

  // Esc = 取消（保持窗口打开）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onCancel() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  const act = async (action: 'background' | 'exit') => {
    if (busy) return
    setBusy(action)
    setError(null)
    try {
      await resolveClose(action, remember)
      // 「直接关闭」会让进程立即结束，走不到这里；能到这里说明是退到后台
      if (action === 'background') onCancel()
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e))
      setBusy(null)
    }
  }

  return (
    <div className="modal-mask" onClick={onCancel}>
      <div className="modal-panel" onClick={(e) => e.stopPropagation()} style={{ width: 'min(480px, 100%)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 6 }}>
          <span style={{ fontSize: 15, fontWeight: 600, flex: 1 }}>要关闭存储管理吗？</span>
          <button className="icon-btn" onClick={onCancel} aria-label="取消关闭"><X size={16} /></button>
        </div>

        {/* 后台仍在跑的任务：退出会直接中断它们，必须提前讲清楚 */}
        {running.length > 0 && (
          <div style={{
            display: 'flex', gap: 8, alignItems: 'flex-start',
            padding: '8px 12px', marginBottom: 12, borderRadius: 'var(--radius-md)',
            background: 'rgba(217,119,6,.08)', border: '1px solid rgba(217,119,6,.22)',
            fontSize: 12, lineHeight: 1.6, color: 'var(--fg-2)',
          }}>
            <AlertTriangle size={14} color="var(--warn)" style={{ flexShrink: 0, marginTop: 1 }} />
            <span>
              {running.map((t) => `${t.letter.toUpperCase()}: 盘空间分析`).join('、')} 仍在后台进行。
              选择「直接关闭」会中断扫描，且本次结果不会保留。
            </span>
          </div>
        )}

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 4 }}>
          <Choice
            icon={<Minimize2 size={18} />}
            title="退到后台"
            desc={trayAvailable
              ? '隐藏窗口继续运行，正在进行的任务不会中断；从任务栏托盘图标可重新打开'
              : '当前无法创建托盘图标，该选项不可用'}
            disabled={!trayAvailable}
            busy={busy === 'background'}
            tone="accent"
            onClick={() => act('background')}
          />
          <Choice
            icon={<LogOut size={18} />}
            title="直接关闭"
            desc="立即退出程序；后台扫描、清理等未完成任务会随之中断"
            busy={busy === 'exit'}
            tone="danger"
            onClick={() => act('exit')}
          />
        </div>

        {error && (
          <div style={{ marginTop: 10, fontSize: 12, color: 'var(--danger)', lineHeight: 1.6 }}>{error}</div>
        )}

        <label style={{
          display: 'flex', alignItems: 'center', gap: 8, marginTop: 16,
          fontSize: 12.5, color: 'var(--muted)', cursor: 'pointer', userSelect: 'none',
        }}>
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
            style={{ width: 14, height: 14, accentColor: 'var(--accent)', cursor: 'pointer' }}
          />
          记住这次选择（之后可在「设置」中修改）
        </label>

        <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 14 }}>
          <button className="btn btn-ghost" style={{ padding: '5px 14px', fontSize: 12.5 }} onClick={onCancel}>
            取消
          </button>
        </div>
      </div>
    </div>
  )
}

function Choice({ icon, title, desc, onClick, disabled, busy, tone }: {
  icon: React.ReactNode
  title: string
  desc: string
  onClick: () => void
  disabled?: boolean
  busy?: boolean
  tone: 'accent' | 'danger'
}) {
  const [hover, setHover] = useState(false)
  const color = tone === 'accent' ? 'var(--accent)' : 'var(--danger)'
  return (
    <button
      onClick={onClick}
      disabled={disabled || busy}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: 'flex', alignItems: 'flex-start', gap: 12, width: '100%',
        padding: '11px 14px', textAlign: 'left',
        borderRadius: 'var(--radius-md)',
        border: `1px solid ${hover && !disabled ? color : 'var(--border-soft)'}`,
        background: hover && !disabled ? (tone === 'accent' ? 'var(--accent-soft)' : 'rgba(220,38,38,.06)') : 'var(--surface)',
        opacity: disabled ? 0.45 : 1,
        cursor: disabled ? 'not-allowed' : 'pointer',
        transition: 'border-color var(--t-fast), background var(--t-fast)',
      }}
    >
      <span style={{ color, flexShrink: 0, marginTop: 1 }}>
        {busy ? <Loader2 size={18} className="spin" /> : icon}
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{ display: 'block', fontSize: 13.5, fontWeight: 600, color: 'var(--fg)' }}>{title}</span>
        <span style={{ display: 'block', fontSize: 11.5, color: 'var(--meta)', marginTop: 3, lineHeight: 1.6 }}>{desc}</span>
      </span>
    </button>
  )
}
