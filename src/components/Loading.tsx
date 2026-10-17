/** 统一页面加载态：旋转指示 + 文案 + 骨架屏（提升加载视觉反馈） */
export function PageLoading({ label, hint }: { label: string; hint?: string }) {
  return (
    <div className="page-enter loading-wrap">
      <div className="loading-spinner" />
      <div className="loading-label">{label}</div>
      {hint && <div className="loading-hint">{hint}</div>}
      <div className="skeleton-list">
        <div className="skeleton-bar" style={{ width: '62%' }} />
        <div className="skeleton-bar" style={{ width: '78%' }} />
        <div className="skeleton-bar" style={{ width: '46%' }} />
      </div>
    </div>
  )
}

/** 内联小型加载态（用于面板内部） */
export function InlineLoading({ label }: { label: string }) {
  return (
    <div className="status-line" style={{ justifyContent: 'center', padding: 28 }}>
      <span className="loading-spinner small" />
      {label}
    </div>
  )
}
