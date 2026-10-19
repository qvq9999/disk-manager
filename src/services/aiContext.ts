// ============ 页面上下文（让 AI 知道你现在在看什么） ============
//
// 问题：用户问「我的配置怎么样」时，AI 只知道「你在硬件信息页」，答不出具体配置。
// 做法：页面自己把「愿意分享的那部分数据」压成一段摘要注册上来，提问时拼进 system 上下文。
//
// 三个刻意的约束：
// 1. **页面决定分享什么**，不是全量上报。硬件页只给 CPU/内存/显卡这类配置项，
//    不传计算机名、不传含用户名的路径。
// 2. **只有当前页有效**。切页即失效，不会把上一页的数据带过去。
// 3. **摘要由页面自己限长**（建议 500 字内），避免某页塞太多把 token 费用顶上去。

import { useEffect, useSyncExternalStore } from 'react'

let currentPage = ''
let currentSummary = ''

const subs = new Set<() => void>()

function notify() {
  for (const f of subs) f()
}

function subscribe(cb: () => void) {
  subs.add(cb)
  return () => { subs.delete(cb) }
}

function snapshot() {
  return currentSummary
}

/** 当前页面的上下文摘要；没有则为空串 */
export function useAiContext(): string {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}

/**
 * 页面组件用：把本页的数据摘要注册为 AI 上下文。
 *
 * 卸载时会清空，但**只在「当前仍是自己」时才清** ——
 * 否则旧页面退出时会把刚挂载的新页面上下文一起清掉。
 */
export function useAiPageContext(page: string, summary: string) {
  useEffect(() => {
    currentPage = page
    currentSummary = summary
    notify()
    return () => {
      if (currentPage === page) {
        currentPage = ''
        currentSummary = ''
        notify()
      }
    }
  }, [page, summary])
}
