/**
 * 更新日志：直接读取仓库根目录的 CHANGELOG.md。
 *
 * 用 `?raw` 在构建时把文件内容内联进 bundle，因此**不需要联网也能看**——
 * 这比运行时请求 GitHub API 更可靠（离线、无 CORS、无速率限制），
 * 代价是发版时必须把说明写进 CHANGELOG.md（release.py 已自动完成这一步）。
 */
import raw from '../../CHANGELOG.md?raw'

export interface ChangelogGroup {
  /** 分组名，如「新增」「修复」 */
  title: string
  items: string[]
}

export interface ChangelogEntry {
  version: string
  date: string
  groups: ChangelogGroup[]
}

/**
 * 解析约定格式（keep-a-changelog 的简化版）：
 *
 *   ## 1.2.0 · 2026-10-18
 *   ### 修复
 *   - 某条改动
 */
export function parseChangelog(md: string): ChangelogEntry[] {
  const lines = md.split(/\r?\n/)
  const entries: ChangelogEntry[] = []
  let cur: ChangelogEntry | null = null
  let group: ChangelogGroup | null = null

  for (const rawLine of lines) {
    const line = rawLine.trimEnd()

    // 版本标题：## <版本> · <日期>
    const mVer = /^##\s+(.+?)\s*(?:[·|]\s*(.+))?$/.exec(line)
    if (mVer) {
      const version = mVer[1].trim()
      // 跳过文件顶部的「# 更新日志」之类的一级标题已由 ^## 排除
      cur = { version, date: (mVer[2] ?? '').trim(), groups: [] }
      entries.push(cur)
      group = null
      continue
    }

    if (!cur) continue

    // 分组标题：### 新增 / ### 修复
    const mGroup = /^###\s+(.+)$/.exec(line)
    if (mGroup) {
      group = { title: mGroup[1].trim(), items: [] }
      cur.groups.push(group)
      continue
    }

    // 列表项
    const mItem = /^[-*]\s+(.+)$/.exec(line.trim())
    if (mItem) {
      const text = mItem[1].replace(/\*\*/g, '').trim()
      if (!group) {
        group = { title: '', items: [] }
        cur.groups.push(group)
      }
      group.items.push(text)
    }
  }

  // 丢掉解析为空的结构（比如标题下面什么都没有）
  return entries.filter((e) => e.groups.some((g) => g.items.length > 0))
}

export const CHANGELOG: ChangelogEntry[] = parseChangelog(raw)

/** 取某个版本号的条目，找不到返回 undefined */
export function findEntry(version: string): ChangelogEntry | undefined {
  const norm = (v: string) => v.replace(/^v/i, '')
  return CHANGELOG.find((e) => norm(e.version) === norm(version))
}
