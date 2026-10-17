/**
 * 自动更新（检查 + 下载安装）
 *
 * 说明一个概念差异：桌面应用做不到真正的「热更新」（不重启替换正在运行的代码）。
 * 这里实现的是业界通行的**自动更新**：检查新版本 → 下载带签名的安装包 → 运行安装器覆盖安装 → 重启。
 *
 * 检查走 tauri-plugin-updater：它请求 GitHub Release 上的 `latest.json`，
 * 用编译进程序的公钥校验签名，签名不匹配的包一律拒绝 —— 避免有人伪造更新源投毒。
 */
import { isTauri } from './api'

export interface UpdateCheckResult {
  /** 是否有可用新版本 */
  available: boolean
  /** 当前版本 */
  current: string
  /** 新版本号（available 为真时有值） */
  latest?: string
  /** 更新说明 */
  notes?: string
  /** 发布日期 */
  date?: string
  /** 更新源是否已配置（tauri.conf.json 里还是 OWNER/REPO 占位符时为 false） */
  configured: boolean
  /** 失败原因（网络、签名、404 等） */
  error?: string
}

/** 模块级缓存：启动时检查一次，设置页直接复用结果，不必每次进入都请求 */
let cached: UpdateCheckResult | null = null
/** 已发现的更新对象，供「立即更新」复用（避免二次 check） */
let pendingUpdate: unknown = null
/** 进行中的请求：启动检查和设置页可能同时触发，复用同一个 Promise 避免重复请求 */
let inflight: Promise<UpdateCheckResult> | null = null
const listeners = new Set<(r: UpdateCheckResult | null) => void>()

export function subscribeUpdate(fn: (r: UpdateCheckResult | null) => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

export function latestUpdate(): UpdateCheckResult | null {
  return cached
}

function emit(r: UpdateCheckResult | null) {
  cached = r
  listeners.forEach((fn) => {
    try {
      fn(r)
    } catch {
      /* 单个订阅者出错不应影响其他 */
    }
  })
}

/** 更新源地址（用于设置页展示；与实际请求同源，见 tauri.conf.json） */
export async function getUpdateEndpoint(): Promise<string> {
  if (!isTauri) return 'https://github.com/OWNER/REPO/releases/latest/download/latest.json'
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<string>('update_endpoint')
  } catch {
    return ''
  }
}

/**
 * 检查更新。
 * @param force 忽略缓存重新请求（用户手动点「检查更新」时传 true）
 */
export async function checkForUpdate(force = false): Promise<UpdateCheckResult> {
  if (!isTauri) {
    // 浏览器预览：?update=1 时伪造一个「发现新版本」，便于验证更新卡片的渲染
    const fake = typeof location !== 'undefined' && new URLSearchParams(location.search).has('update')
    const r: UpdateCheckResult = fake
      ? {
          available: true,
          current: '1.0.0',
          latest: '1.0.1',
          notes: '· 修复了卸载时卡住不返回的问题\n· 新增应用缓存深度扫描\n· 支持开机自启动',
          date: '2026-10-18T00:00:00Z',
          configured: true,
        }
      : {
          available: false,
          current: '1.0.0',
          configured: false,
          error: '预览环境不联网，实际版本请以已安装程序为准',
        }
    // 无论是否 force 都要广播：启动时的检查不是 force，
    // 若只在 force 时 emit，界面会一直停在「尚未检查」
    emit(r)
    return r
  }
  if (!force) {
    if (cached) return cached
    if (inflight) return inflight
  }
  inflight = doCheck()
  try {
    return await inflight
  } finally {
    inflight = null
  }
}

async function doCheck(): Promise<UpdateCheckResult> {
  const endpoint = await getUpdateEndpoint()
  const configured = !endpoint.includes('OWNER/REPO')

  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    const update = await check()
    if (!update) {
      const r: UpdateCheckResult = {
        available: false,
        current: await currentVersion(),
        configured,
      }
      emit(r)
      return r
    }
    pendingUpdate = update
    const r: UpdateCheckResult = {
      available: true,
      current: await currentVersion(),
      latest: update.version,
      notes: update.body ?? '',
      date: update.date ?? undefined,
      configured,
    }
    emit(r)
    return r
  } catch (e) {
    const msg = String(e)
    // 404 在「仓库没发版」和「endpoint 还是占位符」时都会出现，用 configured 区分文案
    const r: UpdateCheckResult = {
      available: false,
      current: await currentVersion(),
      configured,
      error: configured ? friendlyError(msg) : '尚未配置更新源（tauri.conf.json 里的 OWNER/REPO 未替换）',
    }
    emit(r)
    return r
  }
}

function friendlyError(msg: string): string {
  const m = msg.toLowerCase()
  if (m.includes('404') || m.includes('not found')) {
    return '更新源上没有找到 latest.json —— 确认仓库已发布版本，且 Release 里上传了 latest.json'
  }
  if (m.includes('signature') || m.includes('verify')) {
    return '更新包签名校验失败，已拒绝安装（可能是更新源被篡改，或公钥与私钥不匹配）'
  }
  if (m.includes('dns') || m.includes('connect') || m.includes('network') || m.includes('timeout')) {
    return '无法连接更新服务器，请检查网络后重试'
  }
  return msg
}

async function currentVersion(): Promise<string> {
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return await getVersion()
  } catch {
    return '1.0.0'
  }
}

export interface DownloadProgress {
  /** 已下载字节 */
  downloaded: number
  /** 总字节（未知时为 0） */
  total: number
  /** 0~100 */
  pct: number
}

/**
 * 下载并安装已发现的新版本。
 *
 * 安装器是 NSIS 的 passive 模式（显示进度条、无需交互），装完会启动新版本，
 * 本进程需要退出 —— 由调用方在完成后提示用户或直接退出。
 */
export async function installPendingUpdate(
  onProgress?: (p: DownloadProgress) => void,
): Promise<void> {
  if (!isTauri) throw new Error('预览环境不支持安装更新')
  if (!pendingUpdate) throw new Error('尚未检查到可用更新，请先检查更新')

  // 结构由插件定义，这里只用到 downloadAndInstall
  const update = pendingUpdate as {
    downloadAndInstall: (cb: (ev: unknown) => void) => Promise<void>
  }

  let total = 0
  let downloaded = 0
  await update.downloadAndInstall((ev: unknown) => {
    const e = ev as { event?: string; data?: { contentLength?: number; chunkLength?: number } }
    if (e.event === 'Started') {
      total = e.data?.contentLength ?? 0
    } else if (e.event === 'Progress') {
      downloaded += e.data?.chunkLength ?? 0
      onProgress?.({
        downloaded,
        total,
        pct: total > 0 ? Math.min(99, Math.round((downloaded / total) * 100)) : 0,
      })
    } else if (e.event === 'Finished') {
      onProgress?.({ downloaded: total, total, pct: 100 })
    }
  })
}

/** 清掉缓存（例如用户点了「稍后再说」） */
export function clearUpdateCache() {
  pendingUpdate = null
  emit(null)
}
