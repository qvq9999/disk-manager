import { useEffect, useMemo, useState } from 'react'
import {
  Package, ScanSearch, Trash2, Loader2, Check, RefreshCw,
  AlertTriangle, Square, FolderOpen, Info,
} from 'lucide-react'
import type { InstallerItem, Volume } from '../../types'
import {
  scanInstallers, cancelInstallerScan, deleteInstallers, peekVolumes,
} from '../../services/api'
import { fmtBytes } from '../../utils/format'
import { useConfirm } from '../../components/ConfirmDialog'
import { DrivePicker } from './DrivePicker'
import { EmptyArtIcon } from './EmptyArtIcon'

/**
 * 安装包专项清理
 *
 * 与「垃圾扫描」的区别：这里列的是**用户自己下载的安装包文件**，
 * 每一项都需要用户自己勾选 —— 默认一个都不勾。安装包是不可再生的
 * （删了要重新下载几百 MB），所以不做任何「默认全选」的便利设计。
 *
 * 清理后不自动重扫：全盘扫描要几十秒，删完直接把已删项从列表移除，
 * 顶部给出「已释放多少」，需要最新结果时点「重新扫描」。
 */
type Phase = 'idle' | 'scanning' | 'ready' | 'cleaning' | 'done'

function fmtDate(secs: number): string {
  if (!secs) return '—'
  const d = new Date(secs * 1000)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export function InstallerClean() {
  const [drives, setDrives] = useState<Volume[]>(() => peekVolumes() ?? [])
  const [chosen, setChosen] = useState<Set<string>>(
    () => new Set((peekVolumes() ?? []).map((v) => v.letter)),
  )
  const [phase, setPhase] = useState<Phase>('idle')
  const [items, setItems] = useState<InstallerItem[]>([])
  const [sel, setSel] = useState<Set<string>>(new Set())
  const [pct, setPct] = useState(0)
  const [msg, setMsg] = useState('')
  const [freed, setFreed] = useState(0)
  const [failed, setFailed] = useState(0)
  const [doneCount, setDoneCount] = useState(0)

  // 卸载时取消后台扫描，避免切走页签后还在全盘遍历
  useEffect(() => () => { void cancelInstallerScan() }, [])

  // 删除前的二次确认（自绘弹窗）
  const { ask, node: confirmNode } = useConfirm()

  const totalSize = useMemo(() => items.reduce((s, i) => s + i.size, 0), [items])
  const selSize = useMemo(
    () => items.filter((i) => sel.has(i.path)).reduce((s, i) => s + i.size, 0),
    [items, sel],
  )
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
      const res = await scanInstallers(list, (p) => {
        setPct(p.pct)
        setMsg(p.current)
      })
      setItems(res)
      setSel(new Set())
      setPhase('ready')
    } catch (e) {
      const s = String(e)
      if (s.includes('INSTALLER_CANCELLED')) {
        setPhase('idle')
        setMsg('扫描已停止')
      } else {
        setPhase('idle')
        alert(`扫描失败：${s}`)
      }
    }
  }

  const stop = async () => {
    await cancelInstallerScan()
  }

  const doDelete = async () => {
    const paths = [...sel]
    if (paths.length === 0) return
    const ok = await ask({
      title: `删除 ${paths.length} 个安装包`,
      subtitle: '删除的是安装包文件本身，不会卸载已安装的软件',
      stats: [
        { label: '预计释放', value: fmtBytes(selSize, 2), strong: true },
        { label: '文件数量', value: `${paths.length} 个` },
      ],
      warning: '删除后无法恢复，以后重装对应软件需要重新下载。',
      confirmText: '删除安装包',
    })
    if (!ok) return
    setPhase('cleaning')
    setPct(0)
    try {
      const [f, fail] = await deleteInstallers(paths, (p) => {
        setPct(p.pct)
        setMsg(p.current)
      })
      setFreed(f)
      setFailed(fail)
      setDoneCount(paths.length)
      // 只移除已删项：失败的保留在列表里，用户能看出来没删掉
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
        <EmptyArtIcon icon={Package} />
        <div style={{ textAlign: 'center', maxWidth: 560 }}>
          <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 6 }}>扫描设备中的安装包文件</div>
          <div style={{ fontSize: 12.5, color: 'var(--muted)', lineHeight: 1.7 }}>
            识别 .msi / .msix / .appx / .msu 与文件名含 setup、install、安装 的 exe<br />
            卸载程序（unins*.exe）与系统目录不会被列入；扫描到的项目默认不勾选，由你决定删哪些
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
            <button className='btn btn-ghost' onClick={stop}>
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
              范围：{[...chosen].sort().map((l) => `${l}:`).join(' ')} · 需要遍历文件，盘越大越慢
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
            {fmtBytes(isDone ? freed : selSize, 2)}
          </div>
          <div className='sum-label'>{isDone ? '本次已释放' : '已勾选 · 永久删除'}</div>
        </div>
        <div style={{ width: 1, height: 42, background: 'var(--border-soft)' }} />
        <div>
          <div className='sum-num' style={{ fontSize: 20, color: 'var(--muted)' }}>{fmtBytes(totalSize, 2)}</div>
          <div className='sum-label'>共 {items.length} 个安装包</div>
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
            disabled={selSize === 0 || isCleaning}
            onClick={doDelete}
            title={sel.size === 0 ? '先勾选要删除的安装包' : '永久删除已勾选的安装包'}
          >
            <Trash2 size={15} /> 删除选中 {sel.size > 0 ? `(${sel.size})` : ''}
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
            已释放 <b className='mono' style={{ color: 'var(--success)' }}>{fmtBytes(freed, 2)}</b>
            <span style={{ color: 'var(--meta)' }}> · 处理 {doneCount} 个</span>
            {failed > 0 && (
              <span style={{ color: 'var(--warn)' }}> · {failed} 个被占用或权限不足已跳过</span>
            )}
          </div>
        </div>
      )}

      {!isDone && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12, fontSize: 11.5, color: 'var(--meta)' }}>
          <Info size={13} />
          默认不勾选任何项目；勾选后点右侧「删除选中」。删除只影响安装包文件，不会卸载已安装的软件。
        </div>
      )}

      {items.length === 0 ? (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', color: 'var(--meta)', fontSize: 13 }}>
          {isDone ? '已清空扫描结果中的安装包' : '未发现安装包（可能是扫描范围里确实没有，或文件名不含安装特征）'}
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
                    <FolderOpen size={12} style={{ color: 'var(--meta)' }} />
                    {i.name}
                    <span className='tag tag-ssd'>{i.kind}</span>
                  </span>
                  <div className='c-path' style={{ marginTop: 1 }}>{i.path}</div>
                </div>
                <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 2, flexShrink: 0 }}>
                  <div className='c-size'>{fmtBytes(i.size, 2)}</div>
                  <div style={{ fontSize: 10.5, color: 'var(--meta)' }}>{fmtDate(i.mtime)}</div>
                </div>
              </button>
            )
          })}
        </div>
      )}

      {failed > 0 && isDone && (
        <div className='status-line' style={{ marginTop: 8, color: 'var(--warn)' }}>
          <AlertTriangle size={13} /> 部分文件被正在运行的安装程序占用，关闭后重新扫描再试
        </div>
      )}

      {confirmNode}
    </div>
  )
}
