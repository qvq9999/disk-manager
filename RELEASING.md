# 发版约定

> 用户约定：**每次修复成功后都发一版，版本号 +0.1**（1.1.0 → 1.2.0 → …）。

## 一条命令发版

```bash
export TAURI_SIGNING_PRIVATE_KEY=C:/Users/Administrator/.tauri/disk-manager.key
export GH_TOKEN=<你的 PAT>
python scripts/release.py --bump --notes "本次修了什么"
```

它会依次完成：版本号 +0.1（六处同步）→ 带签名构建 → 生成 `latest.json` →
commit + 打 tag → push → 创建 GitHub Release → 上传安装包 / .sig / latest.json。

演练（只改版本并本地构建，不推送）：

```bash
python scripts/release.py --bump --notes "..." --no-push
```

只打印计划、不动任何文件：

```bash
python scripts/release.py --bump --notes "..." --dry-run
```

## 每次发版前必查

1. **编译与测试**：`npx tsc --noEmit`、`cargo test --lib` 全绿
2. **签名生效**：构建输出里应有 `Finished 1 updater signature at: ...exe.sig`
3. **隐私**：`grep -rIn "Administrator\|Users\\\\" src/ src-tauri/src/` 不应命中本机用户名
4. **版本一致**：`package.json` / `Cargo.toml` / `tauri.conf.json` / `api.ts` / `App.tsx` 五处相同

## 两条硬约束

**仓库必须 public。** 私有仓库的 Release 资产需要认证才能下载，客户端拿不到 `latest.json`，
自动更新会直接失效。

**私钥必须备份且保密。** 公钥已编译进程序，只有配套私钥签出的包才会被接受。
私钥丢失后，已经发出去的旧版本将永远收不到新更新 —— 只能让用户手动下载重装。

私钥位置：`C:/Users/Administrator/.tauri/disk-manager.key`（**不要提交，不要外传**）。

## 首次配置（已完成）

- 密钥对已生成，公钥写入 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`
- 更新源：`https://github.com/qvq9999/disk-manager/releases/latest/download/latest.json`
  （`tauri.conf.json` 的 `plugins.updater.endpoints[0]` 与 `src-tauri/src/commands/update.rs`
  的 `ENDPOINT` 常量，**两处必须一致**）
- 仓库：https://github.com/qvq9999/disk-manager （**public**）

## 本机网络环境的三个坑

这三条是 `scripts/release.py` 里已经处理掉的，换机器可能不需要，但记录下来免得再踩：

1. **`insteadOf` 会把 push 打向 ghproxy**
   全局 gitconfig 有 `url.'https://ghproxy.net/https://github.com/'.insteadOf`。
   ghproxy 是只读加速代理，push 会报 `could not read Username for 'https://ghproxy.net'`。
   → 把 token 写进推送 URL（前缀不再匹配 `https://github.com/`）即可绕过。

2. **直连 github.com 时证书吊销检查失败**
   报 `schannel: CRYPT_E_NO_REVOCATION_CHECK`（代理 MITM 证书无法查吊销）。
   `http.schannelCheckRevoke=false` 与 `http.sslBackend=openssl` 都无效
   （后者改报 `unable to get local issuer certificate`）。
   → 仅推送时用 `GIT_SSL_NO_VERIFY=true`，不写入任何持久配置。

3. **urllib 上传资产会 502**
   Python 走本地代理对 `uploads.github.com` 的 CONNECT 隧道返回 502，
   而 curl 走同一代理正常（实测 3.7MB 安装包 2.8 秒完成）。
   → 资产上传用 curl，API 调用仍用 urllib（正常）。

