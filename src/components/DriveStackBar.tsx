import type { Volume } from '../types'

/** 顶部多盘横向对比堆叠条（条形用于多盘对比，环形用于单盘快读） */
export function DriveStackBar({ v, color }: { v: Volume; color: string }) {
  const pct = Math.round((v.usedBytes / v.totalBytes) * 100)
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 200, flex: 1 }}>
      <span className="mono" style={{ fontSize: 12, fontWeight: 600 }}>{v.letter}:</span>
      <div className="progress-track" style={{ height: 6 }}>
        <div className="progress-fill" style={{ width: `${pct}%`, background: color }} />
      </div>
      <span className="mono" style={{ fontSize: 11, color: 'var(--meta)', width: 34, textAlign: 'right' }}>{pct}%</span>
    </div>
  )
}
