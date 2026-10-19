import { useEffect, useState } from 'react'
import {
  Check, AlertTriangle, HelpCircle, Loader2, Sparkles, FolderOpen, Copy,
  ChevronDown, ChevronUp,
} from 'lucide-react'
import type { AiAnalysis, FileNode, IdentifyResult, Verdict } from '../types'
import { aiAnalyze, aiGetConfig, identifyNode, revealInExplorer } from '../services/api'
import { LayerAnalysis } from './LayerAnalysis'

const TONE: Record<Verdict, { color: string; bg: string; text: string }> = {
  safe: { color: 'var(--success)', bg: 'rgba(22,163,74,.07)', text: '可以清理' },
  caution: { color: 'var(--warn)', bg: 'rgba(217,119,6,.07)', text: '谨慎处理' },
  danger: { color: 'var(--danger)', bg: 'rgba(220,38,38,.07)', text: '不要删除' },
  unknown: { color: 'var(--meta)', bg: 'rgba(138,147,161,.08)', text: '判不出来' },
}

function VerdictIcon({ v }: { v: Verdict }) {
  if (v === 'danger') return <AlertTriangle size={13} />
  if (v === 'caution') return <AlertTriangle size={13} />
  if (v === 'safe') return <Check size={13} />
  return <HelpCircle size={13} />
}

/** 只在信息不足时才提一句，确定的不必再写「确定」（右上角标签已经表达了） */
const CONF_TEXT = ['', '依据有限', ''] as const

/**
 * AI 分析结果缓存。
 *
 * 同一个目录大小没变，结论就不会变；重复点「深度分析」只是白花额度。
 * 用「路径 + 体积」作键：内容一变体积就变，缓存自然失效，不需要额外过期策略。
 */
const aiCache = new Map<string, AiAnalysis>()

/**
 * 「这是什么 / 能不能删」卡片。
 *
 * 产品判断：用户真正要的不是文件格式，而是「这堆东西能不能删」。
 * 所以卡片固定回答三个问题：是什么 → 能不能删 → 删了会怎样，并把判断依据摊开给人看。
 *
 * 布局约束（踩过的坑）：卡片高度不能随加载状态跳变，
 * 否则下面 Treemap 会跟着重排，界面看起来在「闪」。所以这里给最小高度占位。
 */
export function WhatIsThis({
  path,
  kind,
  size,
  siblings,
  onGotoClean,
}: {
  path: string
  kind: 'dir' | 'file'
  size: number
  /** 当前目录的直接子项，用于「分析这一层」；文件视图不传 */
  siblings?: FileNode[]
  onGotoClean?: () => void
}) {
  const [res, setRes] = useState<IdentifyResult | null>(null)
  const [loading, setLoading] = useState(true)
  const [failed, setFailed] = useState(false)
  const [showEvidence, setShowEvidence] = useState(false)

  const [aiOn, setAiOn] = useState(false)
  const [aiBusy, setAiBusy] = useState(false)
  const [aiErr, setAiErr] = useState<string | null>(null)
  const [aiOpen, setAiOpen] = useState(false)
  /** AI 给出的建议动作，只用于显示，不参与删除 */
  const [recall, setRecall] = useState('')

  useEffect(() => {
    let alive = true
    setLoading(true)
    setFailed(false)
    setRes(null)
    setAiOpen(false)
    setAiErr(null)
    setShowEvidence(false)

    identifyNode(path, kind, size)
      .then((r) => { if (alive) setRes(r) })
      .catch(() => { if (alive) setFailed(true) })
      .finally(() => { if (alive) setLoading(false) })

    aiGetConfig()
      .then((c) => { if (alive) setAiOn(!!c.enabled && (c.hasKey || /localhost|127\.0\.0\.1/i.test(c.baseUrl))) })
      .catch(() => { if (alive) setAiOn(false) })

    return () => { alive = false }
  }, [path, kind, size])

  const aiCacheKey = `${kind}|${path}|${size}`

  /** 把 AI 结论套到当前卡片上：只覆盖文案，不覆盖本地采集到的依据 */
  const applyAi = (a: AiAnalysis, base: IdentifyResult) => {
    setAiOpen(true)
    setRes({
      ...base,
      source: 'ai',
      what: a.what || base.what,
      consequence: a.consequence || base.consequence,
      verdict: a.canDelete,
      label: base.label === '未能识别' || base.confidence === 0 ? (a.what.slice(0, 18) || base.label) : base.label,
    })
    if (a.suggestion) setRecall(a.suggestion)
  }

  const runAi = async () => {
    if (!res || aiBusy) return
    // 命中缓存就别再花一次额度
    const hit = aiCache.get(aiCacheKey)
    if (hit) {
      applyAi(hit, res)
      return
    }
    // 没配置就别发一个注定失败的请求，直接告诉用户去哪儿配
    if (!aiOn) {
      setAiErr('还没有配置 AI。到「设置 - AI 助手」选一个服务商、填上 Key，回来后就能用深度分析。')
      return
    }
    setAiBusy(true)
    setAiErr(null)
    try {
      const a = await aiAnalyze({
        path,
        kind,
        size,
        summary: res.summary,
        localLabel: res.label,
        localVerdict: res.verdict,
      })
      aiCache.set(aiCacheKey, a)
      if (aiCache.size > 60) {
        const oldest = aiCache.keys().next().value
        if (oldest) aiCache.delete(oldest)
      }
      applyAi(a, res)
    } catch (e) {
      setAiErr(String((e as { message?: string })?.message ?? e))
    } finally {
      setAiBusy(false)
    }
  }

  // AI 给出的建议动作（不参与删除，只用来显示）
  useEffect(() => { setRecall('') }, [path])

  const tone = TONE[res?.verdict ?? 'unknown']

  if (loading) {
    return (
      <div className="what-card what-loading">
        <span className="loading-spinner small" />
        <span style={{ fontSize: 12.5, color: 'var(--muted)' }}>正在判断这是什么…</span>
      </div>
    )
  }

  if (failed || !res) {
    return (
      <div className="what-card what-loading">
        <HelpCircle size={14} color="var(--meta)" />
        <span style={{ fontSize: 12.5, color: 'var(--muted)' }}>无法判断这个位置的用途</span>
      </div>
    )
  }

  return (
    <>
    <div className="what-card" style={{ borderLeftColor: tone.color, background: tone.bg }}>
      <div className="what-main">
        <div className="what-title-row">
          <span className="what-label">{res.label}</span>
          <span className="what-verdict" style={{ color: tone.color, borderColor: tone.color }}>
            <VerdictIcon v={res.verdict} /> {tone.text}
          </span>
          <span className="what-conf">
            {res.source === 'ai' ? 'AI 判断' : CONF_TEXT[res.confidence]}
            {res.truncated ? `${res.source === 'ai' ? ' ' : ''}· 部分采样` : ''}
          </span>
        </div>

        <div className="what-line">
          <span className="what-k">这是什么</span>
          <span className="what-v">{res.what}</span>
        </div>
        <div className="what-line">
          <span className="what-k">删了会怎样</span>
          <span className="what-v" style={{ color: res.verdict === 'danger' ? 'var(--danger)' : undefined }}>
            {res.consequence}
          </span>
        </div>
        {recall && (
          <div className="what-line">
            <span className="what-k">建议</span>
            <span className="what-v">{recall}</span>
          </div>
        )}
      </div>

      <div className="what-actions">
        <button
          type="button"
          className="what-btn"
          onClick={() => setShowEvidence((v) => !v)}
          title="展开判断依据"
        >
          {showEvidence ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
          依据 {res.evidence.length}
        </button>

        <button
          type="button"
          className={`what-btn${aiOn ? '' : ' muted'}`}
          onClick={() => void runAi()}
          disabled={aiBusy}
          title={aiOn ? '把摘要发给模型做深度判断（不含文件内容）' : '需要先在设置里配置 AI'}
        >
          {aiBusy ? <Loader2 size={12} className="spin" /> : <Sparkles size={12} />}
          {aiBusy ? '分析中' : 'AI 深度分析'}
        </button>

        {res.verdict === 'safe' && onGotoClean && (
          <button type="button" className="what-btn" onClick={onGotoClean} title="去垃圾清理页处理">
            去清理
          </button>
        )}

        <button
          type="button"
          className="what-btn"
          onClick={() => void revealInExplorer(path)}
          title="在资源管理器中打开"
        >
          <FolderOpen size={12} /> 打开
        </button>

        <button
          type="button"
          className="what-btn"
          onClick={() => void copyPath(path)}
          title="复制完整路径"
        >
          <Copy size={12} /> 路径
        </button>
      </div>

      {showEvidence && (
        <div className="what-evidence">
          {res.evidence.map((e, i) => (
            <div key={i} className="what-ev-item">{e}</div>
          ))}
          {res.summary && (
            <>
              <div className="what-ev-title">点「AI 深度分析」时发送的内容（不含任何文件内容）</div>
              <pre className="what-ev-summary">{res.summary}</pre>
            </>
          )}
          <div className="what-ev-note">
            <HelpCircle size={11} /> 判断结果仅供参考，删除操作仍需要你自己勾选并确认。
          </div>
        </div>
      )}

      {aiErr && (
        <div className="what-evidence" style={{ borderTopColor: 'rgba(220,38,38,.2)' }}>
          <div className="what-ev-item" style={{ color: 'var(--danger)' }}>AI 分析失败：{aiErr}</div>
        </div>
      )}
    </div>

    {kind === 'dir' && siblings && siblings.length > 1 && (
      // key 用路径：切目录时强制重挂载，否则会留着上一层的分析结果
      <LayerAnalysis key={path} basePath={path} items={siblings} aiOn={aiOn} />
    )}
    </>
  )
}

async function copyPath(p: string) {
  try {
    await navigator.clipboard.writeText(p)
  } catch {
    // 剪贴板不可用时不要静默失败
    alert(`无法访问剪贴板，路径如下：\n${p}`)
  }
}
