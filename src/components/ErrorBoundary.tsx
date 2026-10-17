import { Component, type ReactNode } from 'react'
import { AlertTriangle, RefreshCw } from 'lucide-react'

interface State { error: Error | null }

/** 页面级错误边界：单个页面渲染异常时只影响该页，不至于整个应用无响应 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error) {
    console.error('[page error]', error)
  }

  render() {
    if (this.state.error) {
      return (
        <div className="page-enter" style={{ padding: 32 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: 'var(--danger)', marginBottom: 10 }}>
            <AlertTriangle size={16} />
            <span style={{ fontWeight: 600 }}>此页面渲染出错</span>
          </div>
          <div className="mono" style={{ fontSize: 12, color: 'var(--muted)', marginBottom: 14, wordBreak: 'break-all' }}>
            {this.state.error.message}
          </div>
          <button className="btn btn-ghost" onClick={() => this.setState({ error: null })}>
            <RefreshCw size={14} /> 重新加载此页
          </button>
        </div>
      )
    }
    return this.props.children
  }
}
