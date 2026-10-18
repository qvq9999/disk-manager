// ============ 空间分析扫描任务层（跨页面存活的「后台扫描」） ============
//
// 为什么需要这一层：
// 目录扫描是秒级到分钟级的重任务（大容量机械盘可达数十秒）。用户点开「空间分析」后
// 往往等不及就返回总览，旧实现只把状态放在组件里，一离页就丢失：
//   · 用户在总览页完全看不到「后台仍在扫」，也无从停止；
//   · 再进来时又要从头扫一遍。
//
// 这里把扫描任务提升为模块级状态，与组件生命周期解耦，从而支持三个能力：
//   1. 后台继续扫描：离页不中断，进总览仍能看到进度
//   2. 随时停止：驱动 Rust 侧取消标志，遍历在下一个检查点退出（不是强杀线程）
//   3. 稍后再看：结果常驻本层，不受 api 层 5 分钟缓存 TTL 的限制

import { useCallback, useSyncExternalStore } from 'react'
import type { FileNode } from '../types'
import { cancelDriveScan, queryScanStatus, requestDriveScan } from './api'

export type ScanTaskStatus = 'scanning' | 'cancelling' | 'done' | 'stopped' | 'error'

export interface ScanTask {
  letter: string
  status: ScanTaskStatus
  startedAt: number
  endedAt: number | null
  /** 扫描成功后的完整目录树（稍后再看的来源） */
  tree: FileNode | null
  error: string | null
  /** 已遍历目录项数（后端轮询得到，仅作进度参考） */
  entries: number
  /** 当前正在遍历的目录 */
  current: string
  /** 用户是否已经看过结果（总览页只在没看过时提示） */
  seen: boolean
  /** 是否是启动预热发起的（用户主动扫描时要给它让路） */
  prewarm: boolean
}

const tasks = new Map<string, ScanTask>()
const listeners = new Set<() => void>()
let snapshot: ScanTask[] = []

function emit() {
  snapshot = [...tasks.values()]
  for (const l of listeners) l()
}

function subscribe(l: () => void) {
  listeners.add(l)
  return () => { listeners.delete(l) }
}

function getTasks() {
  return snapshot
}

/** 订阅全部扫描任务（总览页的后台任务条用） */
export function useScanTasks(): ScanTask[] {
  return useSyncExternalStore(subscribe, getTasks, getTasks)
}

/** 订阅单个盘符的扫描任务 */
export function useScanTask(letter: string): ScanTask | null {
  const get = useCallback(() => tasks.get(letter) ?? null, [letter])
  return useSyncExternalStore(subscribe, get, get)
}

/** 同步读取（非 React 环境 / 事件回调里用） */
export function getScanTask(letter: string): ScanTask | null {
  return tasks.get(letter) ?? null
}

export function isTaskRunning(t: ScanTask | null | undefined): boolean {
  return !!t && (t.status === 'scanning' || t.status === 'cancelling')
}

function patch(letter: string, next: Partial<ScanTask>) {
  const cur = tasks.get(letter)
  if (!cur) return
  // 不可变更新：保证 useSyncExternalStore 的引用比较能感知变化
  tasks.set(letter, { ...cur, ...next })
  emit()
}

function isCancelledErr(e: unknown): boolean {
  const s = String((e as { message?: string })?.message ?? e)
  return s.includes('SCAN_CANCELLED')
}

// ---- 进度轮询 ----
// 只有存在运行中的任务时才起定时器；后端用全局原子量记录进度，轮询成本很低。
let pollTimer: ReturnType<typeof setInterval> | null = null

function stopPoll() {
  if (pollTimer !== null) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

function ensurePoll() {
  if (pollTimer !== null) return
  pollTimer = setInterval(async () => {
    const running = [...tasks.values()].filter((t) => t.status === 'scanning')
    if (running.length === 0) {
      stopPoll()
      return
    }
    try {
      const st = await queryScanStatus()
      for (const t of running) {
        // 进度只属于正在被遍历的那个盘。其他盘此刻是在锁上排队，
        // 把别人的数字显示给他，看起来就像「扫描卡住了但数字在涨」。
        if (st.letter && st.letter !== t.letter) continue
        // 数值没变就不 emit，避免无谓的整体重渲染
        if (st.entries === t.entries && st.current === t.current) continue
        patch(t.letter, { entries: st.entries, current: st.current })
      }
    } catch {
      // 进度查询失败不影响扫描本身，静默跳过这一轮
    }
  }, 500)
}

/**
 * 发起一次扫描（已有运行中的任务则复用，不会重复扫盘）。
 * 永远不向外抛错：失败/中止都落在任务状态里，避免触发全局 unhandledrejection。
 *
 * `prewarm` 由启动预热传入：后端全盘扫描是串行的，如果预热正在扫 C 盘、
 * 用户又点了 D 盘，D 会在锁上排队，而界面上却显示着 C 盘的进度。
 * 所以用户一旦主动扫描，就先把预热任务取消掉，把磁盘让出来。
 */
export function startScan(letter: string, force = false, prewarm = false): Promise<void> {
  const exist = tasks.get(letter)
  if (isTaskRunning(exist)) return Promise.resolve()

  if (!prewarm) {
    for (const t of tasks.values()) {
      if (t.status === 'scanning' && t.prewarm && t.letter !== letter) {
        void stopScan(t.letter)
      }
    }
  }

  tasks.set(letter, {
    letter,
    status: 'scanning',
    startedAt: Date.now(),
    endedAt: null,
    // 保留上一份结果：重新扫描期间用户仍可浏览旧树，不会闪回空白页
    tree: exist?.tree ?? null,
    error: null,
    entries: 0,
    current: '',
    seen: exist?.seen ?? false,
    prewarm,
  })
  emit()
  ensurePoll()

  return requestDriveScan(letter, force)
    .then((tree) => {
      patch(letter, { status: 'done', tree, endedAt: Date.now(), current: '', error: null })
    })
    .catch((e: unknown) => {
      if (isCancelledErr(e)) {
        patch(letter, { status: 'stopped', endedAt: Date.now(), error: null, current: '' })
      } else {
        patch(letter, { status: 'error', endedAt: Date.now(), error: String((e as { message?: string })?.message ?? e) })
      }
    })
    .finally(() => {
      if (![...tasks.values()].some((t) => t.status === 'scanning')) stopPoll()
    })
}

/** 请求停止：状态先切到 cancelling，后端返回 SCAN_CANCELLED 后转为 stopped */
export async function stopScan(letter: string): Promise<void> {
  const t = tasks.get(letter)
  if (!isTaskRunning(t)) return
  patch(letter, { status: 'cancelling' })
  try {
    await cancelDriveScan()
  } catch {
    // 取消失败说明扫描可能刚好结束，交给扫描 Promise 的收尾逻辑判定
  }
}

/** 标记结果已被查看（总览页不再提示「已完成」） */
export function markScanSeen(letter: string) {
  const t = tasks.get(letter)
  if (!t || t.seen) return
  patch(letter, { seen: true })
}

/** 从任务列表移除（用户点「忽略」；不影响磁盘与缓存） */
export function dismissScan(letter: string) {
  if (!tasks.delete(letter)) return
  emit()
}
