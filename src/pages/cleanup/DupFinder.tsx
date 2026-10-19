import { useState } from 'react'
import {
  Copy, ScanSearch, Trash2, Loader2, Check, RefreshCw, AlertTriangle, Info, Square,
} from 'lucide-react'
import type { Volume, DupReport } from '../../types'
import { findDuplicates, cancelDupes, deleteDuplicates, peekVolumes } from '../../services/api'
import { fmtBytes } from '../../utils/format'
import { useConfirm } from '../../components/ConfirmDialog'
import { DrivePicker } from './DrivePicker'
import { EmptyArtIcon } from './EmptyArtIcon'

/**
 * 重复文件查找
 *
 * 与「垃圾扫描」「安装包」「空文件夹」的区别：这里**比较的是内容**，
 * 结果天然成组。三条硬规则：
 *   1. **每组至少保留一份**：最早修改的那份固定为「保留」，不可勾选；
 *   2. **默认一个都不勾**：重复文件里经常有"我以为删掉的那份才是原件"的情况；
 *   3. **系统目录不扫也不删**：后端扫描时跳过、删除时拒绝，双保险。
 *
 * 为什么不做"一键全选多余的"作为默认：代价太大。用户误删一份 1GB 素材，
 * 找回来要几小时，而多勾几个只是多点几下。
 */
type Phase = 'idle' | 'scanning' | 'ready'

function fmtDate(secs: number): string {
  if (!secs) return '—'
  const d = new Date(secs * 1000)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** 单次最多渲染的组数：几万组时全量渲染会让滚动卡死 */
const RENDER_LIMIT = 200

export function DupFinder() {
  const [drives] = useState<Volume[]>(() => peekVolumes() ?? [])
  const [chosen, setChosen] = useState<Set<string>>(
    () => new Set((peekVolumes() ?? []).map((v) => v.letter)),
  )
  const [skipSystem, setSkipSystem] = useState(true)
  const [phase, setPhase] = useState<Phase>('idle')
  const [prog, setProg] = useState({ phase: '', scanned: 0, groups: 0 })
  const [report, setReport] = useState<DupReport | null>(null)
  const [sel, setSel] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)
  const { ask, node: confirmNode } = useConfirm()

  const toggleDrive = (letter: string) => {
    setChosen((s) => {
      const n = new Set(s)
      if (n.has(letter)) n.delete(letter)
      else n.add(letter)
      return n
    })
  }

  const start = async () => {
    if (phase === 'scanning' || chosen.size === 0) return
    setPhase('scanning')
    setMsg(null)
    setSel(new Set())
    setProg({ phase: '正在列出文件…', scanned: 0, groups: 0 })
    try {
      const roots = [...chosen].sort().map((l) => `${l}:\\`)
      const r = await findDuplicates(roots, skipSystem, (p) => setProg(p))
      setReport(r)
      setPhase('ready')
      if (r.cancelled) setMsg('已停止，下面只包含停止前已完成比对的部分')
    } catch (e) {
      setPhase('idle')
      setMsg(String((e as { message?: string })?.message ?? e))
    }
  }

  const stop = async () => {
    await cancelDupes()
  }

  const selSize = (() => {
    if (!report) return 0
    let n = 0
    for (const g of report.groups) for (const f of g.files) if (sel.has(f.path)) n += f.size
    return n
  })()

  /** 每组保留第一份（后端已按修改时间升序），其余全选 */
  const selectExtras = () => {
    if (!report) return
    const next = new Set<string>()
    for (const g of report.groups) {
      g.files.slice(1).forEach((f) => next.add(f.path))
    }
    setSel(next)
  }

  const doDelete = async () => {
    if (!report || sel.size === 0 || busy) return
    const paths = [...sel]
    const ok = await ask({
      title: `删除 ${paths.length} 个重复文件`,
      stats: [
        { label: '数量', value: `${paths.length} 个` },
        { label: '预计释放', value: fmtBytes(selSize, 2), strong: true },
      ],
      warning: '删除不进回收站、不可恢复。',
      note: '每组都保留了一份（标记为「保留」的那份），请确认它所在的位置合适。',
      confirmText: '永久删除',
    })
    if (!ok) return
    setBusy(true)
    setMsg(null)
    try {
      const r = await deleteDuplicates(paths)
      const gone = new Set(paths)
      // 删除后要重算每组"可释放"：删掉一份，剩余份数变了，数字不能沿用旧的
      const left = report.groups
        .map((g) => {
          const files = g.files.filter((f) => !gone.has(f.path))
          return { ...g, files, wasted: g.size * Math.max(0, files.length - 1) }
        })
        .filter((g) => g.files.length > 1)
      setReport({ ...report, groups: left, wasted: left.reduce((a, g) => a + g.wasted, 0) })
      setSel(new Set())
      const parts = [`已释放 ${fmtBytes(r.freed, 2)}`]
      if (r.failed > 0) parts.push(`${r.failed} 个被占用或权限不足`)
      if (r.pendingReboot > 0) parts.push(`${r.pendingReboot} 个需重启后生效`)
      if (r.refused.length > 0) parts.push(`${r.refused.length} 个因位于系统目录被拒绝`)
      setMsg(parts.join(' · '))
    } catch (e) {
      setMsg(String((e as { message?: string })?.message ?? e))
    } finally {
      setBusy(false)
    }
  }

  // ---- 扫描中 ----
  if (phase === 'scanning') {
    return (
      <>
      <div className='panel' style={{ margin: 'auto', maxWidth: 460, textAlign: 'center', padding: '34px 28px' }}>
        <Loader2 size={26} className='spin' color='var(--accent)' />
        <div style={{ marginTop: 12, fontSize: 14, fontWeight: 600 }}>正在查找重复文件</div>
        <div style={{ marginTop: 6, fontSize: 12, color: 'var(--muted)', minHeight: 18 }}>
          {prog.phase || '准备中…'}
        </div>
        <div style={{ marginTop: 4, fontSize: 11.5, color: 'var(--meta)' }}>
          已检查 {prog.scanned.toLocaleString()} 个文件 · 已确认 {prog.groups} 组
        </div>
        <button
          className='btn btn-ghost'
          style={{ marginTop: 16, padding: '6px 14px' }}
          onClick={() => void stop()}
        >
          <Square size={12} /> 停止
        </button>
        <div style={{ marginTop: 14, fontSize: 11, color: 'var(--meta)', lineHeight: 1.7 }}>
          先按大小分组，再逐字节比对内容。只读扫描，不会改动任何文件。
        </div>
      </div>
      {confirmNode}
      </>
    )
  }

  // ---- 未扫描 ----
  if (!report) {
    return (
      <>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <div className='summary-bar'>
          <div>
            <div className='sum-num' style={{ fontSize: 20, color: 'var(--muted)' }}>—</div>
            <div className='sum-label'>尚未扫描</div>
          </div>
          <div className='sum-spacer' />
          <button className='btn btn-primary' onClick={() => void start()} disabled={chosen.size === 0}>
            <ScanSearch size={15} /> 开始查找
          </button>
        </div>

        <div className='panel' style={{ padding: '18px 20px', marginBottom: 14 }}>
          <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 10 }}>扫描范围</div>
          <DrivePicker drives={drives} selected={chosen} onToggle={toggleDrive} disabled={false} />
          <label style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 12, fontSize: 12.5 }}>
            <input
              type='checkbox'
              checked={skipSystem}
              onChange={(e) => setSkipSystem(e.target.checked)}
            />
            跳过系统目录（Windows、Program Files、ProgramData\Microsoft）
            <span style={{ color: 'var(--meta)', fontSize: 11 }}>推荐</span>
          </label>
        </div>

        {msg && (
          <div className='status-line' style={{ color: 'var(--warn)', marginBottom: 10 }}>
            <AlertTriangle size={13} /> {msg}
          </div>
        )}

        <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--meta)' }}>
          <div style={{ textAlign: 'center' }}>
            <EmptyArtIcon icon={Copy} />
            <div style={{ marginTop: 10, fontSize: 13 }}>只统计 1MB 以上的文件</div>
            <div style={{ marginTop: 4, fontSize: 11.5 }}>大小不同的一定不是重复，所以先比大小再比内容</div>
          </div>
        </div>
      </div>
      {confirmNode}
      </>
    )
  }

  // ---- 结果 ----
  const groups = report.groups.slice(0, RENDER_LIMIT)
  return (
    <>
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div className='summary-bar'>
        <div>
          <div className='sum-num'>{fmtBytes(report.wasted, 2)}</div>
          <div className='sum-label'>全部重复可释放</div>
        </div>
        <div style={{ width: 1, height: 42, background: 'var(--border-soft)' }} />
        <div>
          <div className='sum-num' style={{ fontSize: 20, color: 'var(--muted)' }}>
            {report.groups.length}
          </div>
          <div className='sum-label'>组重复 · 检查 {report.scanned.toLocaleString()} 个文件</div>
        </div>
        <div className='sum-spacer' />
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <button className='btn btn-ghost' onClick={() => void start()} disabled={busy}>
            <RefreshCw size={14} /> 重新扫描
          </button>
          <button className='btn btn-ghost' onClick={selectExtras} disabled={busy || report.groups.length === 0}>
            选中每组多余的
          </button>
          <button
            className='btn btn-danger'
            disabled={sel.size === 0 || busy}
            onClick={() => void doDelete()}
            title={sel.size === 0 ? '先勾选要删除的副本' : '永久删除已勾选的副本'}
          >
            {busy ? <Loader2 size={14} className='spin' /> : <Trash2 size={15} />}
            删除选中 {sel.size > 0 ? `(${sel.size})` : ''}
          </button>
        </div>
      </div>

      {msg && (
        <div
          className='panel'
          style={{
            marginBottom: 12, padding: '10px 14px', display: 'flex', alignItems: 'center', gap: 8,
            fontSize: 12.5, borderColor: 'rgba(22,163,74,.3)', background: 'rgba(22,163,74,.06)',
            color: 'var(--success)', overflow: 'visible',
          }}
        >
          <Check size={14} /> {msg}
        </div>
      )}

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10, fontSize: 11.5, color: 'var(--meta)' }}>
        <Info size={13} />
        每组默认保留修改时间最早的一份（标「保留」，不可勾选）。已勾选 {fmtBytes(selSize, 2)}。
        {report.groups.length > RENDER_LIMIT && ` 仅显示前 ${RENDER_LIMIT} 组，删完可重新扫描查看剩余。`}
      </div>

      <div style={{ flex: 1, minHeight: 0, overflowY: 'auto', paddingRight: 4 }}>
        {groups.map((g, gi) => (
          <div key={gi} className='panel' style={{ marginBottom: 10, overflow: 'hidden' }}>
            <div
              style={{
                display: 'flex', alignItems: 'center', gap: 10, padding: '9px 14px',
                background: 'var(--surface-warm)', fontSize: 12.5,
              }}
            >
              <Copy size={13} color='var(--muted)' />
              <span style={{ fontWeight: 600 }}>{g.files.length} 份相同</span>
              <span className='mono' style={{ color: 'var(--muted)' }}>
                每份 {fmtBytes(g.size, 2)}
              </span>
              <span className='mono' style={{ color: 'var(--accent)' }}>
                可释放 {fmtBytes(g.wasted, 2)}
              </span>
            </div>
            {g.files.map((f, fi) => {
              const keep = fi === 0
              const checked = sel.has(f.path)
              return (
                <label
                  key={f.path}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 10, padding: '8px 14px',
                    borderTop: '1px solid var(--border-soft)', cursor: keep ? 'default' : 'pointer',
                    background: checked ? 'var(--accent-soft)' : 'transparent',
                  }}
                >
                  <input
                    type='checkbox'
                    checked={keep || checked}
                    disabled={keep || busy}
                    onChange={() =>
                      setSel((s) => {
                        const n = new Set(s)
                        if (n.has(f.path)) n.delete(f.path)
                        else n.add(f.path)
                        return n
                      })
                    }
                  />
                  <span
                    style={{
                      flex: 1, minWidth: 0, fontSize: 12,
                      overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                      direction: 'rtl', textAlign: 'left',
                    }}
                    title={f.path}
                  >
                    {f.path}
                  </span>
                  {keep && (
                    <span
                      style={{
                        fontSize: 10.5, color: 'var(--success)', border: '1px solid rgba(22,163,74,.35)',
                        borderRadius: 4, padding: '0 5px', flexShrink: 0,
                      }}
                    >
                      保留
                    </span>
                  )}
                  <span className='mono' style={{ fontSize: 11, color: 'var(--meta)', flexShrink: 0 }}>
                    {fmtDate(f.mtime)}
                  </span>
                </label>
              )
            })}
          </div>
        ))}
      </div>
    </div>
    {confirmNode}
    </>
  )
}
