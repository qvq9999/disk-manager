// ============ Squarified Treemap 布局算法 ============
// 输入: 可用矩形 + 已排序的带权项；输出: 每项的矩形
export interface Rect { x: number; y: number; w: number; h: number }
export interface Placed<T> { item: T; rect: Rect }

export function squarify<T extends { size: number }>(
  items: T[],
  rect: Rect,
): Placed<T>[] {
  const out: Placed<T>[] = []
  const list = items
    .filter((i) => i.size > 0)
    .sort((a, b) => b.size - a.size)
  if (list.length === 0 || rect.w <= 0 || rect.h <= 0) return out

  const total = list.reduce((s, i) => s + i.size, 0)
  let { x, y, w, h } = rect
  const scale = (w * h) / total
  let rest = list.map((i) => ({ item: i, area: i.size * scale }))

  while (rest.length > 0) {
    const horizontal = w >= h // 沿短边排列一行
    const side = horizontal ? h : w
    // 贪心取行: 逐个尝试加入，直到长宽比变差
    let row: typeof rest = []
    let rowArea = 0
    let bestRatio = Infinity
    let i = 0
    while (i < rest.length) {
      const newArea = rowArea + rest[i].area
      const newSide = newArea / side
      const ratio = worst(row, newArea, side)
      const ratioNew = worst([...row, rest[i]], newArea, side)
      if (ratioNew > bestRatio && row.length > 0) break
      bestRatio = Math.min(ratioNew, bestRatio)
      row.push(rest[i])
      rowArea = newArea
      i++
    }
    const rowSide = rowArea / side
    let off = 0
    for (const r of row) {
      const len = r.area / rowSide
      out.push({
        item: r.item,
        rect: horizontal
          ? { x, y: y + off, w: rowSide, h: len }
          : { x: x + off, y, w: len, h: rowSide },
      })
      off += len
    }
    if (horizontal) { x += rowSide; w -= rowSide }
    else { y += rowSide; h -= rowSide }
    rest = rest.slice(row.length)
  }
  return out
}

function worst(row: { area: number }[], rowArea: number, side: number): number {
  if (row.length === 0) return Infinity
  const s = rowArea / side
  let max = 0
  for (const r of row) {
    const len = r.area / s
    const ratio = Math.max(s / len, len / s)
    if (ratio > max) max = ratio
  }
  return max
}
