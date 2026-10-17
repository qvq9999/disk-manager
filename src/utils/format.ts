// ============ 工具函数 ============

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

export function fmtBytes(n: number, digits?: number): string {
  if (n <= 0) return '0 B'
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), UNITS.length - 1)
  const v = n / 1024 ** i
  const d = digits ?? (i === 0 ? 0 : v >= 100 ? 1 : v >= 10 ? 1 : 2)
  return `${v.toFixed(d)} ${UNITS[i]}`
}

export function fmtPct(used: number, total: number): number {
  return total > 0 ? Math.round((used / total) * 100) : 0
}

/** 占用率语义色: <85 绿 | 85-95 琥珀 | >95 红 */
export function capColor(pct: number): 'ok' | 'warn' | 'danger' {
  if (pct > 95) return 'danger'
  if (pct > 85) return 'warn'
  return 'ok'
}

export function capVar(pct: number): string {
  const s = capColor(pct)
  return s === 'danger' ? 'var(--cap-danger)' : s === 'warn' ? 'var(--cap-warn)' : 'var(--cap-free)'
}

/** 毫秒耗时 → 人类可读（0.8 秒 / 12 秒 / 1 分 07 秒），用于扫描用时提示 */
export function fmtDuration(ms: number): string {
  const sec = Math.max(0, ms) / 1000
  if (sec < 1) return `${sec.toFixed(1)} 秒`
  if (sec < 60) return `${Math.round(sec)} 秒`
  const m = Math.floor(sec / 60)
  const s = Math.round(sec % 60)
  return `${m} 分 ${String(s).padStart(2, '0')} 秒`
}
