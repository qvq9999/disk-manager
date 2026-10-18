// ============ 原生窗口行为 ============
//
// Tauri 的 Windows 后端是 WebView2，默认保留一整套浏览器行为：右键菜单、F5 刷新、
// Ctrl+P 打印、Ctrl+滚轮缩放、把文件拖进窗口就导航过去……这些在桌面软件里都是「露馅」的。
//
// 这里统一接管。原则：
// - **输入框不牺牲体验**：右键给自定义的剪切/复制/粘贴/全选，而不是一律屏蔽；
// - **业务右键交给组件**：文档级只负责掐掉浏览器默认菜单，具体菜单项由页面自己给；
// - **不拦 Ctrl+C/V/A/Z** 这类编辑快捷键，只拦浏览器专属的。

import { openExternal } from './api'
import { installMenuDismiss, menuJustOpened, showContextMenu, type MenuEntry } from './contextMenu'

let installed = false

/** 只装一次（StrictMode 下 effect 会跑两遍） */
export function installNativeShell() {
  if (installed || typeof document === 'undefined') return
  installed = true
  installMenuDismiss()

  // ---- 1. 右键：一律不弹浏览器菜单 ----
  document.addEventListener(
    'contextmenu',
    (e) => {
      const t = e.target
      // 组件的 onContextMenu 在冒泡阶段已经先执行过了，这里不再重复弹
      if (menuJustOpened()) {
        e.preventDefault()
        return
      }
      e.preventDefault()
      if (isEditable(t)) {
        showContextMenu(editMenu(t), e.clientX, e.clientY)
      }
      // 非编辑区域且组件没处理：什么都不弹，比弹一个「刷新」干净
    },
    { capture: false },
  )

  // ---- 2. 快捷键：拦掉浏览器专属的，放行编辑类 ----
  document.addEventListener(
    'keydown',
    (e) => {
      const key = e.key.toLowerCase()
      const ctrl = e.ctrlKey || e.metaKey

      // Ctrl+K 唤起 AI 助手（这是功能键，不拦）
      if (ctrl && key === 'k') {
        e.preventDefault()
        window.dispatchEvent(new CustomEvent('dm:toggle-ai'))
        return
      }

      if (isBlockedKey(e, key, ctrl)) {
        e.preventDefault()
        e.stopPropagation()
      }
    },
    { capture: true },
  )

  // ---- 3. Ctrl+滚轮：不许缩放整个界面 ----
  document.addEventListener(
    'wheel',
    (e) => {
      if (e.ctrlKey) e.preventDefault()
    },
    { passive: false, capture: true },
  )

  // ---- 4. 拖放：不许把文件拖进来就导航走 ----
  // 真正的拦截在 tauri.conf.json 的 dragDropEnabled:false，这里是双保险（含拖拽 DOM 元素）
  document.addEventListener('dragstart', (e) => e.preventDefault())
  document.addEventListener('dragover', (e) => e.preventDefault())
  document.addEventListener('drop', (e) => e.preventDefault())

  // ---- 5. 外链交给系统浏览器 ----
  document.addEventListener('click', (e) => {
    const target = e.target
    if (!(target instanceof Element)) return
    const a = target.closest('a[href]')
    if (!a) return
    const href = a.getAttribute('href') ?? ''
    if (/^https?:\/\//i.test(href)) {
      e.preventDefault()
      void openExternal(href)
    }
  })
}

function isBlockedKey(e: KeyboardEvent, key: string, ctrl: boolean): boolean {
  if (key === 'f5' || key === 'f12' || key === 'f7') return true
  // Alt+左/右 是浏览器的前进后退
  if (e.altKey && (key === 'arrowleft' || key === 'arrowright')) return true
  if (!ctrl) return false
  if (key === 'r' || key === 'p' || key === 'u' || key === 's' || key === 'o') return true
  if (e.shiftKey && (key === 'i' || key === 'j' || key === 'c' || key === 'r' || key === 'p')) return true
  return false
}

function isEditable(t: EventTarget | null): t is HTMLElement {
  if (!(t instanceof HTMLElement)) return false
  const tag = t.tagName
  if (tag === 'TEXTAREA') return true
  if (tag === 'INPUT') {
    const type = (t as HTMLInputElement).type
    return !['button', 'checkbox', 'radio', 'submit', 'range', 'file', 'color'].includes(type)
  }
  return t.isContentEditable
}

function editMenu(t: HTMLElement): MenuEntry[] {
  const sel = window.getSelection()?.toString() ?? ''
  return [
    {
      type: 'item',
      label: '剪切',
      hint: 'Ctrl+X',
      disabled: !sel,
      onClick: () => document.execCommand('cut'),
    },
    {
      type: 'item',
      label: '复制',
      hint: 'Ctrl+C',
      disabled: !sel,
      onClick: () => document.execCommand('copy'),
    },
    { type: 'sep' },
    {
      type: 'item',
      label: '粘贴',
      hint: 'Ctrl+V',
      onClick: () => void pasteInto(t),
    },
    { type: 'sep' },
    {
      type: 'item',
      label: '全选',
      hint: 'Ctrl+A',
      onClick: () => {
        t.focus()
        document.execCommand('selectAll')
      },
    },
  ]
}

async function pasteInto(t: HTMLElement) {
  t.focus()
  try {
    const text = await navigator.clipboard.readText()
    if (!text) return
    document.execCommand('insertText', false, text)
  } catch {
    // WebView 内核不给读剪贴板时，不要静默失败 —— 用户会以为粘贴坏了
    toast('当前环境不允许直接读取剪贴板，请按 Ctrl+V 粘贴')
  }
}

/** 轻量提示（右下角浮一条，1.8 秒后消失），不打断操作 */
let toastTimer: number | undefined
function toast(text: string) {
  let el = document.getElementById('dm-toast')
  if (!el) {
    el = document.createElement('div')
    el.id = 'dm-toast'
    el.className = 'dm-toast'
    document.body.appendChild(el)
  }
  el.textContent = text
  el.classList.add('show')
  if (toastTimer) window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => el?.classList.remove('show'), 1800)
}
