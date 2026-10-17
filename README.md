# Disk Manager · Windows 存储管理

一个面向 Windows 10 / 11 的磁盘存储管理工具：磁盘总览、空间分析、垃圾清理、软件卸载、硬件信息，一站式完成。

界面基于 Rust + Tauri 2 + React，安装包约 3.5 MB，内存占用低。

## 功能

**磁盘总览**：物理磁盘、分区容量、文件系统、健康状态一览。

**空间分析**：按目录递归统计体积，树形占比图，找得出现在最占空间的是什么。

**垃圾清理**：
- 70+ 条声明式清理规则（浏览器、通讯软件、游戏平台、开发缓存、系统垃圾等）
- **应用缓存深度扫描**：遍历应用目录按精确白名单识别 Electron / Chromium 系缓存，覆盖手工规则追不上的新应用
- 危险项（如 `Windows.old`）单独列出、默认不勾选，并给出后果说明
- 只删除缓存目录内的文件、保留目录本身，应用下次启动自动重建

**软件卸载**：
- 优先调用软件自带卸载程序，读 exe 二进制签名判定安装框架（Inno / NSIS / MSI），据此选择正确的静默参数
- 卸载后扫描残留：注册表、目录、快捷方式、开机启动、服务、计划任务、防火墙规则、PATH 变量、COM 关联
- 卸载器弹交互向导时自动改用轮询，界面不阻塞、可随时关闭
- 强制移除（卸载程序已损坏时的兜底）

**硬件信息**：CPU、内存、显卡、主板与 SMART 健康度（NVMe / SATA）。

**自动更新**：启动后静默检查 GitHub Release 新版本，发现后在设置页提示，可一键下载安装（安装包带签名校验）。

## 构建

需要 Node.js 20+、Rust stable、以及 MSVC 构建工具（Windows）。

```bash
npm install
npx tauri build --bundles nsis
```

产物在 `src-tauri/target/release/bundle/nsis/`。

> 如果你的 rustc 报 `link.exe not found`，参考 `dev-env.example.sh` 里的做法指定 MSVC 工具链。

## 发布新版本（含自动更新清单）

自动更新依赖三样东西：安装包、它的签名、以及一份 `latest.json` 清单。

```bash
# 1. 签名私钥（首次生成一次，务必备份且不要外传）
npx tauri signer generate -w ~/.tauri/disk-manager.key

# 2. 带签名构建
export TAURI_SIGNING_PRIVATE_KEY=~/.tauri/disk-manager.key
npx tauri build --bundles nsis

# 3. 生成更新清单
python scripts/make_latest_json.py --repo <用户名>/<仓库名> --notes "更新说明"
```

然后把 `release/` 下的三个文件上传到 GitHub Release：

```
disk-manager_<版本>_x64-setup.exe
disk-manager_<版本>_x64-setup.exe.sig
latest.json
```

⚠️ 两个必须注意的点：

1. **仓库必须是 public**。私有仓库的 Release 资产需要认证才能下载，客户端拿不到 `latest.json`，自动更新会失败。
2. **私钥必须保密且备份**。公钥已编译进程序，只有配套私钥签出的包才会被接受；私钥丢失后旧版本将无法接收新更新。

## 目录结构

```
src/                  前端（React + TypeScript）
  pages/              各功能页
  services/           与 Rust 通信、状态缓存
src-tauri/src/
  commands/           所有系统调用（磁盘、扫描、清理、卸载、SMART）
    clean_rules.rs    清理规则表 + 规则引擎
    app_cache.rs      应用缓存自动发现
    uninstall.rs      卸载与残留扫描
    update.rs         自动更新入口
scripts/              发布辅助脚本
```

## 数据与隐私

所有磁盘、注册表与软件信息均在本机读取，**不上传任何数据**。唯一的外网请求是检查更新时拉取 GitHub 上的版本清单。

## 许可

MIT
