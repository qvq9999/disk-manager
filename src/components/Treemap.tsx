import { useMemo } from 'react'
import type { FileNode, Category } from '../types'
import { squarify } from '../utils/treemap'
import { fmtBytes } from '../utils/format'

export const CAT_COLORS: Record<Category, string> = {
  video: 'var(--cat-video)',
  image: 'var(--cat-image)',
  audio: 'var(--cat-audio)',
  doc: 'var(--cat-doc)',
  archive: 'var(--cat-archive)',
  app: 'var(--cat-app)',
  system: 'var(--cat-system)',
  other: 'var(--cat-other)',
}
export const CAT_NAMES: Record<Category, string> = {
  video: '视频', image: '图片', audio: '音频', doc: '文档',
  archive: '压缩包', app: '程序应用', system: '系统', other: '其他',
}

interface Props {
  node: FileNode
  hidden: Set<Category>
  onOpen: (name: string) => void
  onPickFile?: (n: FileNode) => void
}

/** Treemap 主视图：矩形面积 = 占用大小，颜色 = 分类（绝不单独以颜色表意，配图例） */
export function Treemap({ node, hidden, onOpen, onPickFile }: Props) {
  const rects = useMemo(() => {
    const children = (node.children ?? []).filter((c) => !hidden.has(c.category))
    return squarify(children, { x: 0, y: 0, w: 1000, h: 640 })
  }, [node, hidden])

  return (
    <div
      style={{ position: 'relative', flex: 1, minHeight: 320 }}
      role="treegrid"
      aria-label="空间占用矩形树图"
    >
      {rects.map(({ item, rect }) => {
        const pct = node.size > 0 ? Math.round((item.size / node.size) * 100) : 0
        const isDir = item.kind === 'dir'
        return (
          <div
            key={item.name}
            className={`tm-cell${isDir ? ' dir' : ''}`}
            style={{
              left: `${(rect.x / 1000) * 100}%`,
              top: `${(rect.y / 640) * 100}%`,
              width: `${(rect.w / 1000) * 100}%`,
              height: `${(rect.h / 640) * 100}%`,
              background: CAT_COLORS[item.category],
              borderRadius: Math.min(rect.w, rect.h) > 40 ? 4 : 2,
            }}
            onClick={() => (isDir ? onOpen(item.name) : onPickFile?.(item))}
            title={`${item.name}\n${fmtBytes(item.size)} · 占当前层级 ${pct}%\n${item.mtime}`}
            tabIndex={0}
            onKeyDown={(e) => e.key === 'Enter' && isDir && onOpen(item.name)}
          >
            {rect.w > 52 && rect.h > 22 && (
              <span className="tm-label">
                {item.name} · {fmtBytes(item.size, 1)}
              </span>
            )}
          </div>
        )
      })}
      {rects.length === 0 && (
        <div style={{ display: 'grid', placeItems: 'center', height: '100%', color: 'var(--meta)', fontSize: 13 }}>
          当前分类筛选下无可见项
        </div>
      )}
    </div>
  )
}

export function TreemapLegend({ hidden, onToggle }: { hidden: Set<Category>; onToggle: (c: Category) => void }) {
  return (
    <div className="treemap-legend">
      {(Object.keys(CAT_NAMES) as Category[]).map((c) => (
        <button
          key={c}
          className={`legend-item${hidden.has(c) ? ' off' : ''}`}
          onClick={() => onToggle(c)}
          aria-pressed={!hidden.has(c)}
        >
          <span className="legend-dot" style={{ background: CAT_COLORS[c] }} />
          {CAT_NAMES[c]}
        </button>
      ))}
    </div>
  )
}
