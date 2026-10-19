import { useEffect, useState } from 'react'
import { Layers, Loader2, Sparkles, AlertTriangle, RefreshCw, X } from 'lucide-react'
import type { AiBatchResult, FileNode } from '../types'
import { aiAnalyzeBatch } from '../services/api'
import { usePrefs } from '../services/prefs'
import { fmtBytes } from '../utils/format'

/**
 * 整层批量分析：一次看清当前目录下最大的几项分别是什么。
 *
 * 右侧面板现在就是 AI 的展示位（树图已移到别处），所以**进入目录就自动分析**，
 * 不再需要用户先点一下按钮。结果按「路径 + 项数 + 总体积」缓存，同一层来回看不重复请求。
 *
 * 为什么值得整层一起问而不是逐个点：用户真正的问题是「这一层里哪些能删」。
 * 一次调用把同层几项一起给模型，它还能互相参照（认出某个目录是另一个的运行时），
 * token 也比逐个调用省得多。
 */
const cache = new Map<string, AiBatchResult[]>()

const TONE = {
  safe: { color: 'var(--success)', text: '可以清理' },
  caution: { color: 'var(--warn)', text: '谨慎处理' },
  danger: { color: 'var(--danger)', text: '不要删除' },
  unknown: { color: 'var(--meta)', text: '判不出来' },
} as const

type Verdict = keyof typeof TONE

function toneOf(v: string) {
  return TONE[(v as Verdict) in TONE ? (v as Verdict) : 'unknown']
}

export function LayerAnalysis({
  basePath,
  items,
  aiOn,
}: {
  basePath: string
  /** 当前目录的直接子项（已按体积排序） */
  items: FileNode[]
  aiOn: boolean
}) {
  // 「（其余 N 项）」是聚合出来的假节点，分析它没有意义
  const top = items.filter((n) => !n.name.startsWith('（其余 ')).slice(0, 5)
  const cacheKey = `${basePath}|${top.length}|${top.reduce((s, n) => s + n.size, 0)}`

  const [res, setRes] = useState<AiBatchResult[] | null>(() => cache.get(cacheKey) ?? null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  // 自动分析可以在设置里关掉（浏览目录时会持续消耗额度）
  const autoOn = usePrefs().autoAnalyzeLayer

  const run = async (silent = false) => {
    if (busy) return
    if (!aiOn) {
      if (!silent) setErr('还没有配置 AI。到「设置 - AI 助手」里打开后就能用整层分析。')
      return
    }
    setBusy(true)
    setErr(null)
    try {
      const out = await aiAnalyzeBatch(
        top.map((n) => ({
          name: n.name,
          path: basePath.endsWith('\\') ? `${basePath}${n.name}` : `${basePath}\\${n.name}`,
          kind: n.kind,
          size: n.size,
          summary: '',
        })),
      )
      cache.set(cacheKey, out)
      if (cache.size > 40) {
        const oldest = cache.keys().next().value
        if (oldest) cache.delete(oldest)
      }
      setRes(out)
    } catch (e) {
      setErr(String((e as { message?: string })?.message ?? e))
    } finally {
      setBusy(false)
    }
  }

  // 进入目录自动分析。命中缓存直接跳过，不会因为来回切目录而重复消耗额度。
  // 关掉「自动分析」后这里不再触发，用户仍可手动点「开始分析」。
  useEffect(() => {
    if (!aiOn || !autoOn || cache.has(cacheKey)) return
    void run(true)
    // 只应在「目录变了」「AI 从关到开」「自动分析开关变化」时触发
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cacheKey, aiOn, autoOn])

  // 少于两项就没什么可横向比较的，交给上面那张单项目卡片就够了
  if (top.length < 2) return null

  return (
    <div className="layer-ai">
      <div className="layer-head">
        <span className="layer-icon"><Layers size={13} /></span>
        <span className="layer-title">这一层最大的 {top.length} 项</span>
        <span className="layer-sub">
          {aiOn ? '按体积排序，AI 逐项判断' : '配置 AI 后可自动分析它们分别是什么'}
        </span>
        {aiOn && (
          <button type="button" className="what-btn" onClick={() => void run()} disabled={busy}>
            {busy ? <Loader2 size={12} className="spin" /> : res ? <RefreshCw size={12} /> : <Sparkles size={12} />}
            {busy ? '分析中' : res ? '重新分析' : '开始分析'}
          </button>
        )}
        {res && (
          <button type="button" className="what-btn" onClick={() => setRes(null)} title="收起结果">
            <X size={12} />
          </button>
        )}
      </div>

      {err && (
        <div className="layer-err">
          <AlertTriangle size={12} style={{ flexShrink: 0, marginTop: 2 }} />
          <span>{err}</span>
        </div>
      )}

      {res && (
        <div className="layer-rows">
          {res.map((r) => {
            const it = top.find((t) => t.name === r.name)
            const tone = toneOf(r.canDelete)
            return (
              <div className="layer-row" key={r.name}>
                <div className="layer-row-top">
                  <span className="layer-name" title={r.name}>{r.name}</span>
                  {it && <span className="layer-size mono">{fmtBytes(it.size, 1)}</span>}
                  <span className="layer-verdict" style={{ color: tone.color, borderColor: tone.color }}>
                    {tone.text}
                  </span>
                </div>
                <div className="layer-what">
                  {r.what || '模型没有给出说明'}
                  {r.consequence && <span className="layer-conseq"> · {r.consequence}</span>}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
