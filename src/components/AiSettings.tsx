import { useEffect, useState } from 'react'
import {
  ArrowLeft, Sparkles, Check, AlertTriangle, Loader2, KeyRound, Server, ShieldCheck,
} from 'lucide-react'
import type { AiConfigPublic } from '../types'
import { aiGetConfig, aiSaveConfig, aiTestConnection } from '../services/api'

interface Preset {
  id: string
  name: string
  base: string
  model: string
  /** 本地模型不需要 Key */
  noKey?: boolean
}

/**
 * 内置额度的可选模型。
 *
 * 只放当前免费可用的那一个：`agnes-3.0-pro` 在内置额度里调不通，
 * 列出来只会让用户点了报错。只剩一项时界面渲染成只读行，不做单选组。
 */
const BUILTIN_MODELS: { id: string; name: string; desc: string }[] = [
  { id: 'agnes-3.0-flash', name: 'agnes-3.0-flash', desc: '免费可用，响应快，判断目录用途足够' },
]

/** 用户自带 Key 时的服务商预设。选好基本只需填 Key。 */
const PRESETS: Preset[] = [
  { id: 'deepseek', name: 'DeepSeek（性价比高）', base: 'https://api.deepseek.com/v1', model: 'deepseek-chat' },
  { id: 'qwen', name: '通义千问（阿里云）', base: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-plus' },
  { id: 'kimi', name: 'Kimi（月之暗面）', base: 'https://api.moonshot.cn/v1', model: 'moonshot-v1-8k' },
  { id: 'zhipu', name: '智谱 GLM', base: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash' },
  { id: 'openai', name: 'OpenAI', base: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  { id: 'ollama', name: '本地 Ollama（不出本机）', base: 'http://localhost:11434/v1', model: 'qwen2.5:7b', noKey: true },
  { id: 'custom', name: '自定义（OpenAI 兼容）', base: '', model: '' },
]

/** 把任意模型名收敛到内置白名单（后端也会做一遍，这里是让界面别显示一个用不了的模型） */
function pickBuiltinModel(m: string): string {
  return BUILTIN_MODELS.some((x) => x.id === m) ? m : BUILTIN_MODELS[0].id
}

function Field({ title, desc, children }: { title: string; desc?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div style={{ padding: '14px 18px', borderTop: '1px solid var(--border-soft)' }}>
      <div style={{ fontSize: 13.5, fontWeight: 600, marginBottom: 3 }}>{title}</div>
      {desc && <div style={{ fontSize: 11.5, color: 'var(--meta)', lineHeight: 1.65, marginBottom: 8 }}>{desc}</div>}
      {children}
    </div>
  )
}

const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '7px 10px',
  fontSize: 12.5,
  fontFamily: 'var(--font-mono)',
  borderRadius: 'var(--radius-md)',
  border: '1px solid var(--border)',
  background: 'var(--surface)',
  color: 'var(--fg)',
  boxSizing: 'border-box',
}

/** 模式选项卡片 */
function ModeCard({
  active, icon: Icon, title, desc, onClick,
}: {
  active: boolean
  icon: typeof Sparkles
  title: string
  desc: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        display: 'flex', alignItems: 'flex-start', gap: 11, width: '100%',
        padding: '12px 14px', textAlign: 'left',
        borderRadius: 'var(--radius-md)',
        border: `1px solid ${active ? 'var(--accent)' : 'var(--border-soft)'}`,
        background: active ? 'var(--accent-soft)' : 'var(--surface)',
        cursor: 'pointer',
        transition: 'border-color var(--t-fast), background var(--t-fast)',
      }}
    >
      <span style={{ color: active ? 'var(--accent)' : 'var(--meta)', flexShrink: 0, marginTop: 1 }}>
        <Icon size={17} />
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{
          display: 'flex', alignItems: 'center', gap: 7,
          fontSize: 13, fontWeight: 600, color: active ? 'var(--accent)' : 'var(--fg)',
        }}>
          {title}
          {active && <Check size={13} />}
        </span>
        <span style={{ display: 'block', fontSize: 11.5, color: 'var(--meta)', marginTop: 3, lineHeight: 1.6 }}>
          {desc}
        </span>
      </span>
    </button>
  )
}

/**
 * AI 设置的二级页。
 *
 * 从设置页的一个入口点进来，而不是把一堆表单塞在设置页中间——
 * 设置页本来就长，AI 的配置项又多（模式、地址、模型、Key、测试），
 * 混在一起会让「改个关闭行为」都得滚半天。
 *
 * 默认状态是「内置额度 + 已启用」，所以绝大多数用户不需要进这个页面。
 */
export function AiSettingsPage({ onBack }: { onBack: () => void }) {
  const [cfg, setCfg] = useState<AiConfigPublic | null>(null)
  const [loading, setLoading] = useState(true)

  const [enabled, setEnabled] = useState(true)
  const [mode, setMode] = useState<'builtin' | 'custom'>('builtin')
  const [builtinModel, setBuiltinModel] = useState('agnes-3.0-flash')

  const [provider, setProvider] = useState('deepseek')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [keyInput, setKeyInput] = useState('')

  // 只保留「称呼」——自定义指令与背景知识已移除（用的人少，且提示词越长越容易跑偏）
  const [assistantName, setAssistantName] = useState('')

  const [busy, setBusy] = useState<'save' | 'test' | 'reset' | null>(null)
  const [msg, setMsg] = useState<{ tone: 'ok' | 'bad'; text: string } | null>(null)

  useEffect(() => {
    let alive = true
    aiGetConfig()
      .then((c) => {
        if (!alive) return
        setCfg(c)
        setEnabled(c.enabled)
        setAssistantName(c.assistantName ?? '')
        if (c.isBuiltin) {
          setMode('builtin')
          setBuiltinModel(pickBuiltinModel(c.model))
        } else {
          setMode('custom')
          setProvider(c.provider || 'deepseek')
          setBaseUrl(c.baseUrl)
          setModel(c.model)
        }
      })
      .catch(() => { if (alive) setCfg(null) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [])

  const pickPreset = (id: string) => {
    setProvider(id)
    const p = PRESETS.find((x) => x.id === id)
    if (p && id !== 'custom') {
      setBaseUrl(p.base)
      setModel(p.model)
    }
    setMsg(null)
  }

  const save = async () => {
    if (busy) return
    setBusy('save')
    setMsg(null)
    try {
      const next = mode === 'builtin'
        ? await aiSaveConfig({
            provider: 'builtin', baseUrl: '', model: builtinModel, enabled,
            assistantName: assistantName.trim(),
          })
        : await aiSaveConfig({
            provider,
            baseUrl,
            model,
            enabled,
            apiKey: keyInput.trim() ? keyInput.trim() : undefined,
            assistantName: assistantName.trim(),
          })
      setCfg(next)
      setKeyInput('')
      setMsg({ tone: 'ok', text: '已保存' })
    } catch (e) {
      setMsg({ tone: 'bad', text: String((e as { message?: string })?.message ?? e) })
    } finally {
      setBusy(null)
    }
  }

  const test = async () => {
    if (busy) return
    setBusy('test')
    setMsg(null)
    try {
      const r = mode === 'builtin'
        ? await aiTestConnection('', builtinModel)
        : await aiTestConnection(baseUrl, model, keyInput.trim() || undefined)
      setMsg({ tone: 'ok', text: r })
    } catch (e) {
      setMsg({ tone: 'bad', text: String((e as { message?: string })?.message ?? e) })
    } finally {
      setBusy(null)
    }
  }

  const preset = PRESETS.find((p) => p.id === provider)
  const needsKey = !preset?.noKey

  return (
    <div className="page-enter" style={{ maxWidth: 880 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 16 }}>
        <button className="btn btn-ghost" style={{ padding: '5px 11px', fontSize: 12.5 }} onClick={onBack}>
          <ArrowLeft size={14} /> 返回设置
        </button>
        <span style={{ fontSize: 15, fontWeight: 600 }}>AI 助手</span>
        {!loading && (
          <span className={`tag ${enabled ? 'tag-safe' : 'tag-hdd'}`} style={{ marginLeft: 'auto' }}>
            {enabled ? '已启用' : '已关闭'}
          </span>
        )}
      </div>

      {/* 当前状态：让用户一眼知道「现在用的是谁的额度」 */}
      <div className="panel" style={{ padding: '16px 18px', marginBottom: 18 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <div className="quick-icon" style={{ width: 38, height: 38, background: 'var(--accent-soft)', color: 'var(--accent)', flexShrink: 0 }}>
            <Sparkles size={18} />
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 13.5, fontWeight: 600 }}>
              {loading ? '正在读取…' : cfg?.isBuiltin ? '内置额度' : `自定义 · ${cfg?.provider || ''}`}
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 3, fontFamily: 'var(--font-mono)' }}>
              {loading
                ? ''
                : cfg?.isBuiltin
                  ? `${cfg?.model || 'agnes-3.0-flash'} · 无需配置`
                  : `${cfg?.model || '未指定模型'}${cfg?.keyHint ? ` · ${cfg.keyHint}` : ''}`}
            </div>
          </div>
          <button
            role="switch"
            aria-checked={enabled}
            aria-label="启用 AI"
            onClick={() => setEnabled((v) => !v)}
            style={{
              width: 42, height: 24, borderRadius: 9999, padding: 2, border: 'none',
              display: 'flex', alignItems: 'center',
              justifyContent: enabled ? 'flex-end' : 'flex-start',
              background: enabled ? 'var(--accent)' : 'var(--border)',
              cursor: 'pointer', flexShrink: 0,
              transition: 'background var(--t-fast)',
            }}
          >
            <span style={{
              width: 20, height: 20, borderRadius: '50%', background: '#fff',
              boxShadow: '0 1px 3px rgba(0,0,0,.22)', display: 'block',
            }} />
          </button>
        </div>
        <div style={{ fontSize: 11.5, color: 'var(--meta)', marginTop: 10, lineHeight: 1.75 }}>
          关掉之后，「AI 深度分析」与右下角助手只保留本地问答库——常见问题仍然能答，但不会联网。
        </div>
      </div>

      {/* 模式选择 */}
      <div className="section-label">使用哪一种</div>
      <div className="panel" style={{ marginBottom: 18 }}>
        <div style={{ padding: 14, display: 'flex', flexDirection: 'column', gap: 9 }}>
          <ModeCard
            active={mode === 'builtin'}
            icon={Server}
            title="内置额度（默认）"
            desc="开箱可用，不需要注册也不需要填任何东西。额度有限，适合日常使用。"
            onClick={() => { setMode('builtin'); setMsg(null) }}
          />
          <ModeCard
            active={mode === 'custom'}
            icon={KeyRound}
            title="使用我自己的 API Key"
            desc="更稳定、可换更强的模型，也可以接本地 Ollama（完全不出本机）。费用由你自己和服务商结算。"
            onClick={() => { setMode('custom'); setMsg(null) }}
          />
        </div>
      </div>

      {/* 内置额度的模型选择 */}
      {mode === 'builtin' && (
        <>
          <div className="section-label">内置模型</div>
          <div className="panel" style={{ marginBottom: 18 }}>
            {BUILTIN_MODELS.length === 1 ? (
              // 只有一个可用模型时不给单选组：一个选项的单选题看着像坏了
              <div style={{ padding: '14px 18px', display: 'flex', alignItems: 'center', gap: 9, flexWrap: 'wrap' }}>
                <span className="mono" style={{ fontSize: 12.5, fontWeight: 600 }}>{BUILTIN_MODELS[0].id}</span>
                <span className="tag tag-safe">免费可用</span>
                <span style={{ fontSize: 11.5, color: 'var(--meta)' }}>{BUILTIN_MODELS[0].desc}</span>
              </div>
            ) : (
              <div style={{ padding: 14, display: 'flex', flexDirection: 'column', gap: 8 }}>
                {BUILTIN_MODELS.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    onClick={() => setBuiltinModel(m.id)}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 10, width: '100%',
                      padding: '10px 13px', textAlign: 'left',
                      borderRadius: 'var(--radius-md)',
                      border: `1px solid ${builtinModel === m.id ? 'var(--accent)' : 'var(--border-soft)'}`,
                      background: builtinModel === m.id ? 'var(--accent-soft)' : 'var(--surface)',
                      cursor: 'pointer',
                    }}
                  >
                    <span style={{ flex: 1, minWidth: 0 }}>
                      <span className="mono" style={{ fontSize: 12.5, fontWeight: 600, color: builtinModel === m.id ? 'var(--accent)' : 'var(--fg)' }}>
                        {m.name}
                      </span>
                      <span style={{ display: 'block', fontSize: 11.5, color: 'var(--meta)', marginTop: 2 }}>{m.desc}</span>
                    </span>
                    {builtinModel === m.id && <Check size={14} color="var(--accent)" />}
                  </button>
                ))}
              </div>
            )}
            <div style={{
              padding: '10px 18px', borderTop: '1px solid var(--border-soft)',
              fontSize: 11.5, color: 'var(--meta)', lineHeight: 1.75,
              display: 'flex', gap: 7, alignItems: 'flex-start',
            }}>
              <ShieldCheck size={13} style={{ flexShrink: 0, marginTop: 2 }} />
              <span>
                内置额度是体验性质的，长期重度使用建议换成自己的 Key。
                它同样只接收路径与结构摘要，不会上传你的文件内容。
              </span>
            </div>
          </div>
        </>
      )}

      {/* 自定义配置 */}
      {mode === 'custom' && (
        <>
          <div className="section-label">接口配置</div>
          <div className="panel" style={{ marginBottom: 18 }}>
            <Field title="服务商" desc="选预设基本只需填 Key；也可以选「自定义」接任何 OpenAI 兼容接口。">
              <select
                value={provider}
                onChange={(e) => pickPreset(e.target.value)}
                style={{ ...inputStyle, fontFamily: 'var(--font-body)' }}
              >
                {PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
              </select>
            </Field>

            <Field title="接口地址" desc="通常以 /v1 这类版本段结尾。填错会返回 404。">
              <input
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="https://api.example.com/v1"
                spellCheck={false}
                style={inputStyle}
              />
            </Field>

            <Field title="模型" desc="建议先用便宜的通用模型；判断目录用途不需要很强的模型。">
              <input
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="deepseek-chat"
                spellCheck={false}
                style={inputStyle}
              />
            </Field>

            <Field
              title="API Key"
              desc={
                needsKey
                  ? '只保存在本机。用 Windows 数据保护接口（DPAPI）加密，只有当前 Windows 账户能解开，不写明文、不发到任何中间服务器。'
                  : '本地模型通常不需要 Key，留空即可。'
              }
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <KeyRound size={14} color="var(--meta)" style={{ flexShrink: 0 }} />
                <input
                  type="password"
                  value={keyInput}
                  onChange={(e) => setKeyInput(e.target.value)}
                  placeholder={
                    cfg?.hasKey && !cfg.isBuiltin
                      ? `已保存 ${cfg.keyHint}，留空则不修改`
                      : needsKey ? '粘贴你的 API Key' : '本地模型可留空'
                  }
                  spellCheck={false}
                  autoComplete="off"
                  style={inputStyle}
                />
              </div>
            </Field>
          </div>
        </>
      )}

      {/* 助手称呼：与用哪种模型无关，始终可见 */}
      <div className="section-label">助手</div>
      <div className="panel" style={{ marginBottom: 18 }}>
        <Field title="称呼" desc="你希望怎么称呼它。留空就是「AI 助手」。">
          <input
            value={assistantName}
            onChange={(e) => setAssistantName(e.target.value)}
            placeholder="AI 助手"
            maxLength={20}
            style={inputStyle}
          />
        </Field>
      </div>

      {/* 操作区 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap', marginBottom: 12 }}>
        <button className="btn btn-primary" style={{ padding: '7px 16px', fontSize: 12.5 }} onClick={() => void save()} disabled={!!busy}>
          {busy === 'save' ? <><Loader2 size={13} className="spin" /> 保存中…</> : '保存'}
        </button>
        <button
          className="btn btn-ghost"
          style={{ padding: '7px 14px', fontSize: 12.5 }}
          onClick={() => void test()}
          disabled={!!busy || (mode === 'custom' && (!baseUrl || !model))}
        >
          {busy === 'test' ? <><Loader2 size={13} className="spin" /> 连接中…</> : '测试连接'}
        </button>
      </div>

      {msg && (
        <div style={{
          display: 'flex', alignItems: 'flex-start', gap: 8,
          padding: '10px 14px', borderRadius: 'var(--radius-md)', marginBottom: 12,
          fontSize: 12, lineHeight: 1.7,
          color: msg.tone === 'ok' ? 'var(--success)' : 'var(--warn)',
          background: msg.tone === 'ok' ? 'rgba(22,163,74,.06)' : 'rgba(217,119,6,.06)',
          border: `1px solid ${msg.tone === 'ok' ? 'rgba(22,163,74,.2)' : 'rgba(217,119,6,.2)'}`,
        }}>
          {msg.tone === 'ok' ? <Check size={13} style={{ flexShrink: 0, marginTop: 2 }} /> : <AlertTriangle size={13} style={{ flexShrink: 0, marginTop: 2 }} />}
          <span style={{ flex: 1, minWidth: 0, wordBreak: 'break-word' }}>{msg.text}</span>
        </div>
      )}

      <div style={{
        fontSize: 11.5, color: 'var(--meta)', lineHeight: 1.8,
        padding: '12px 14px', borderRadius: 'var(--radius-md)',
        background: 'var(--surface-warm)', border: '1px solid var(--border-soft)',
      }}>
        使用 AI 分析时，发送给模型的是：目录或文件的路径、体积、扩展名分布、子项名示例。
        <strong style={{ color: 'var(--fg-2)' }}>不发送任何文件内容。</strong>
        每次分析几百字，内置额度不额外收费；换成自己的 Key 后按服务商计费，普通模型一次通常不到一分钱。
      </div>
    </div>
  )
}
