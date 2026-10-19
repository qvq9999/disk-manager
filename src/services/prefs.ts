// ============ 运行期偏好（AI 相关开关） ============
//
// 为什么单独放一层：这两个开关同时被「设置页（写）」和「空间分析 / AI 助手（读）」用到，
// 而它们不在同一棵组件树里。用模块级状态 + useSyncExternalStore，
// 设置页一改，正在显示的空间分析页立刻跟着变，不需要重新加载。

import { useSyncExternalStore } from 'react'

export interface Prefs {
  /** 进入目录后自动分析这一层 */
  autoAnalyzeLayer: boolean
  /** 允许把当前页面数据摘要作为 AI 上下文 */
  allowPageContext: boolean
}

let state: Prefs = {
  // 与 Rust 侧 AppSettings::default 保持一致（自动分析默认关，避免默默消耗额度）
  autoAnalyzeLayer: false,
  allowPageContext: true,
}

const subs = new Set<() => void>()

export function setPrefs(p: Partial<Prefs>) {
  const next = { ...state, ...p }
  if (next.autoAnalyzeLayer === state.autoAnalyzeLayer && next.allowPageContext === state.allowPageContext) {
    return
  }
  state = next
  for (const f of subs) f()
}

export function getPrefs(): Prefs {
  return state
}

export function usePrefs(): Prefs {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb)
      return () => { subs.delete(cb) }
    },
    () => state,
    () => state,
  )
}
