import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'

// 版本号只在 package.json 里维护一处，构建时注入到前端。
// 之前侧边栏写死 `v1.3`、预览 mock 写死 `1.3.1`，发版后永远不会变 —— 用户看到的版本是错的。
// release.py 会同步 package.json / tauri.conf.json / Cargo.toml 三处，所以这里读到的就是当前版本。
const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf-8')) as {
  version: string
  author: string
  homepage: string
}

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
    // 作者与仓库同理：只在 package.json 维护一处，避免界面/FAQ/AI 三处各写一份
    __APP_AUTHOR__: JSON.stringify(pkg.author),
    __APP_REPO__: JSON.stringify(pkg.homepage),
  },
  server: { port: 5173 },
  build: { outDir: 'dist', emptyOutDir: true },
})
