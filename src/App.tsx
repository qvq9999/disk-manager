import { useEffect, useRef, useState } from 'react'
import { Cpu, HardDrive, Trash2, Package, Sun, Moon, AlertTriangle, Menu, Settings as SettingsIcon } from 'lucide-react'
import type { Page, Volume } from './types'
import { getVolumes, getAppSettings, acceptDisclaimer, peekVolumes, warmUp, isElevated, restartAsAdmin, onCloseRequested } from './services/api'
import { setPrefs } from './services/prefs'
import { installNativeShell } from './services/native'
import { checkForUpdate, subscribeUpdate, latestUpdate } from './services/update'
import { ErrorBoundary } from './components/ErrorBoundary'
import { CloseConfirm } from './components/CloseConfirm'
import { DisclaimerModal } from './components/Disclaimer'
import { AiAssistant } from './components/AiAssistant'
import { fmtBytes } from './utils/format'
import { Hardware } from './pages/Hardware'
import { Overview } from './pages/Overview'
import { DriveDetail } from './pages/DriveDetail'
import { Cleanup } from './pages/Cleanup'
import { Uninstall } from './pages/Uninstall'
import { Settings } from './pages/Settings'
import { AiSettingsPage } from './components/AiSettings'

const NAV: { id: Page; label: string; icon: typeof HardDrive }[] = [
  { id: 'hardware', label: '硬件信息', icon: Cpu },
  { id: 'overview', label: '磁盘总览', icon: HardDrive },
  { id: 'clean', label: '垃圾清理', icon: Trash2 },
  { id: 'uninstall', label: '软件卸载', icon: Package },
  { id: 'settings', label: '设置', icon: SettingsIcon },
]

const TITLES: Record<Page, string> = {
  hardware: '硬件信息',
  overview: '磁盘总览',
  drive: '空间分析',
  clean: '垃圾清理',
  uninstall: '软件卸载',
  settings: '设置',
  ai: 'AI 助手',
}

export default function App() {
  const [page, setPage] = useState<Page>('hardware')
  /** 主内容滚动容器：切页后要回到顶部 */
  const contentRef = useRef<HTMLDivElement>(null)
  const [volumes, setVolumes] = useState<Volume[]>(() => peekVolumes() ?? [])
  const [drive, setDrive] = useState<Volume | null>(null)
  const [dark, setDark] = useState(false)
  const [fatal, setFatal] = useState<string | null>(null)
  const [admin, setAdmin] = useState(false)
  // 窄屏抽屉侧栏开关（宽屏下 CSS 忽略该状态，侧栏恒显示）
  const [menuOpen, setMenuOpen] = useState(false)
  // 点窗口 ✕ 时：Rust 已拦下关闭，由这里弹窗让用户选择「退到后台 / 直接关闭」
  const [closePrompt, setClosePrompt] = useState(false)
  // 免责声明：first-run 为首次启动自动弹出，review 为用户从设置里主动打开
  const [disc, setDisc] = useState<'first-run' | 'review' | null>(null)

  useEffect(() => {
    return onCloseRequested(() => setClosePrompt(true))
  }, [])

  // 接管 WebView 的浏览器行为（右键菜单/刷新/缩放/拖放/外链），让它看起来是个原生程序。
  // 必须在首帧后立刻装好：晚一步用户就可能已经右键看到「刷新」了。
  useEffect(() => {
    installNativeShell()
  }, [])

  // 自动检查更新：延迟 3 秒，不拖慢首屏，也避开开机时与其他程序抢网络。
  // 只在侧边栏「设置」上打一个提示点，不弹窗打扰。
  const [updateReady, setUpdateReady] = useState(() => !!latestUpdate()?.available)
  useEffect(() => {
    const un = subscribeUpdate((r) => setUpdateReady(!!r?.available))
    const t = window.setTimeout(() => {
      void checkForUpdate().then((r) => setUpdateReady(!!r?.available))
    }, 3000)
    return () => {
      un()
      window.clearTimeout(t)
    }
  }, [])

  // 切页回到顶部。
  //
  // 之前换页只替换子节点、不重置滚动位置：从「设置」滚到底再点进 AI 配置，
  // 打开就停在中间那一截，看起来像"内容缺失"。下钻（空间分析进目录）不是切页，
  // 不经过这里，那层自己的滚动位置不受影响。
  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 })
  }, [page])

  // 启动预热：空闲时后台预取硬件信息/盘符，用户切页时通常已就绪；同时兜底捕获异常
  useEffect(() => {
    warmUp()
    const onErr = (e: ErrorEvent) => setFatal(e.message || '未知脚本错误')
    const onRej = (e: PromiseRejectionEvent) => {
      const msg = String(e.reason ?? '')
      if (msg && !msg.includes('invoke')) setFatal(msg)
    }
    window.addEventListener('error', onErr)
    window.addEventListener('unhandledrejection', onRej)
    return () => {
      window.removeEventListener('error', onErr)
      window.removeEventListener('unhandledrejection', onRej)
    }
  }, [])

  useEffect(() => {
    getVolumes().then(setVolumes).catch(() => setVolumes([]))
    isElevated().then(setAdmin).catch(() => setAdmin(false))
    // AI 相关开关要在首屏就能被读到（空间分析会自动分析、AI 助手会读页面数据）
    getAppSettings()
      .then((s) => {
        setPrefs({ autoAnalyzeLayer: s.autoAnalyzeLayer, allowPageContext: s.allowPageContext })
        // 首次安装后的第一次进入才自动弹一次；确认状态由 Rust 侧持久化
        if (!s.disclaimerAccepted) setDisc('first-run')
      })
      .catch(() => { /* 读不到就用默认值，且不拦截使用 */ })
  }, [])

  const elevate = async () => {
    try {
      await restartAsAdmin()
      alert('已请求管理员权限：请在系统弹窗中确认。\n确认后会启动新的管理员实例，请关闭当前窗口使用。')
    } catch {
      // 用户取消 UAC
    }
  }

  useEffect(() => {
    if (!drive && volumes.length > 0) setDrive(volumes[0])
  }, [volumes, drive])

  const setTheme = (d: boolean) => {
    setDark(d)
    document.documentElement.dataset.theme = d ? 'dark' : 'light'
  }

  const openDrive = (v: Volume) => {
    setDrive(v)
    setPage('drive')
  }

  // 切页时关闭抽屉（窄屏体验）
  const goto = (id: Page) => {
    setPage(id)
    setMenuOpen(false)
  }

  const body = (() => {
    switch (page) {
      case 'hardware': return <Hardware />
      case 'overview': return <Overview volumes={volumes} onOpenDrive={openDrive} />
      case 'drive': return drive ? <DriveDetail volume={drive} onBack={() => setPage('overview')} onGotoClean={() => setPage('clean')} /> : null
      case 'clean': return <Cleanup />
      case 'uninstall': return <Uninstall />
      case 'settings': return <Settings onOpenAi={() => setPage('ai')} onOpenDisclaimer={() => setDisc('review')} />
      case 'ai': return <AiSettingsPage onBack={() => setPage('settings')} />
    }
  })()

  return (
    <div className="app-shell">
      {/* 窄屏抽屉蒙层：点击关闭 */}
      {menuOpen && <div className="sidebar-mask" onClick={() => setMenuOpen(false)} />}
      <aside className={`sidebar${menuOpen ? ' open' : ''}`}>
        <div className="brand">
          <div className="brand-icon"><HardDrive size={17} /></div>
          <div className="brand-text">
            <div className="brand-name">存储管理</div>
            <div className="brand-sub">Disk Manager v{__APP_VERSION__}</div>
          </div>
        </div>
        {NAV.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            className={`nav-item${page === id || (id === 'settings' && page === 'ai') ? ' active' : ''}`}
            onClick={() => goto(id)}
            title={label}
          >
            <Icon size={17} /> <span className="nav-label">{label}</span>
            {id === 'settings' && updateReady && (
              <span
                title="有新版本可用"
                style={{
                  marginLeft: 'auto', width: 7, height: 7, borderRadius: '50%',
                  background: 'var(--accent)', flexShrink: 0,
                }}
              />
            )}
          </button>
        ))}
        <div className="sidebar-foot">
          <div className="perm-box" title={admin
            ? '当前以管理员权限运行：可读取全部系统目录，扫描结果最完整'
            : '当前以标准用户权限运行：Windows 等系统目录不可完整读取，扫描结果可能偏小；点击右侧按钮提权可获得完整结果'}>
            <span className={`perm-dot${admin ? ' ok' : ''}`} />
            <div>
              <div className="perm-title">{admin ? '管理员权限' : '标准权限'}</div>
              <div className="perm-desc">
                {admin ? '可读取全部系统目录，扫描最完整' : '部分系统目录不可读，扫描结果可能偏小'}
              </div>
            </div>
          </div>
          {!admin && (
            <button
              className="btn btn-ghost"
              style={{ padding: '2px 8px', fontSize: 11, flexShrink: 0 }}
              title="以管理员身份重新启动（会弹出系统 UAC 确认）"
              onClick={elevate}
            >
              提权
            </button>
          )}
        </div>
      </aside>

      <main className="main">
        <div className="topbar">
          {/* 窄屏抽屉切换；宽屏由 CSS 隐藏 */}
          <button
            className="icon-btn sidebar-toggle"
            onClick={() => setMenuOpen((s) => !s)}
            aria-label="打开导航菜单"
            title="导航菜单"
          >
            <Menu size={18} />
          </button>
          <span className="topbar-title">{TITLES[page]}</span>
          <span className="topbar-spacer" />
          <button
            className="icon-btn"
            onClick={() => setTheme(!dark)}
            aria-label={dark ? '切换到浅色模式' : '切换到深色模式'}
            title={dark ? '浅色模式' : '深色模式'}
          >
            {dark ? <Sun size={17} /> : <Moon size={17} />}
          </button>
        </div>
        <div className="content" ref={contentRef} style={{ display: 'flex', flexDirection: 'column' }}>
          {fatal && (
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12, padding: '8px 12px', borderRadius: 8, background: 'rgba(220,38,38,.08)', border: '1px solid rgba(220,38,38,.25)', fontSize: 12.5 }}>
              <AlertTriangle size={14} color="var(--danger)" />
              <span style={{ flex: 1, wordBreak: 'break-all' }}>检测到脚本异常（已隔离，不影响其他功能）：{fatal}</span>
              <button className="btn btn-ghost" style={{ padding: '3px 10px', fontSize: 12 }} onClick={() => setFatal(null)}>关闭</button>
            </div>
          )}
          <ErrorBoundary key={page}>{body}</ErrorBoundary>
        </div>
      </main>

      {closePrompt && <CloseConfirm onCancel={() => setClosePrompt(false)} />}

      {/* 免责声明：首次启动自动弹出（必须读到底部），之后只能从设置里主动查看 */}
      {disc && (
        <DisclaimerModal
          mode={disc}
          onClose={() => setDisc(null)}
          onAccept={disc === 'first-run' ? async () => { await acceptDisclaimer() } : undefined}
        />
      )}

      {/* 全局 AI 助手：不占导航位，Ctrl+K 或点右下角圆球唤起 */}
      <AiAssistant
        page={page}
        pageTitle={TITLES[page]}
        admin={admin}
        diskContext={
          volumes.length > 0
            ? `磁盘：${volumes
                .map((v) => `${v.letter}: 可用 ${fmtBytes(v.freeBytes, 1)} / 共 ${fmtBytes(v.totalBytes, 1)}`)
                .join('；')}`
            : ''
        }
        onGotoSettings={() => setPage('settings')}
      />
    </div>
  )
}
