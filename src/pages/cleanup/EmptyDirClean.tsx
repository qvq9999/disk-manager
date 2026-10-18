import { useEffect, useState } from 'react'
import {
  FolderMinus, ScanSearch, Trash2, Loader2, Check, RefreshCw,
  Square, Info, AlertTriangle,
} from 'lucide-react'
import type { EmptyDirItem, Volume } from '../../types'
import {
  scanEmptyDirs, cancelEmptyDirScan, deleteEmptyDirs, peekVolumes,
} from '../../services/api'
import { DrivePicker } from './DrivePicker'
import { EmptyArtIcon } from './EmptyArtIcon'

/**
 * 空文件夹清理
 *
 * 「空」的定义：目录下没有任何文件（递归），且所有子目录也都是空的。
 * 删除只调 `remove_dir` —— 扫描之后万一又被写入内容，删除必然失败，
 * 不会把新内容一起删掉。这类目录删掉通常只是清理视觉噪音，
 * 但对「下载后随手生成的空目录」「卸载残留的空壳」很有效。
 *
 * 默认同样一个都不勾：某些软件依赖空目录存在（启动时不会重建），
 * 由用户自己判断更稳妥。
 */
type Phase = 'idle' | 'scanning' | 'ready' | 'cleaning' | 'done'

function fmtDate(secs: number): string {
  if (!secs) return '—'
  const d = new Date(secs * 1000)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export function EmptyDirClean() {
  const [drives, setDrives] = useState<Volume[]>(() => peekVolumes() ?? [])
  const [chosen, setChosen] = useState<Set<string>>(
    () => new Set((peekVolumes() ?? []).map((v) => v.letter)),
  )
  const [phase, setPhase] = useState<Phase>('idle')
  const [items, setItems] = useState<EmptyDirItem[]>([])
  const [sel, setSel] = useState<Set<string>>(new Set())
  const [pct, setPct] = useState(0)
  const [msg, setMsg] = useState('')
  const [removed, setRemoved] = useState(0)
  const [failed, setFailed] = useState(0)

  useEffect(() => () => { void cancelEmptyDirScan() }, [])

  const selCount = sel.size
  const allOn = items.length > 0 && items.every((i) => sel.has(i.path))

  const toggle = (p: string) => {
    setSel((s) => {
      const n = new Set(s)
      if (n.has(p)) n.delete(p)
      else n.add(p)
      return n
    })
  }

  const start = async () => {
    if (chosen.size === 0) return
    const list = [...chosen].sort()
    setPhase('scanning')
    setPct(0)
    setMsg('正在准备扫描…')
    try {
      const res = await scanEmptyDirs(list, (p) => {
        setPct(p.pct)
        setMsg(p.current)
      })
      setItems(res)
      setSel(new Set())
      setPhase('ready')
    } catch (e) {
      const s = String(e)
      if (s.includes('EMPTYDIR_CANCELLED')) {
        setPhase('idle')
        setMsg('扫描已停止')
      } else {
        setPhase('idle')
        alert(`扫描失败：${s}`)
      }
    }
  }

  const doDelete = async () => {
    const paths = [...sel]
    if (paths.length === 0) return
    const ok = window.confirm(
      `将删除 ${paths.length} 个空文件夹。\n\n` +
      `只删除**完全空**的目录；如果某个目录在这期间被写入了内容，会自动跳过。\n` +
      `删除后无法撤销（空目录本身不含数据）。\n\n确定继续？`,
    )
    if (!ok) return
    setPhase('cleaning')
    setPct(0)
    try {
      const [okCount, failCount] = await deleteEmptyDirs(paths, (p) => {
        setPct(p.pct)
        setMsg(p.current)
      })
      setRemoved(okCount)
      setFailed(failCount)
      setItems((prev) => prev.filter((i) => !sel.has(i.path)))
      setSel(new Set())
      setPhase('done')
    } catch (e) {
      setPhase('ready')
      alert(`清理失败：${e}`)
    }
  }

  const toggleDrive = (l: string) => {
    setChosen((s) => {
      const n = new Set(s)
      if (n.has(l)) n.delete(l)
      else n.add(l)
      return n
    })
  }

  // ---- 空态 / 扫描中 ----
  if (phase === 'idle' || phase === 'scanning') {
    const scanning = phase === 'scanning'
    return (
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', gap: 20, padding: '24px 0' }}>
        <EmptyArtIcon icon={FolderMinus} />
        <div style={{ textAlign: 'center', maxWidth: 560 }}>
          <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 6 }}>扫描设备中的空文件夹</div>
          <div style={{ fontSize: 12.5, color: 'var(--muted)', lineHeight: 1.7 }}>
            只收录「没有任何文件、且子目录也全空」的目录，系统目录与盘根不会被列入<br />
            扫描到的项目默认不勾选，由你决定清理哪些
          </div>
        </div>

        <DrivePicker drives={drives} selected={chosen} onToggle={toggleDrive} disabled={scanning} />

        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <button
            className='btn btn-ghost'
            onClick={() => setChosen((s) => (s.size === drives.length ? new Set() : new Set(drives.map((d) => d.letter))))}
            disabled={scanning || drives.length === 0}
          >
            {chosen.size === drives.length && drives.length > 0 ? '取消全选' : '全选'}
          </button>
          {scanning ? (
            <button className='btn btn-ghost' onClick={() => void cancelEmptyDirScan()}>
              <Square size={14} /> 停止扫描
            </button>
          ) : (
            <button className='btn btn-primary' onClick={start} disabled={chosen.size === 0}>
              <ScanSearch size={15} /> 开始扫描
            </button>
          )}
        </div>

        {scanning && (
          <div style={{ width: 'min(460px, 84%)' }}>
            <div className='progress-track'>
              <div className='progress-fill' style={{ width: `${Math.max(2, pct)}%` }} />
            </div>
            <div className='status-line' style={{ justifyContent: 'center', marginTop: 6, gap: 8 }}>
              <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 300 }}>
                {msg || '正在扫描…'}
              </span>
              <span className='mono' style={{ fontSize: 11, color: 'var(--meta)', flexShrink: 0 }}>{pct}%</span>
            </div>
            <div className='status-line' style={{ justifyContent: 'center', marginTop: 2, fontSize: 11 }}>
              范围：{[...chosen].sort().map((l) => `${l}:`).join(' ')} · 需要遍历目录树
            </div>
          </div>
        )}
      </div>
    )
  }

  // ---- 结果态 ----
  const isCleaning = phase === 'cleaning'
  const isDone = phase === 'done'
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div className='summary-bar'>
        <div>
          <div className='sum-num' style={isDone ? { color: 'var(--success)' } : undefined}>
            {isDone ? removed : selCount}
            <span style={{ fontSize: 15, marginLeft: 4 }}>个</span>
          </div>
          <div className='sum-label'>{isDone ? '本次已删除' : '已勾选 · 空文件夹'}</div>
        </div>
        <div style={{ width: 1, height: 42, background: 'var(--border-soft)' }} />
        <div>
          <div className='sum-num' style={{ fontSize: 20, color: 'var(--muted)' }}>
            {items.length}<span style={{ fontSize: 14, marginLeft: 3 }}>个</span>
          </div>
          <div className='sum-label'>扫描到空文件夹</div>
        </div>
        <div className='sum-spacer' />
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <button className='btn btn-ghost' onClick={start} disabled={isCleaning || chosen.size === 0}>
            <RefreshCw size={14} /> 重新扫描
          </button>
          <button
            className='btn btn-ghost'
            onClick={() => setSel(allOn ? new Set() : new Set(items.map((i) => i.path)))}
            disabled={isCleaning || items.length === 0}
          >
            {allOn ? '取消全选' : '全选'}
          </button>
          <button
            className='btn btn-danger'
            disabled={selCount === 0 || isCleaning}
            onClick={doDelete}
            title={selCount === 0 ? '先勾选要删除的空文件夹' : '删除已勾选的空文件夹'}
          >
            <Trash2 size={15} /> 删除选中 {selCount > 0 ? `(${selCount})` : ''}
          </button>
        </div>
      </div>

      {isCleaning && (
        <div style={{ marginBottom: 14 }}>
          <div className='progress-track' style={{ height: 6 }}>
            <div className='progress-fill' style={{ width: `${pct}%` }} />
          </div>
          <div className='status-line' style={{ marginTop: 6, gap: 8 }}>
            <Loader2 size={13} className='spin' />
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1, minWidth: 0 }}>
              {msg || '正在删除…'}
            </span>
          </div>
        </div>
      )}

      {isDone && (
        <div
          className='panel'
          style={{
            marginBottom: 14, padding: '12px 16px', display: 'flex', flexDirection: 'row',
            alignItems: 'center', flexWrap: 'wrap', columnGap: 18, rowGap: 6, overflow: 'visible',
            borderColor: 'rgba(22,163,74,.35)', background: 'rgba(22,163,74,.06)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: 'var(--success)', fontWeight: 600, fontSize: 13.5 }}>
            <Check size={16} /> 清理完成
          </div>
          <div style={{ fontSize: 12.5 }}>
            已删除 <b className='mono' style={{ color: 'var(--success)' }}>{removed}</b> 个空文件夹
            {failed > 0 && (
              <span style={{ color: 'var(--warn)' }}> · {failed} 个已不再为空，已跳过</span>
            )}
          </div>
        </div>
      )}

      {!isDone && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12, fontSize: 11.5, color: 'var(--meta)' }}>
          <Info size={13} />
          默认不勾选；勾选后点右侧「删除选中」。只删完全空的目录，含任何文件的目录会自动跳过。
        </div>
      )}

      {items.length === 0 ? (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', color: 'var(--meta)', fontSize: 13 }}>
          {isDone ? '已清空扫描结果中的空文件夹' : '未发现空文件夹（所选盘符已经很整洁）'}
        </div>
      ) : (
        <div className='panel' style={{ flex: 1, minHeight: 0, overflow: 'auto', padding: 6 }}>
          {items.map((i) => {
            const on = sel.has(i.path)
            return (
              <button
                key={i.path}
                className='check-row'
                onClick={() => toggle(i.path)}
                title={i.path}
                style={{ paddingLeft: 10 }}
              >
                <span className={`checkbox${on ? ' on' : ''}`}>
                  {on && <Check size={12} />}
                </span>
                <div className='c-name'>
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                    <FolderMinus size={12} style={{ color: 'var(--meta)' }} />
                    {i.name}
                    <span className='tag tag-ssd'>第 {i.depth} 层</span>
                  </span>
                  <div className='c-path' style={{ marginTop: 1 }}>{i.path}</div>
                </div>
                <div style={{ fontSize: 10.5, color: 'var(--meta)', flexShrink: 0 }}>{fmtDate(i.mtime)}</div>
              </button>
            )
          })}
        </div>
      )}

      {failed > 0 && isDone && (
        <div className='status-line' style={{ marginTop: 8, color: 'var(--warn)' }}>
          <AlertTriangle size={13} /> 有目录在扫描后又被写入内容，已按安全策略跳过
        </div>
      )}
    </div>
  )
}
