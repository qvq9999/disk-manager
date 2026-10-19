import { useEffect, useRef, useState } from 'react'
import { Sparkles, X, Send, Settings as SettingsIcon, Loader2, AlertTriangle } from 'lucide-react'
import type { AiConfigPublic, AiMessage, Page } from '../types'
import { aiChatStream, aiGetConfig, aiSuggest } from '../services/api'
import { answerLocally } from '../services/knowledge'
import { guideFor } from '../services/aiGuide'
import { usePrefs } from '../services/prefs'
import { useAiContext } from '../services/aiContext'

/**
 * 右下角 AI 助手（悬浮球 + 侧滑面板）。
 *
 * 几个刻意的取舍：
 * - **不占导航位**：侧边栏保持 5 项，AI 是随时能叫出来的一层，而不是一个要记得去点的页面。
 * - **不挤压主内容**：面板是 fixed 浮层，绝不给 main 加 padding —— 那会让 Treemap 宽度变化、
 *   触发重排，用户看到的就是「一打开面板整页跳一下」。
 * - **未配置 Key 也能用**：常见问题走本地问答库，答不了才提示去配置。
 *   否则用户点开发现「什么都没有」，会认为功能坏了。
 */
export function AiAssistant({
  page,
  pageTitle,
  admin,
  diskContext = '',
  onGotoSettings,
}: {
  /** 当前页面 id：决定首屏「猜你想问」问什么 */
  page: Page
  pageTitle: string
  admin: boolean
  /** 磁盘剩余等真实处境，会作为上下文带给模型（B4） */
  diskContext?: string
  onGotoSettings: () => void
}) {
  const [open, setOpen] = useState(false)
  const [cfg, setCfg] = useState<AiConfigPublic | null>(null)
  const [msgs, setMsgs] = useState<AiMessage[]>([])
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  /** 回答之后给出的「可能想问」（B3） */
  const [suggests, setSuggests] = useState<string[]>([])

  const prefs = usePrefs()
  /** 当前页面上报的数据摘要（设置里关掉授权时为空） */
  const pageCtx = useAiContext()

  const listRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // Ctrl+K 唤起（native.ts 里派发）
  useEffect(() => {
    const onToggle = () => setOpen((o) => !o)
    window.addEventListener('dm:toggle-ai', onToggle)
    return () => window.removeEventListener('dm:toggle-ai', onToggle)
  }, [])

  useEffect(() => {
    if (!open) return
    let alive = true
    aiGetConfig()
      .then((c) => { if (alive) setCfg(c) })
      .catch(() => { if (alive) setCfg(null) })
    const t = window.setTimeout(() => inputRef.current?.focus(), 200)
    return () => {
      alive = false
      window.clearTimeout(t)
    }
  }, [open])

  // 只滚消息区，不动整页
  useEffect(() => {
    const el = listRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [msgs, busy])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  const send = async (raw?: string) => {
    const text = (raw ?? input).trim()
    if (!text || busy) return
    setInput('')
    setErr(null)
    setSuggests([])

    const next: AiMessage[] = [...msgs, { role: 'user', content: text }]
    setMsgs(next)

    // 1) 本地库优先：零延迟、零费用
    const local = answerLocally(text)
    if (local) {
      setMsgs([...next, { role: 'assistant', content: local }])
      return
    }

    // 2) 没配置 AI 就诚实说，不要假装在思考
    if (!cfg?.enabled) {
      setMsgs([
        ...next,
        {
          role: 'assistant',
          content:
            '这个问题本地问答库答不了。到「设置 → AI 助手」配置接口之后，我可以回答更复杂的问题，也能解释你的具体扫描结果。',
        },
      ])
      return
    }

    setBusy(true)
    // 先放一个空的助手气泡，然后边收边往里追加 —— 这就是「流式」的全部秘密
    const placeholder: AiMessage = { role: 'assistant', content: '' }
    setMsgs([...next, placeholder])
    try {
      // B4：把磁盘剩余、当前页面的数据摘要一并带上，回答才贴得住用户当下在看的界面
      const ctx = [
        `当前页面：${pageTitle}`,
        `权限：${admin ? '管理员' : '标准用户（部分系统目录读不到）'}`,
        diskContext || '',
        prefs.allowPageContext ? pageCtx : '',
      ]
        .filter(Boolean)
        .join('\n')

      const full = await aiChatStream(next, ctx, (t) => {
        setMsgs((prev) => {
          const last = prev[prev.length - 1]
          if (!last || last.role !== 'assistant') return prev
          const copy = prev.slice()
          copy[copy.length - 1] = { ...last, content: last.content + t }
          return copy
        })
      })
      // 用返回值校正一次：网络抖动丢块时，界面不能停在一半
      setMsgs((prev) => {
        const copy = prev.slice()
        if (copy.length > 0) copy[copy.length - 1] = { role: 'assistant', content: full.trim() }
        return copy
      })
      // 追问建议在后台另起一次轻量请求，不占用主回答的等待时间
      void aiSuggest(text, full).then((v) => {
        if (v.length > 0) setSuggests(v)
      })
    } catch (e) {
      setErr(String((e as { message?: string })?.message ?? e))
      // 回滚到只保留用户这一问，空的气泡不要留在界面上
      setMsgs(next)
    } finally {
      setBusy(false)
    }
  }

  const configured = !!cfg?.enabled
  // 引导语与「猜你想问」随页面变化：在硬件页问配置、在空间分析页问能不能删
  const guide = guideFor(page)

  // 「正在思考」只在还没有任何内容可显示时出现。
  // 之前 busy 一直为真、而流式气泡已经在长了，于是同时冒出两个气泡：
  // 一个在打字、一个写着「正在思考」。这里只在首字未到时才显示占位。
  const lastMsg = msgs[msgs.length - 1]
  const waitingFirstToken =
    busy && (!lastMsg || lastMsg.role !== 'assistant' || lastMsg.content.length === 0)

  return (
    <>
      {!open && (
        <button
          type="button"
          className="ai-ball"
          onClick={() => setOpen(true)}
          title="问 AI（Ctrl+K）"
          aria-label="打开 AI 助手"
        >
          <Sparkles size={19} />
        </button>
      )}

      {open && (
        <div className="ai-panel" role="dialog" aria-label="AI 助手">
          <div className="ai-head">
            <span className="ai-head-icon"><Sparkles size={15} /></span>
            <div style={{ minWidth: 0, flex: 1 }}>
              <div style={{ fontSize: 13, fontWeight: 600 }}>AI 助手</div>
              <div className="ai-head-sub">
                {configured
                  ? `${cfg?.isBuiltin ? '内置额度' : cfg?.provider || '自定义'} · ${cfg?.model || '未指定模型'}`
                  : '本地问答模式（AI 已关闭）'}
              </div>
            </div>
            <button
              type="button"
              className="icon-btn"
              onClick={() => setOpen(false)}
              title="关闭（Esc）"
              aria-label="关闭"
            >
              <X size={16} />
            </button>
          </div>

          {!configured && (
            <div className="ai-tip">
              <span style={{ flex: 1, minWidth: 0 }}>
                常见问题本地就能答。配置 AI 后可以问更复杂的，也能解释你的具体扫描结果。
              </span>
              <button
                type="button"
                className="btn btn-ghost ai-tip-btn"
                onClick={() => { setOpen(false); onGotoSettings() }}
              >
                <SettingsIcon size={12} /> 去配置
              </button>
            </div>
          )}

          <div className="ai-list" ref={listRef}>
            {msgs.length === 0 && (
              <div className="ai-hello">
                <div className="ai-hello-title">{guide.title}</div>
                <div className="ai-hello-sub">
                  当前在「{pageTitle}」
                  {prefs.allowPageContext
                    ? '，我会结合这一页的数据回答。'
                    : '。未开启「读取当前页面」，回答不会包含你的具体数据。'}
                </div>
                <div className="ai-chips">
                  {guide.chips.map((s) => (
                    <button key={s} type="button" className="ai-chip" onClick={() => void send(s)}>
                      {s}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {msgs.map((m, i) => (
              <div key={i} className={`ai-msg ${m.role}`}>
                {m.role === 'assistant' && <span className="ai-avatar"><Sparkles size={12} /></span>}
                <div className="ai-bubble">{m.content}</div>
              </div>
            ))}

            {waitingFirstToken && (
              <div className="ai-msg assistant">
                <span className="ai-avatar"><Sparkles size={12} /></span>
                <div className="ai-bubble ai-thinking">
                  <Loader2 size={13} className="spin" /> 正在思考…
                </div>
              </div>
            )}

            {err && (
              <div className="ai-err">
                <AlertTriangle size={13} style={{ flexShrink: 0, marginTop: 1 }} />
                <span style={{ flex: 1, minWidth: 0, wordBreak: 'break-word' }}>{err}</span>
              </div>
            )}
          </div>

          {suggests.length > 0 && !busy && (
            <div className="ai-suggest">
              {suggests.map((s) => (
                <button key={s} type="button" className="ai-chip" onClick={() => void send(s)}>
                  {s}
                </button>
              ))}
            </div>
          )}

          <div className="ai-foot">
            <input
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  void send()
                }
              }}
              placeholder="问点什么…（Enter 发送）"
              maxLength={500}
            />
            <button
              type="button"
              className="btn btn-primary ai-send"
              onClick={() => void send()}
              disabled={busy || input.trim().length === 0}
              title="发送"
              aria-label="发送"
            >
              {busy ? <Loader2 size={14} className="spin" /> : <Send size={14} />}
            </button>
          </div>

          <div className="ai-foot-note">
            {configured
              ? '只发送问题与你所在页面，不上传任何磁盘数据'
              : '配置后可回答更复杂的问题'}
          </div>
        </div>
      )}
    </>
  )
}
