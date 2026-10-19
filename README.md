# Disk Manager · Windows 存储管理

一个面向 Windows 10 / 11 的本地磁盘管理工具：磁盘总览、空间分析、垃圾清理、软件卸载、硬件信息，放在同一处。

Rust + Tauri 2 + React，安装包约 4.3 MB，常驻内存低。

## 功能

**磁盘总览**：分区容量、文件系统、物理硬盘与 SMART 健康度（NVMe / SATA），温度与磨损单独标色。

**空间分析**
- 直接解析 NTFS 的 `$MFT` 元文件取全盘清单，整盘扫描通常在 1~2 秒；标准权限或非 NTFS 分区自动回退到目录遍历
- 目录树按体积排序，可逐层下钻
- 「这是什么」：选中目录或文件，直接给出「是什么 / 能不能删 / 删了会怎样」，附判断依据（路径特征、扩展名分布、文件头真实格式）
- 右侧是 AI 分析位：当前目录的说明，以及这一层最大的 5 项逐项判断

**垃圾清理**（三个页签）
- 垃圾扫描：70+ 条声明式规则 + 应用缓存深度扫描（按精确白名单识别 Electron / Chromium 系缓存，覆盖手工规则追不上的新应用）
- 安装包清理：`.msi` / `.msix` / `.msu` 与文件名含 setup / install 的 `.exe`，排除卸载器与系统安装缓存
- 空文件夹：后序遍历，只调 `remove_dir`，扫描后被写入内容的目录会自动跳过
- 危险项（如 `Windows.old`）、显卡着色器缓存、安装包与空目录**默认都不勾选**，需要自己确认
- 只删除缓存目录内的文件、保留目录本身，应用下次启动自动重建；配置文件、登录数据与数据库一律跳过

**软件卸载**
- 优先调用软件自带卸载程序，读 exe 签名判定安装框架（Inno / NSIS / MSI），据此选择正确的静默参数
- 卸载后扫描残留：注册表、目录、快捷方式、开机启动、服务、计划任务、防火墙规则、PATH 变量、COM 关联
- 卸载器弹交互向导时自动改用轮询，界面不阻塞、可随时关闭
- 强制移除（卸载程序已损坏时的兜底）

**AI 助手**
- 右下角悬浮球，`Ctrl+K` 也能唤起；内置额度开箱可用，也可以换成自己的 Key（DeepSeek / 通义千问 / Kimi / 智谱 / OpenAI / 本地 Ollama）
- 回答逐字流式输出；可一次分析整层目录、给出追问建议，结果按目录缓存
- 首屏引导语与推荐问题跟随当前页面变化
- 常见问题走本地问答库，零延迟、零费用，不配置 AI 也能用
- 只发送路径、体积、扩展名分布、硬件配置这类摘要，**不读取任何文件内容**；删除操作永远需要手动勾选确认

**自动更新**：启动后静默检查 GitHub Release，发现新版本在设置页提示，可一键下载安装（安装包带签名校验）。

## 构建

需要 Node.js 20+、Rust stable、MSVC 构建工具，以及 WebView2 运行时（Win11 自带，Win10 一般随 Edge 安装）。

```bash
npm install
npx tauri build --bundles nsis     # 安装包
npx tauri build --no-bundle        # 免安装 exe
```

产物在 `src-tauri/target/release/`，安装包在 `bundle/nsis/`。

> 如果 rustc 报 `link.exe not found`，参考 `dev-env.example.sh` 指定 MSVC 工具链。

## 发布

自动更新依赖三样东西：安装包、它的签名、一份 `latest.json` 清单。

```bash
# 1. 签名私钥（首次生成一次，务必备份且不要外传）
npx tauri signer generate -w ~/.tauri/disk-manager.key

# 2. 带签名构建
export TAURI_SIGNING_PRIVATE_KEY=~/.tauri/disk-manager.key
npx tauri build --bundles nsis

# 3. 生成更新清单
python scripts/make_latest_json.py --repo qvq9999/disk-manager --notes "更新说明"
```

把 `release/` 下的三个文件上传到 GitHub Release：

```
disk-manager_<版本>_x64-setup.exe
disk-manager_<版本>_x64-setup.exe.sig
latest.json
```

⚠️ 两个必须注意的点：

1. **仓库必须是 public**。私有仓库的 Release 资产需要认证才能下载，客户端拿不到 `latest.json`，自动更新会失败。
2. **私钥必须保密且备份**。公钥已编译进程序，只有配套私钥签出的包才会被接受；私钥丢失后旧版本将无法接收更新。

## 目录结构

```
src/                  前端（React + TypeScript）
  pages/              各功能页（cleanup/ 下是清理页的三个页签）
  components/         弹窗、AI 助手、免责声明等
  services/           与 Rust 通信、状态缓存、页面上下文
src-tauri/src/
  commands/           所有系统调用
    scan.rs           目录遍历（并行）与 MFT 结果建树
    mft.rs            NTFS $MFT 解析
    clean_rules.rs    清理规则表 + 规则引擎
    app_cache.rs      应用缓存自动发现
    installer.rs      安装包扫描与删除
    emptydir.rs       空文件夹扫描与删除
    uninstall.rs      卸载与残留扫描
    identify.rs       目录/文件识别（含文件头校验）
    ai.rs             AI 请求、配置与密钥加密
    update.rs         自动更新入口
scripts/              发布辅助脚本
```

## 数据与隐私

磁盘、注册表与软件信息全部在本机读取。外网请求只有两类：

1. 检查更新时拉取 GitHub 上的版本清单；
2. **你主动使用 AI 功能**时，把当前页面的摘要（路径、体积、扩展名分布、硬件配置）发给你选择的服务商，不包含任何文件内容。

API Key 用 Windows DPAPI 加密后存在本机，只有当前用户能解密。AI 功能可以完全关闭，本地识别与清理规则不受影响。

## 许可

MIT
