// ============ 原生感右键菜单 ============
//
// 为什么不用 WebView 自带的菜单：它带「刷新 / 另存为 / 打印 / 检查」这些浏览器项，
// 一眼就露馅。这里用纯 DOM 画一个，样式走应用的 CSS 变量，跟着主题一起变。

export type MenuEntry =
  | { type: 'sep' }
  | {
      type: 'item'
      label: string
      /** 危险操作：文字用 danger 色 */
      danger?: boolean
      disabled?: boolean
      /** 右侧快捷键提示，如 Ctrl+C */
      hint?: string
      onClick: () => void
    }

let openEl: HTMLElement | null = null
/** 最近一次弹出菜单的时间：文档级 contextmenu 兜底时据此判断「组件已经处理过」 */
let lastOpenAt = 0

export function menuJustOpened(withinMs = 120): boolean {
  return Date.now() - lastOpenAt < withinMs
}

export function closeContextMenu() {
  if (!openEl) return
  openEl.remove()
  openEl = null
}

/** 在 (x, y) 处弹出菜单；超出窗口会自动翻转，永不出现窗口外的半截菜单 */
export function showContextMenu(entries: MenuEntry[], x: number, y: number) {
  closeContextMenu()
  if (entries.length === 0) return

  const el = document.createElement('div')
  el.className = 'ctx-menu'
  el.setAttribute('role', 'menu')

  for (const e of entries) {
    if (e.type === 'sep') {
      const sep = document.createElement('div')
      sep.className = 'ctx-sep'
      el.appendChild(sep)
      continue
    }
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.className = 'ctx-item' + (e.danger ? ' danger' : '')
    btn.setAttribute('role', 'menuitem')
    btn.disabled = !!e.disabled

    const label = document.createElement('span')
    label.className = 'ctx-label'
    label.textContent = e.label
    btn.appendChild(label)

    if (e.hint) {
      const hint = document.createElement('span')
      hint.className = 'ctx-hint'
      hint.textContent = e.hint
      btn.appendChild(hint)
    }

    btn.addEventListener('click', (ev) => {
      ev.stopPropagation()
      if (e.disabled) return
      closeContextMenu()
      e.onClick()
    })
    el.appendChild(btn)
  }

  // 先挂在 body 上量尺寸，再决定位置（避免先闪一下再跳）
  el.style.visibility = 'hidden'
  document.body.appendChild(el)
  const r = el.getBoundingClientRect()
  const pad = 6
  const left = Math.max(pad, Math.min(x, window.innerWidth - r.width - pad))
  const top = Math.max(pad, Math.min(y, window.innerHeight - r.height - pad))
  el.style.left = `${left}px`
  el.style.top = `${top}px`
  el.style.visibility = 'visible'

  openEl = el
  lastOpenAt = Date.now()
}

/** 全局关闭：点别处 / 滚轮 / Esc / 失焦 / 窗口尺寸变化 */
export function installMenuDismiss() {
  const onDown = (e: MouseEvent) => {
    if (openEl && e.target instanceof Node && openEl.contains(e.target)) return
    closeContextMenu()
  }
  document.addEventListener('mousedown', onDown, true)
  document.addEventListener('wheel', onDown, { capture: true, passive: true })
  window.addEventListener('blur', closeContextMenu)
  window.addEventListener('resize', closeContextMenu)
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeContextMenu()
  })
}
