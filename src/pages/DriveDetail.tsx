import { useEffect, useMemo, useState } from 'react'
import type { MouseEvent as RMouseEvent } from 'react'
import { ChevronRight, Folder, File as FileIcon, CornerLeftUp, RefreshCw, Square, AlertTriangle, X } from 'lucide-react'
import type { Volume, FileNode } from '../types'
import { peekDriveTree, revealInExplorer } from '../services/api'
import { showContextMenu } from '../services/contextMenu'
import { startScan, stopScan, isTaskRunning, getScanTask, markScanSeen, useScanTask } from '../services/scanTasks'
import { fmtBytes } from '../utils/format'
import { ScanWaitCard } from '../components/ScanTask'
import { useAiPageContext } from '../services/aiContext'
import { WhatIsThis } from '../components/WhatIsThis'

/** 拼绝对路径：盘根已带反斜杠，子项之间补一个 */
function joinPath(base: string, name: string): string {
  return base.endsWith('\\') ? `${base}${name}` : `${base}\\${name}`
}

export function DriveDetail({ volume, onBack, onGotoClean }: { volume: Volume; onBack: () => void; onGotoClean?: () => void }) {
  const letter = volume.letter.toUpperCase()
  // 扫描任务与组件生命周期解耦：离开本页不会中断，回到本页直接接上（含已完成的结果）
  const task = useScanTask(letter)

  const [path, setPath] = useState<string[]>([])
  const [picked, setPicked] = useState<FileNode | null>(null)

  useEffect(() => {
    setPath([])
    setPicked(null)
    // 已有任务结果 / 任务在跑 / api 缓存命中 → 都不重复扫盘
    const t = getScanTask(letter)
    const hasResult = !!t?.tree
    if (!isTaskRunning(t) && !hasResult && !peekDriveTree(letter)) startScan(letter, false)
  }, [letter])

  // 结果来源优先级：任务结果（含重新扫描时的旧结果）> api 层缓存（5 分钟内）
  const root = useMemo<FileNode | null>(() => {
    if (task?.tree) return task.tree
    return peekDriveTree(letter)
  }, [task?.tree, letter])

  const running = isTaskRunning(task)

  // 结果被看到后清掉总览页的「已完成」提示
  useEffect(() => {
    if (root && task?.status === 'done' && !task.seen) markScanSeen(letter)
  }, [root, task?.status, task?.seen, letter])

  const cur = useMemo(() => {
    if (!root) return null
    if (path.length === 0) return root
    let node: FileNode = root
    for (const seg of path) {
      const next = node.children?.find((c) => c.name === seg)
      if (!next) return root
      node = next
    }
    return node
  }, [root, path])

  const curPath = [`${volume.letter}:\\`, ...path]
  /** 当前浏览到的绝对路径（盘根形如 `C:\`） */
  const absPath = path.length === 0
    ? `${volume.letter.toUpperCase()}:\\`
    : `${volume.letter.toUpperCase()}:\\${path.join('\\')}`

  const listItems = useMemo(
    () => (cur ? [...(cur.children ?? [])].sort((a, b) => b.size - a.size) : []),
    [cur],
  )

  /**
   * 给 AI 的页面摘要：当前在哪一层、这层最大的几项。
   * 只放路径与体积 —— 这些用户本来就在屏幕上看着，传出去不增加暴露面。
   */
  const aiSummary = useMemo(() => {
    if (!cur) return ''
    const kids = listItems.filter((n) => !n.name.startsWith('（其余 ')).slice(0, 5)
    const lines = [`当前目录：${absPath}，共 ${fmtBytes(cur.size, 1)}`]
    if (kids.length > 0) {
      lines.push('这一层最大的几项：')
      kids.forEach((n, i) => lines.push(`${i + 1}. ${n.name}，${fmtBytes(n.size, 1)}`))
    }
    return lines.join('\n').slice(0, 500)
    // absPath / listItems 都由 cur 与 path 派生，依赖它们即可
  }, [cur, listItems, absPath])
  useAiPageContext('drive', aiSummary)

  // ⚠️ 硬约束：下面这个早退之前不允许再出现任何 Hook。
  // 首帧扫描还没结果时走早退（Hook 少），扫描完成后走完整渲染（Hook 多），
  // 一旦把 Hook 写在早退后面，React 会抛 #310「渲染时调用的 Hook 比上次多」，
  // 整页直接变成「此页面渲染出错」，只能重载恢复。
  if (!root || !cur) {
    return (
      <ScanWaitCard
        volume={volume}
        task={task}
        onBack={onBack}
        onRetry={() => startScan(letter, true)}
      />
    )
  }

  const goUp = () => setPath((p) => p.slice(0, -1))
  const canOpen = (n: FileNode) => n.kind === 'dir' && !!n.children && n.children.length > 0

  const enter = (name: string) => {
    setPicked(null)
    setPath((p) => [...p, name])
  }

  /** 目录/文件行的右键菜单：全部是原生动作，没有一项来自浏览器 */
  const onRowMenu = (e: RMouseEvent, node: FileNode) => {
    e.preventDefault()
    const target = joinPath(absPath, node.name)
    const isDir = node.kind === 'dir'
    showContextMenu(
      [
        isDir
          ? { type: 'item', label: '打开这一层', onClick: () => enter(node.name) }
          : { type: 'item', label: '这是什么', onClick: () => setPicked(node) },
        { type: 'sep' },
        {
          type: 'item',
          label: '复制路径',
          onClick: () => {
            void navigator.clipboard.writeText(target).catch(() => {
              alert(`无法访问剪贴板，路径如下：\n${target}`)
            })
          },
        },
        { type: 'item', label: '在资源管理器中打开', onClick: () => void revealInExplorer(target) },
      ],
      e.clientX,
      e.clientY,
    )
  }

  return (
    <div className="detail-layout page-enter">
      {/* 左：目录树 */}
      <div className="panel">
        <div className="panel-head">
          目录树
          <span className="count">{listItems.length} 项</span>
        </div>
        <div className="panel-body">
          {path.length > 0 && (
            <button className="folder-row" onClick={goUp}>
              <span className="f-icon"><CornerLeftUp size={15} /></span>
              <span className="f-name" style={{ color: 'var(--accent)' }}>返回上级</span>
            </button>
          )}
          {listItems.map((c) => {
            const pct = cur.size > 0 ? (c.size / cur.size) * 100 : 0
            return (
              <button
                key={c.name}
                className={`folder-row${picked === c ? ' active' : ''}`}
                onClick={() => (canOpen(c) ? enter(c.name) : setPicked(c))}
                onContextMenu={(e) => onRowMenu(e, c)}
                title={c.kind === 'dir' && !canOpen(c) ? '已到展示深度上限（体积已完整统计）' : undefined}
              >
                <span className={`f-icon${c.kind === 'file' ? ' file' : ''}`}>
                  {c.kind === 'dir' ? <Folder size={15} /> : <FileIcon size={15} />}
                </span>
                <span className="f-name">{c.name}</span>
                <span className="f-size">{fmtBytes(c.size, 1)}</span>
                <span className="f-bar" style={{ width: `${Math.max(2, pct)}%` }} />
              </button>
            )
          })}
        </div>
      </div>

      {/* 右：Treemap 主视图 */}
      <div className="panel">
        <div className="panel-head" style={{ flexWrap: 'wrap' }}>
          <button className="btn btn-ghost" style={{ padding: '4px 10px', fontSize: 12.5 }} onClick={onBack}>
            返回总览
          </button>
          <div className="breadcrumb">
            {curPath.map((seg, i) => (
              <span key={i} style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
                {i > 0 && <ChevronRight size={12} className="sep" />}
                {i === curPath.length - 1 ? (
                  <span className="cur">{seg}</span>
                ) : (
                  <button onClick={() => setPath(path.slice(0, i))}>{seg}</button>
                )}
              </span>
            ))}
          </div>
          <button
            className="btn btn-ghost"
            style={{ padding: '3px 10px', fontSize: 12, marginLeft: 'auto' }}
            onClick={() => startScan(letter, true)}
            disabled={running}
            title="重新扫描该盘（会重新遍历目录，约数秒到数十秒）"
          >
            <RefreshCw size={12} className={running ? 'spin' : undefined} />
            {running ? ' 正在扫描…' : ' 重新扫描'}
          </button>
          <span className="count">{fmtBytes(cur.size, 1)}</span>
        </div>

        {/* 右侧不再是树图，改成 AI 分析的展示位；内容可能超出，需要独立滚动 */}
        <div className="ai-scroll">

        {/* 选中文件时给出明确出口：卡片此时看的是这个文件，不是当前目录 */}
        {picked && (
          <div className="what-picked">
            <FileIcon size={12} />
            <span className="what-picked-name" title={picked.name}>{picked.name}</span>
            <span className="what-picked-size">{fmtBytes(picked.size, 1)}</span>
            <button type="button" className="what-btn" onClick={() => setPicked(null)}>
              <X size={11} /> 回到当前目录
            </button>
          </div>
        )}

        <WhatIsThis
          path={picked ? joinPath(absPath, picked.name) : absPath}
          kind={picked ? 'file' : 'dir'}
          size={picked ? picked.size : cur.size}
          siblings={picked ? undefined : listItems}
          onGotoClean={onGotoClean}
        />

        {/* 重新扫描进行中：旧结果仍可浏览，同时给出进度与停止入口 */}
        {running && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0,
            padding: '6px 14px', background: 'var(--accent-soft)',
            borderBottom: '1px solid var(--border-soft)', fontSize: 12,
          }}>
            <span className="loading-spinner small" />
            <span style={{ fontWeight: 600, color: 'var(--accent)' }}>
              {task?.status === 'cancelling' ? '正在停止…' : '正在重新扫描'}
            </span>
            <span className="mono" style={{ fontSize: 11.5, color: 'var(--meta)' }}>
              已遍历 {(task?.entries ?? 0).toLocaleString('zh-CN')} 项
            </span>
            {task?.current && (
              <span className="mono" style={{
                fontSize: 11.5, color: 'var(--meta)', maxWidth: 240, minWidth: 0,
                overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
              }} title={task.current}>{task.current}</span>
            )}
            <span style={{ flex: 1 }} />
            <span style={{ fontSize: 11.5, color: 'var(--meta)' }}>下方为上一次结果</span>
            <button
              className="btn btn-ghost"
              style={{ padding: '2px 9px', fontSize: 11.5 }}
              onClick={() => stopScan(letter)}
              disabled={task?.status === 'cancelling'}
            >
              <Square size={11} /> 停止
            </button>
          </div>
        )}

        {/* 重新扫描被停止 / 失败：旧结果仍在屏幕上，必须说明「现在看到的是旧的」 */}
        {!running && (task?.status === 'stopped' || task?.status === 'error') && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0,
            padding: '6px 14px',
            background: task.status === 'error' ? 'rgba(220,38,38,.06)' : 'var(--surface-warm)',
            borderBottom: '1px solid var(--border-soft)', fontSize: 12,
          }}>
            <AlertTriangle size={13} color={task.status === 'error' ? 'var(--danger)' : 'var(--warn)'} />
            <span style={{ flex: 1, minWidth: 0, color: 'var(--fg-2)', wordBreak: 'break-word' }}>
              {task.status === 'error'
                ? `重新扫描失败：${task.error ?? '未知错误'}`
                : '重新扫描已停止'}　下方仍是上一次扫描的结果
            </span>
            <button
              className="btn btn-ghost"
              style={{ padding: '2px 9px', fontSize: 11.5 }}
              onClick={() => startScan(letter, true)}
            >
              <RefreshCw size={11} /> 重新扫描
            </button>
          </div>
        )}
        </div>
      </div>
    </div>
  )
}
