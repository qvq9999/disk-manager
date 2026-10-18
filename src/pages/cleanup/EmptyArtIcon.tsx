import type { LucideIcon } from 'lucide-react'

/**
 * 清理页三个页签共用的空态图标。
 *
 * 之前三个页签各画各的：垃圾扫描是 112px 的自绘 SVG，安装包与空文件夹是
 * 96px 圆底 + 44px 图标，切页时图标尺寸明显跳一下。统一抽到这里，
 * 以后再加页签也不会又长出第三种规格。
 */
export function EmptyArtIcon({ icon: Icon }: { icon: LucideIcon }) {
  return (
    <div
      style={{
        width: 96,
        height: 96,
        borderRadius: '50%',
        background: 'var(--accent-soft)',
        display: 'grid',
        placeItems: 'center',
        color: 'var(--accent)',
      }}
    >
      <Icon size={44} strokeWidth={1.6} />
    </div>
  )
}
