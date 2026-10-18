import { useEffect, useRef, useState } from 'react'
import { Sparkles, X, Send, Settings as SettingsIcon, Loader2, AlertTriangle } from 'lucide-react'
import type { AiConfigPublic, AiMessage } from '../types'
import { aiChat, aiGetConfig } from '../services/api'
import { answerLocally, SUGGESTIONS } from '../services/knowledge'

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
  pageTitle,
  admin,
  onGotoSettings,
}: {
  pageTitle: string
  admin: boolean
  onGotoSettings: () => void
}) {
  const [open, setOpen] = useState(false)
  const [cfg, setCfg] = useState<AiConfigPublic | null>(null)
  const [msgs, setMsgs] = useState<AiMessage[]>([])
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

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
    try {
      const ctx = `当前页面：${pageTitle}\n权限：${admin ? '管理员' : '标准用户（部分系统目录读不到）'}`
      const reply = await aiChat(next, ctx)
      setMsgs([...next, { role: 'assistant', content: reply.trim() }])
    } catch (e) {
      setErr(String((e as { message?: string })?.message ?? e))
      // 失败的这一轮留在界面上，方便用户看清问了什么
      setMsgs(next)
    } finally {
      setBusy(false)
    }
  }

  const configured = !!cfg?.enabled

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
                <div className="ai-hello-title">这个软件怎么用，或者这块空间能不能删，都可以问我</div>
                <div className="ai-hello-sub">
                  当前在「{pageTitle}」。我会结合你所在的页面回答。
                </div>
                <div className="ai-chips">
                  {SUGGESTIONS.map((s) => (
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

            {busy && (
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
