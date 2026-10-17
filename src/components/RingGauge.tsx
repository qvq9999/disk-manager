import { fmtBytes } from '../utils/format'

/** 环形容量仪表（SVG，单盘状态快读） */
export function RingGauge({ pct, color, size = 84 }: { pct: number; color: string; size?: number }) {
  const r = size / 2 - 7
  const c = 2 * Math.PI * r
  const filled = (Math.min(pct, 100) / 100) * c
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} role="img" aria-label={`已用 ${pct}%`}>
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--surface-warm)" strokeWidth="9" />
      <circle
        cx={size / 2} cy={size / 2} r={r} fill="none"
        stroke={color} strokeWidth="9" strokeLinecap="round"
        strokeDasharray={`${filled} ${c - filled}`}
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
        style={{ transition: 'stroke-dasharray 600ms cubic-bezier(0.2,0,0,1)' }}
      />
    </svg>
  )
}

export function RingCenter({ pct, free, sub }: { pct: number; free: number; sub?: string }) {
  return (
    <div className="ring-center">
      <div>
        <div className="ring-pct">{pct}%</div>
        <div style={{ fontSize: 10, color: 'var(--meta)' }}>{sub ?? `剩 ${fmtBytes(free, 0)}`}</div>
      </div>
    </div>
  )
}
