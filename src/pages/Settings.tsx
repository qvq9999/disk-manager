import { useEffect, useState } from 'react'
import {
  HardDrive, Power, LayoutGrid, Trash2, Package, Cpu, Check, AlertTriangle,
  Minimize2, LogOut, HelpCircle, FolderCog, RefreshCw, Loader2, Download,
  ChevronDown, ChevronUp,
} from 'lucide-react'
import type { AppSettings, CloseAction } from '../types'
import { getAppSettings, updateAutostart, updateCloseAction } from '../services/api'
import { CHANGELOG } from '../services/changelog'
import {
  checkForUpdate, installPendingUpdate, latestUpdate, subscribeUpdate,
  getUpdateEndpoint, type UpdateCheckResult, type DownloadProgress,
} from '../services/update'

/** 开关控件（无原生 checkbox，纯 CSS 过渡，与整体视觉一致） */
function Switch({ checked, disabled, onChange, label }: {
  checked: boolean
  disabled?: boolean
  onChange: (v: boolean) => void
  label: string
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        width: 42, height: 24, borderRadius: 9999, padding: 2, border: 'none',
        display: 'flex', alignItems: 'center',
        justifyContent: checked ? 'flex-end' : 'flex-start',
        background: checked ? 'var(--accent)' : 'var(--border)',
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.5 : 1,
        transition: 'background var(--t-fast)',
        flexShrink: 0,
      }}
    >
      <span style={{
        width: 20, height: 20, borderRadius: '50%', background: '#fff',
        boxShadow: '0 1px 3px rgba(0,0,0,.22)', display: 'block',
      }} />
    </button>
  )
}

/** 设置行容器：左侧标题 + 说明，右侧控件 */
function Row({ title, desc, children, last }: {
  title: string
  desc?: React.ReactNode
  children: React.ReactNode
  last?: boolean
}) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 16,
      padding: '15px 18px',
      borderBottom: last ? 'none' : '1px solid var(--border-soft)',
    }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 13.5, fontWeight: 600 }}>{title}</div>
        {desc && <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 4, lineHeight: 1.65 }}>{desc}</div>}
      </div>
      {children}
    </div>
  )
}

const CLOSE_OPTIONS: { value: CloseAction; icon: typeof HelpCircle; title: string; desc: string }[] = [
  {
    value: 'ask',
    icon: HelpCircle,
    title: '每次询问',
    desc: '点 ✕ 时弹出选择框，由你当场决定',
  },
  {
    value: 'background',
    icon: Minimize2,
    title: '退到后台',
    desc: '隐藏窗口继续运行，后台扫描不中断；从托盘图标可重新打开',
  },
  {
    value: 'exit',
    icon: LogOut,
    title: '直接关闭',
    desc: '立即退出程序，未完成的扫描与清理会中断',
  },
]

const FEATURES = [
  { icon: LayoutGrid, name: '磁盘总览', desc: '分区容量与物理硬盘健康、SMART 寿命一览' },
  { icon: FolderCog, name: '空间分析', desc: '目录树 + 矩形树图，定位占空间的大户' },
  { icon: Trash2, name: '垃圾清理', desc: '按规则扫描可清理项，支持按盘符筛选' },
  { icon: Package, name: '软件卸载', desc: '卸载后扫描残留（注册表 / 服务 / 计划任务）' },
]

export function Settings() {
  const [s, setS] = useState<AppSettings | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [autostartBusy, setAutostartBusy] = useState(false)
  const [actionBusy, setActionBusy] = useState<CloseAction | null>(null)
  const [notice, setNotice] = useState<{ tone: 'ok' | 'bad'; text: string } | null>(null)

  // ---- 软件更新 ----
  const [upd, setUpd] = useState<UpdateCheckResult | null>(() => latestUpdate())
  const [updChecking, setUpdChecking] = useState(false)
  const [endpoint, setEndpoint] = useState('')
  const [installing, setInstalling] = useState(false)
  const [dl, setDl] = useState<DownloadProgress | null>(null)
  const [installErr, setInstallErr] = useState('')
  /** 更新日志是否展开（默认收起，设置页已经够长） */
  const [showLog, setShowLog] = useState(false)

  useEffect(() => {
    let alive = true
    getUpdateEndpoint().then((e) => { if (alive) setEndpoint(e) })
    // 启动时已检查过的话，这里直接复用；否则补一次静默检查
    const un = subscribeUpdate((r) => { if (alive) setUpd(r) })
    if (!latestUpdate()) {
      // 订阅之外再显式接一次返回值：订阅是异步广播，这里能保证首帧之后立刻有状态
      void checkForUpdate().then((r) => { if (alive) setUpd(r) })
    }
    return () => { alive = false; un() }
  }, [])

  const runCheckUpdate = async () => {
    if (updChecking) return
    setUpdChecking(true)
    setInstallErr('')
    setDl(null)
    try {
      setUpd(await checkForUpdate(true))
    } finally {
      setUpdChecking(false)
    }
  }

  const runInstall = async () => {
    if (installing) return
    setInstalling(true)
    setInstallErr('')
    try {
      await installPendingUpdate(setDl)
      // 走完 downloadAndInstall 说明安装器已启动并接管，本进程即将被替换
      setNotice({ tone: 'ok', text: '安装包已下载并启动安装，程序将自动重启到新版本' })
    } catch (e) {
      setInstallErr(String((e as { message?: string })?.message ?? e))
    } finally {
      setInstalling(false)
    }
  }

  useEffect(() => {
    let alive = true
    getAppSettings()
      .then((v) => { if (alive) setS(v) })
      .catch((e) => { if (alive) setLoadError(String((e as { message?: string })?.message ?? e)) })
    return () => { alive = false }
  }, [])

  const toggleAutostart = async (next: boolean) => {
    if (!s || autostartBusy) return
    setAutostartBusy(true)
    setNotice(null)
    try {
      // Rust 会回读注册表返回真实状态：不能假设写入一定成功
      const real = await updateAutostart(next)
      setS({ ...s, autostart: real })
      setNotice(real === next
        ? { tone: 'ok', text: next ? '已开启开机自启动' : '已关闭开机自启动' }
        : { tone: 'bad', text: '设置未生效，请检查系统权限后重试' })
    } catch (e) {
      setNotice({ tone: 'bad', text: String((e as { message?: string })?.message ?? e) })
    } finally {
      setAutostartBusy(false)
    }
  }

  const pickAction = async (action: CloseAction) => {
    if (!s || actionBusy) return
    setActionBusy(action)
    setNotice(null)
    try {
      await updateCloseAction(action)
      setS({ ...s, closeAction: action })
    } catch (e) {
      setNotice({ tone: 'bad', text: String((e as { message?: string })?.message ?? e) })
    } finally {
      setActionBusy(null)
    }
  }

  if (loadError) {
    return (
      <div className="page-enter" style={{ color: 'var(--danger)', padding: 32 }}>
        读取设置失败：{loadError}
      </div>
    )
  }
  if (!s) {
    return <div className="page-enter status-line" style={{ padding: 48, justifyContent: 'center' }}>正在读取设置…</div>
  }

  const devPath = /\\target\\(debug|release)\\/i.test(s.exePath)

  return (
    <div className="page-enter" style={{ maxWidth: 880 }}>
      {/* ---------- 应用介绍 ---------- */}
      <div className="panel" style={{ padding: '20px 22px', marginBottom: 20 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
          <div className="quick-icon" style={{ width: 46, height: 46, background: 'var(--accent-soft)', color: 'var(--accent)' }}>
            <HardDrive size={22} />
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 16, fontWeight: 600, letterSpacing: '-0.01em' }}>存储管理 Disk Manager</div>
            <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 3 }}>
              版本 {s.appVersion} · 适用于 Windows 10 / 11（需管理员权限以读取完整磁盘信息）
            </div>
          </div>
        </div>

        <p style={{ fontSize: 12.5, lineHeight: 1.85, color: 'var(--muted)', margin: '14px 0 0' }}>
          一款面向本机的存储管理工具，把「谁在占空间」「什么可以删」「怎么安全卸载」三件事放在一处。
          全部数据在本机读取，不上传任何信息；清理与卸载前均可预览明细，危险项会单独标注。
        </p>

        <div style={{
          display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))',
          gap: 10, marginTop: 16,
        }}>
          {FEATURES.map(({ icon: Icon, name, desc }) => (
            <div key={name} style={{
              display: 'flex', gap: 9, alignItems: 'flex-start',
              padding: '10px 12px', borderRadius: 'var(--radius-md)',
              background: 'var(--surface-warm)', border: '1px solid var(--border-soft)',
            }}>
              <Icon size={15} color="var(--accent)" style={{ flexShrink: 0, marginTop: 1 }} />
              <div style={{ minWidth: 0 }}>
                <div style={{ fontSize: 12.5, fontWeight: 600 }}>{name}</div>
                <div style={{ fontSize: 11, color: 'var(--meta)', marginTop: 2, lineHeight: 1.55 }}>{desc}</div>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* ---------- 常规设置 ---------- */}
      <div className="section-label">常规</div>
      <div className="panel" style={{ marginBottom: 20 }}>
        <Row
          title="开机自启动"
          desc={
            <>
              登录 Windows 后自动启动本程序。本程序需要管理员权限，自启动时会请求一次 UAC 确认。
              {devPath && (
                <span style={{ color: 'var(--warn)' }}>
                  {' '}注意：当前运行的是开发/免安装副本，自启动将指向该文件路径。
                </span>
              )}
            </>
          }
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            {autostartBusy && <span className="loading-spinner small" />}
            <Switch
              checked={s.autostart}
              disabled={autostartBusy}
              onChange={toggleAutostart}
              label="开机自启动"
            />
          </div>
        </Row>

        <div style={{ padding: '15px 18px' }}>
          <div style={{ fontSize: 13.5, fontWeight: 600 }}>点 ✕ 关闭窗口时</div>
          <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 4, lineHeight: 1.65 }}>
            每次关闭时也可以在弹窗里勾选「记住这次选择」，效果等同于在这里选定。
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 12 }}>
            {CLOSE_OPTIONS.map(({ value, icon: Icon, title, desc }) => {
              const selected = s.closeAction === value
              const disabled = value === 'background' && !s.trayAvailable
              const busy = actionBusy === value
              return (
                <button
                  key={value}
                  onClick={() => pickAction(value)}
                  disabled={disabled || !!actionBusy}
                  style={{
                    display: 'flex', alignItems: 'flex-start', gap: 12, width: '100%',
                    padding: '11px 14px', textAlign: 'left',
                    borderRadius: 'var(--radius-md)',
                    border: `1px solid ${selected ? 'var(--accent)' : 'var(--border-soft)'}`,
                    background: selected ? 'var(--accent-soft)' : 'var(--surface)',
                    opacity: disabled ? 0.45 : 1,
                    cursor: disabled ? 'not-allowed' : 'pointer',
                    transition: 'border-color var(--t-fast), background var(--t-fast)',
                  }}
                >
                  <span style={{ color: selected ? 'var(--accent)' : 'var(--meta)', flexShrink: 0, marginTop: 1 }}>
                    <Icon size={17} />
                  </span>
                  <span style={{ flex: 1, minWidth: 0 }}>
                    <span style={{
                      display: 'flex', alignItems: 'center', gap: 7,
                      fontSize: 13, fontWeight: 600, color: selected ? 'var(--accent)' : 'var(--fg)',
                    }}>
                      {title}
                      {selected && <Check size={13} />}
                    </span>
                    <span style={{ display: 'block', fontSize: 11.5, color: 'var(--meta)', marginTop: 3, lineHeight: 1.6 }}>
                      {value === 'background' && !s.trayAvailable
                        ? '当前无法创建托盘图标，该选项不可用'
                        : desc}
                    </span>
                  </span>
                  {busy && <span className="loading-spinner small" style={{ marginTop: 3 }} />}
                </button>
              )
            })}
          </div>

          {s.closeAction === 'background' && (
            <div style={{
              display: 'flex', gap: 8, alignItems: 'flex-start', marginTop: 12,
              padding: '9px 12px', borderRadius: 'var(--radius-md)',
              background: 'var(--surface-warm)', border: '1px solid var(--border-soft)',
              fontSize: 11.5, color: 'var(--muted)', lineHeight: 1.7,
            }}>
              <Power size={13} style={{ flexShrink: 0, marginTop: 1 }} />
              <span>
                退到后台后窗口会从任务栏消失，请在系统托盘（屏幕右下角，可能需点开「^」）右击图标选择
                「显示主界面」或「退出程序」。再次从开始菜单启动本程序也会直接把已有窗口调回前台。
              </span>
            </div>
          )}
        </div>
      </div>

      {notice && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: 8, marginBottom: 16,
          padding: '9px 13px', borderRadius: 'var(--radius-md)', fontSize: 12.5,
          background: notice.tone === 'ok' ? 'rgba(22,163,74,.08)' : 'rgba(217,119,6,.08)',
          border: `1px solid ${notice.tone === 'ok' ? 'rgba(22,163,74,.25)' : 'rgba(217,119,6,.25)'}`,
          color: notice.tone === 'ok' ? 'var(--success)' : 'var(--warn)',
        }}>
          {notice.tone === 'ok' ? <Check size={14} /> : <AlertTriangle size={14} />}
          {notice.text}
        </div>
      )}

      {/* ---------- 软件更新 ---------- */}
      <div className="section-label">软件更新</div>
      <div className="panel">
        <Row
          title="当前版本"
          desc={
            <>
              版本 {s.appVersion} · 检查更新会请求 GitHub Release 上的版本清单，
              并用内置公钥校验安装包签名，签名不符的包会被拒绝安装。
            </>
          }
        >
          <button
            className="btn btn-ghost"
            style={{ padding: '6px 12px', fontSize: 12.5 }}
            onClick={runCheckUpdate}
            disabled={updChecking || installing}
          >
            {updChecking
              ? <><Loader2 size={14} className="spin" /> 检查中…</>
              : <><RefreshCw size={14} /> 检查更新</>}
          </button>
        </Row>

        <Row title="更新源" desc={<span className="mono" style={{ wordBreak: 'break-all' }}>{endpoint || '读取中…'}</span>}>
          <span />
        </Row>

        <Row title="自动检查" desc="每次启动后自动检查一次；发现新版本时侧边栏「设置」会出现提示点，不会弹出打扰。" last>
          <span className="mono" style={{ fontSize: 12, color: 'var(--muted)' }}>已开启</span>
        </Row>

        {/* 状态反馈：四种情况互斥，避免「既是错误又是最新」这类矛盾呈现 */}
        <div style={{ padding: '12px 14px', borderTop: '1px solid var(--border)' }}>
          {installErr ? (
            <div className="status-line" style={{ color: 'var(--warn)', lineHeight: 1.7, alignItems: 'flex-start' }}>
              <AlertTriangle size={14} style={{ flexShrink: 0, marginTop: 3 }} />
              <span>安装失败：{installErr}</span>
            </div>
          ) : installing ? (
            <div>
              <div className="status-line" style={{ marginBottom: 8 }}>
                <Loader2 size={14} className="spin" />
                {dl ? `正在下载安装包 ${dl.pct}%（${(dl.downloaded / 1024 / 1024).toFixed(1)} MB）` : '正在准备下载…'}
              </div>
              <div className="progress-track">
                <div className="progress-fill" style={{ width: `${Math.max(2, dl?.pct ?? 0)}%` }} />
              </div>
              <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 6, lineHeight: 1.7 }}>
                下载完成后会自动运行安装程序，中途请勿关闭本窗口。
              </div>
            </div>
          ) : upd?.available ? (
            <div>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                <Download size={15} color="var(--accent)" />
                <span style={{ fontSize: 13, fontWeight: 600 }}>
                  发现新版本 {upd.latest}
                  {upd.date && <span style={{ fontWeight: 400, color: 'var(--muted)', fontSize: 12 }}> · {upd.date.slice(0, 10)}</span>}
                </span>
              </div>
              {upd.notes && (
                <div style={{
                  fontSize: 12.5, lineHeight: 1.75, color: 'var(--fg-2)', whiteSpace: 'pre-wrap',
                  maxHeight: 140, overflowY: 'auto', marginBottom: 10, padding: '8px 10px',
                  background: 'var(--surface-2, rgba(0,0,0,.03))', borderRadius: 8,
                }}>
                  {upd.notes}
                </div>
              )}
              <button className="btn btn-primary" style={{ padding: '7px 14px', fontSize: 12.5 }} onClick={runInstall}>
                <Download size={14} /> 下载并安装
              </button>
            </div>
          ) : upd?.error ? (
            <div className="status-line" style={{ color: 'var(--warn)', lineHeight: 1.7, alignItems: 'flex-start' }}>
              <AlertTriangle size={14} style={{ flexShrink: 0, marginTop: 3 }} />
              <span>{upd.error}</span>
            </div>
          ) : upd ? (
            <div className="status-line" style={{ color: 'var(--success)' }}>
              <Check size={14} /> 已是最新版本（{upd.current}）
            </div>
          ) : (
            <div className="status-line">
              <HelpCircle size={14} /> 尚未检查
            </div>
          )}
        </div>

        {/*
          更新日志：内容来自仓库根的 CHANGELOG.md，构建时内联进包，离线可看。
          默认折叠 —— 设置页已经很长，日志只在用户主动想看时展开。
        */}
        <div style={{ borderTop: '1px solid var(--border)' }}>
          <button
            type="button"
            onClick={() => setShowLog((v) => !v)}
            style={{
              width: '100%', display: 'flex', alignItems: 'center', gap: 8,
              padding: '11px 14px', background: 'transparent', border: 'none',
              color: 'var(--fg-2)', fontSize: 12.5, cursor: 'pointer', textAlign: 'left',
            }}
            aria-expanded={showLog}
          >
            {showLog ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
            更新日志
            <span style={{ marginLeft: 'auto', fontSize: 11.5, color: 'var(--meta)' }}>
              {CHANGELOG.length} 个版本
            </span>
          </button>

          {showLog && (
            <div style={{ padding: '2px 16px 16px', maxHeight: 340, overflowY: 'auto' }}>
              {CHANGELOG.length === 0 ? (
                <div style={{ fontSize: 12.5, color: 'var(--meta)' }}>暂无可显示的更新记录</div>
              ) : (
                CHANGELOG.map((e) => {
                  const isCurrent = !!s && e.version.replace(/^v/i, '') === s.appVersion.replace(/^v/i, '')
                  return (
                    <div key={e.version} style={{ marginBottom: 16 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6, flexWrap: 'wrap' }}>
                        <span className='mono' style={{ fontSize: 12.5, fontWeight: 600 }}>v{e.version}</span>
                        {isCurrent && <span className='tag tag-safe'>当前版本</span>}
                        {e.date && <span style={{ fontSize: 11.5, color: 'var(--meta)' }}>{e.date}</span>}
                      </div>
                      {e.groups.map((g, gi) => (
                        <div key={gi} style={{ marginBottom: 7 }}>
                          {g.title && (
                            <div style={{ fontSize: 11.5, color: 'var(--muted)', marginBottom: 2 }}>{g.title}</div>
                          )}
                          <ul style={{ margin: 0, paddingLeft: 17, fontSize: 12.5, lineHeight: 1.8, color: 'var(--fg-2)' }}>
                            {g.items.map((it, i) => <li key={i}>{it}</li>)}
                          </ul>
                        </div>
                      ))}
                    </div>
                  )
                })
              )}
            </div>
          )}
        </div>
      </div>

      {/* ---------- 程序信息 ---------- */}
      <div className="section-label">程序信息</div>
      <div className="panel">
        <Row title="程序位置" desc={<span className="mono" style={{ wordBreak: 'break-all' }}>{s.exePath || '未知'}</span>}>
          <span />
        </Row>
        <Row title="数据与隐私" desc="所有磁盘、注册表与软件信息均在本机读取，不上传、不联网请求。" last>
          <span />
        </Row>
      </div>

      <div style={{ fontSize: 11, color: 'var(--meta)', marginTop: 14, lineHeight: 1.8 }}>
        设置会保存在本机配置目录，重启后保持。
        开机自启动通过当前用户注册表 <span className="mono">Run</span> 项实现，可在任务管理器「启动」中一并管理。
      </div>
    </div>
  )
}
