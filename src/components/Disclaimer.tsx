// ============ 首次启动免责声明 ============
//
// 三个约束来自产品决定，不是随手加的：
// 1. **只在首次进入时自动弹**：确认状态存在 Rust 侧 settings.json，
//    清理浏览器数据不会让它反复弹出，卸载重装才会。
// 2. **必须读到底部才能点同意**：这类声明最容易变成「一路回车」，
//    滚到底才解锁是最低成本的阅读确认。内容不足一屏时直接放行，
//    否则用户永远滚不到「底部」，反而卡死。
// 3. **设置里能随时再看**：已同意之后从「设置 - 关于」进入，
//    这时不再强制滚动 —— 用户是主动来查的，不是被拦路。

import { useCallback, useEffect, useRef, useState } from 'react'
import { ShieldCheck, ChevronDown, X, Loader2 } from 'lucide-react'

/** 声明正文。措辞与界面其余文案保持一致：短句、第二人称、不堆法律术语。 */
const SECTIONS: { title: string; body: string[] }[] = [
  {
    title: '一、适用范围',
    body: [
      '本声明适用于「存储管理 Disk Manager」（下称本程序）的全部功能，包括磁盘总览、空间分析、垃圾清理、软件卸载、安装包与空文件夹清理，以及 AI 助手。',
    ],
  },
  {
    title: '二、运行方式与权限',
    body: [
      '本程序以管理员权限运行，用于读取分区、物理硬盘、注册表与系统目录的完整信息。若不以管理员权限运行，部分目录将无法读取，分析结果会不完整。',
      '除检查更新与你主动使用的 AI 功能外，本程序不发起网络请求。磁盘信息、硬件信息与已安装软件列表均在本机读取，不上传。',
    ],
  },
  {
    title: '三、清理与删除的风险',
    body: [
      '本程序执行的清理、卸载与删除操作不可撤销，文件不会进入回收站。程序不会、也无法为你找回已删除的内容。',
      '扫描结果依据目录特征、文件属性与内置规则判断，部分场景会结合模型输出给出建议。这类判断存在误判的可能：某些项目可能被软件依赖，删除后需要重新下载或重新生成缓存。',
      '请在确认前逐个核对目标路径与用途，对重要数据提前备份。程序只提供判断与执行工具，最终保留哪些、删除哪些由你决定。',
    ],
  },
  {
    title: '四、体积数字的口径',
    body: [
      '空间分析显示的体积为文件的逻辑大小。硬链接、压缩文件、稀疏文件与系统保留区域的处理方式不同，因此本程序的统计结果可能与资源管理器、系统「存储设置」或其他工具存在差异。该数字用于定位占用大户，不作为精确计量依据。',
    ],
  },
  {
    title: '五、AI 功能',
    body: [
      'AI 输出由第三方模型生成，不保证准确、完整，也不保证适用于你的具体环境，仅供参考，不构成任何专业建议。是否采纳、是否据此删除文件，由你自行判断。',
      '使用内置额度时，服务受提供方限流与额度策略约束，可能随时调整或中断；切换为你自己的 API Key 后，费用、配额与服务条款由你与服务商约定，与本程序无关。',
      '无论使用哪种方式，本程序只发送路径、体积、扩展名分布、硬件配置这类摘要，不读取、不上传任何文件内容。关闭 AI 功能后，本地识别与清理规则照常可用。',
    ],
  },
  {
    title: '六、卸载与系统更改',
    body: [
      '「软件卸载」调用的是软件自带的卸载程序，本程序不代替它删除程序文件。卸载后的残留扫描同样需要你逐项确认。',
      '系统目录、受保护目录与部分配置数据被刻意排除在清理范围之外。若你通过本程序之外的方式手动删除系统文件，由此产生的结果由你承担。',
    ],
  },
  {
    title: '七、版本更新',
    body: [
      '自动更新会先校验安装包签名，签名不符的包不会被安装。即便如此，仍建议从官方仓库获取安装包，避免使用来源不明的修改版本。',
    ],
  },
  {
    title: '八、责任限制',
    body: [
      '本程序按「现状」提供，不附带任何明示或默示的担保，包括但不限于对适销性、特定用途适用性的担保。',
      '在法律允许的最大范围内，因使用或无法使用本程序而导致的数据丢失、系统异常、业务中断或其他损失，作者不承担赔偿责任。',
      '若你不接受本声明的任何内容，请停止使用并卸载本程序。',
    ],
  },
  {
    title: '九、许可与反馈',
    body: [
      '本程序以 MIT 许可开源，仓库地址见「设置 - 关于」，可自行查阅源码、构建方式与历史版本。问题与建议可通过仓库的 Issues 反馈。',
      '本声明可能随版本调整，最新版本可在「设置 - 关于」中随时查看。',
    ],
  },
]

export function DisclaimerModal({
  /** first-run：首次启动自动弹出，必须读到底部；review：从设置进入，随时可关 */
  mode,
  onClose,
  onAccept,
}: {
  mode: 'first-run' | 'review'
  onClose: () => void
  onAccept?: () => Promise<void> | void
}) {
  const bodyRef = useRef<HTMLDivElement>(null)
  const [atBottom, setAtBottom] = useState(mode === 'review')
  const [busy, setBusy] = useState(false)
  const gate = mode === 'first-run'

  const checkBottom = useCallback(() => {
    const el = bodyRef.current
    if (!el) return
    // 内容不足一屏时 scrollHeight 与 clientHeight 几乎相等，
    // 这种情况下必须直接放行，否则按钮永远点不了。
    if (el.scrollHeight - el.clientHeight <= 8) {
      setAtBottom(true)
      return
    }
    setAtBottom(el.scrollTop + el.clientHeight >= el.scrollHeight - 16)
  }, [])

  useEffect(() => {
    if (!gate) return
    const el = bodyRef.current
    if (!el) return
    checkBottom()
    // 字体加载完成后行高会变，高度会变，需要重新判定一次
    const t = window.setTimeout(checkBottom, 150)
    const ro = new ResizeObserver(checkBottom)
    ro.observe(el)
    if (el.firstElementChild) ro.observe(el.firstElementChild)
    return () => {
      window.clearTimeout(t)
      ro.disconnect()
    }
  }, [gate, checkBottom])

  // 首次启动时 Esc 不生效：要么读完点同意，要么直接结束进程
  useEffect(() => {
    if (gate) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [gate, onClose])

  const accept = async () => {
    if (busy || !atBottom) return
    if (!onAccept) {
      onClose()
      return
    }
    setBusy(true)
    try {
      await onAccept()
      onClose()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="modal-mask"
      // 首次启动时点遮罩不关闭：避免误触后声明再也不会出现
      onClick={gate ? undefined : onClose}
      role="dialog"
      aria-modal="true"
      aria-labelledby="disc-title"
    >
      <div
        className="disc-panel"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="disc-head">
          <span className="disc-badge"><ShieldCheck size={17} /></span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div id="disc-title" style={{ fontSize: 15, fontWeight: 600 }}>使用前请阅读</div>
            <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 3 }}>
              存储管理 Disk Manager · 免责声明
            </div>
          </div>
          {!gate && (
            <button className="icon-btn" onClick={onClose} aria-label="关闭">
              <X size={16} />
            </button>
          )}
        </div>

        <div className="disc-body" ref={bodyRef} onScroll={gate ? checkBottom : undefined}>
          <p className="disc-lead">
            本程序会读取磁盘与系统信息，并执行不可撤销的删除操作。
            在开始使用之前，请花一点时间了解它的能力边界与风险。
          </p>

          {SECTIONS.map((s) => (
            <section key={s.title} className="disc-sec">
              <h4>{s.title}</h4>
              {s.body.map((p, i) => (
                <p key={i}>{p}</p>
              ))}
            </section>
          ))}

          <div className="disc-end">
            <ChevronDown size={13} />
            以上为声明全文
          </div>
        </div>

        <div className="disc-foot">
          {gate ? (
            <span className={`disc-hint${atBottom ? ' ready' : ''}`}>
              {atBottom
                ? '已阅读至底部，可以继续了'
                : '请向下滚动阅读全文后再继续'}
            </span>
          ) : (
            <span className="disc-hint">本声明不会再次自动弹出，可在「设置 - 关于」中查看。</span>
          )}
          <button
            className="btn btn-primary"
            style={{ padding: '7px 16px', fontSize: 12.5 }}
            onClick={() => void accept()}
            disabled={gate && (!atBottom || busy)}
            title={gate && !atBottom ? '请先滚动到底部' : undefined}
          >
            {busy && <Loader2 size={13} className="spin" />}
            {gate ? '我已阅读并同意' : '关闭'}
          </button>
        </div>
      </div>
    </div>
  )
}
