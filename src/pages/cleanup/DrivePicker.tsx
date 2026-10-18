import type { Volume } from '../../types'
import { Check, HardDrive } from 'lucide-react'

/**
 * 盘符选择器：与「垃圾扫描」页保持同一套视觉，抽出来给
 * 「安装包清理 / 空文件夹」两个子页复用，避免三份重复代码各自漂移。
 */
export function DrivePicker({
  drives,
  selected,
  onToggle,
  disabled,
}: {
  drives: Volume[]
  selected: Set<string>
  onToggle: (letter: string) => void
  disabled?: boolean
}) {
  if (drives.length === 0) {
    return <div className='status-line'>正在读取盘符…</div>
  }
  return (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, justifyContent: 'center', maxWidth: 660 }}>
      {drives.map((d) => {
        const on = selected.has(d.letter)
        return (
          <button
            key={d.letter}
            onClick={() => onToggle(d.letter)}
            disabled={disabled}
            title={`${d.letter}: ${d.label || '本地磁盘'}`}
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 8,
              padding: '8px 14px', borderRadius: 'var(--radius-md)',
              border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
              background: on ? 'var(--accent-soft)' : 'var(--surface)',
              color: on ? 'var(--accent)' : 'var(--fg-2)', fontSize: 13,
              opacity: disabled ? 0.7 : 1,
            }}
          >
            <span className={`checkbox${on ? ' on' : ''}`} style={{ width: 16, height: 16 }}>
              {on && <Check size={11} />}
            </span>
            <HardDrive size={14} />
            <span style={{ fontWeight: 600 }}>{d.letter}:</span>
            <span style={{ color: 'var(--meta)', fontSize: 11.5 }}>{d.label || '本地磁盘'}</span>
          </button>
        )
      })}
    </div>
  )
}
