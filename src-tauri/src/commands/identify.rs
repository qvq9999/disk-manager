//! 本地识别「这是什么 / 能不能删」。
//!
//! 设计原则（与产品方案一致）：
//! - **类型判断不交给大模型**。扩展名 + 目录签名 + 文件头魔数在本机就能判准，
//!   而且不需要联网、没有费用、没有隐私风险。
//! - 大模型只兜「本地判不出来的长尾」，且只收摘要（见 `summary` 字段）。
//!
//! 安全边界：
//! - 本模块**只读**。除了读文件头 16 字节与目录一层元数据，不打开、不解析、不修改任何文件。
//! - 判定结果 `verdict` 只用于文案与颜色，**不参与任何删除决策**；
//!   删除永远只认扫描树里的真实路径 + 用户显式勾选。

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

/// 单目录最多采样多少个条目（避免超大目录把点击拖成几秒）
const MAX_ENTRIES: usize = 400;
/// 读文件头的字节数
const HEAD_LEN: usize = 16;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyResult {
    /// 结论标题，如「Steam 游戏本体」
    pub label: String,
    /// 2 = 确定，1 = 较可能，0 = 判不出来
    pub confidence: u8,
    /// 每条都能在界面上摊开给用户看
    pub evidence: Vec<String>,
    /// safe | caution | danger | unknown
    pub verdict: String,
    /// 这是什么（一句话）
    pub what: String,
    /// 删了会怎样（一句话）
    pub consequence: String,
    /// local | ai
    pub source: String,
    /// 供云端分析使用的摘要（不含文件内容）
    pub summary: String,
    /// 建议动作：clean（可去清理）/ locate（只是看看）/ none
    pub action_hint: String,
    /// 扫描范围是否被截断（条目数超过 MAX_ENTRIES）
    pub truncated: bool,
}

// ============ 目录签名库 ============
//
// 匹配方式：把路径按 `\` 切段并小写，`segs` 需按顺序出现在路径里（子序列匹配）。
// **数组顺序就是优先级** —— 具体的必须排在宽泛的前面，否则 `Windows\WinSxS`
// 会先命中 `Windows` 这条。改动时务必保持这个约束。

struct Sig {
    segs: &'static [&'static str],
    label: &'static str,
    verdict: &'static str,
    what: &'static str,
    consequence: &'static str,
}

const SIGS: &[Sig] = &[
    // ---- 绝对不能删 ----
    Sig {
        segs: &["windows", "winsxs"],
        label: "Windows 组件存储（WinSxS）",
        verdict: "danger",
        what: "系统组件、更新备份与旧版本文件，主要靠硬链接共享",
        consequence: "不能手动删除。删了会导致更新与修复失败；而且界面显示的体积大部分是硬链接，实际占用远小于这个数字。",
    },
    Sig {
        segs: &["windows", "installer"],
        label: "MSI 安装缓存",
        verdict: "danger",
        what: "已安装程序的安装信息、补丁与卸载所需文件",
        consequence: "删了会让软件无法卸载、无法修复、无法更新。这是最容易被网上教程误删的目录之一。",
    },
    Sig {
        segs: &["windows", "system32"],
        label: "Windows 核心系统目录（System32）",
        verdict: "danger",
        what: "系统核心文件与驱动",
        consequence: "不能删除任何内容，会直接导致系统无法启动。",
    },
    Sig {
        segs: &["windows", "syswow64"],
        label: "Windows 32 位兼容目录",
        verdict: "danger",
        what: "64 位系统上运行 32 位程序所需的系统文件",
        consequence: "不能删除，会导致大量 32 位程序无法启动。",
    },
    Sig {
        segs: &["apple", "mobilesync", "backup"],
        label: "iPhone / iPad 本地备份",
        verdict: "danger",
        what: "iTunes / 访达做过的手机整机备份",
        consequence: "删了等于永久丢失该备份（含照片、聊天记录、健康数据）。确认已在 iCloud 有备份再动。",
    },
    Sig {
        segs: &["wechat files"],
        label: "微信聊天文件与记录",
        verdict: "danger",
        what: "微信接收的图片、视频、文件与聊天数据库",
        consequence: "直接删目录会丢失聊天记录。要在微信「设置 - 存储空间」里清理，别在这里删。",
    },
    Sig {
        segs: &["tencent files"],
        label: "QQ 聊天文件与记录",
        verdict: "danger",
        what: "QQ 的聊天记录、接收文件与表情缓存",
        consequence: "直接删目录会丢失聊天记录，请在 QQ 内清理。",
    },
    Sig {
        segs: &["onedrive"],
        label: "OneDrive 同步目录",
        verdict: "danger",
        what: "与云端保持同步的文件",
        consequence: "在本机删除会同步删除云端文件，且会进云端回收站。处理前先在网页端确认。",
    },
    Sig {
        segs: &["steamapps", "common"],
        label: "Steam 游戏本体",
        verdict: "danger",
        what: "Steam 下载安装的游戏文件",
        consequence: "删了要重新下载几十 GB。存档大多在别处，但个别游戏把存档放在这里，删前先确认。",
    },
    Sig {
        segs: &["epic games"],
        label: "Epic 游戏库",
        verdict: "danger",
        what: "Epic 平台安装的游戏",
        consequence: "删了要重新下载。请优先在启动器里卸载。",
    },
    Sig {
        segs: &["gog games"],
        label: "GOG 游戏库",
        verdict: "danger",
        what: "GOG 平台安装的游戏",
        consequence: "删了要重新下载。",
    },
    Sig {
        segs: &["wsl"],
        label: "WSL 虚拟磁盘",
        verdict: "danger",
        what: "Linux 子系统的 ext4 虚拟磁盘镜像",
        consequence: "删了等于删掉整个 Linux 发行版与里面所有文件。要瘦身请用 diskpart 压缩虚拟磁盘，不要删文件。",
    },
    Sig {
        segs: &["docker"],
        label: "Docker 数据目录",
        verdict: "caution",
        what: "镜像、容器层与数据卷",
        consequence: "这里的文件不能直接删。要释放空间请用 docker system prune，否则容器与数据会一起没。",
    },
    Sig {
        segs: &["$recycle.bin"],
        label: "回收站",
        verdict: "safe",
        what: "已删除但还没清空的文件",
        consequence: "清空后无法恢复。建议先打开回收站看一眼里面是什么。",
    },
    // ---- 可以清理但有代价 ----
    Sig {
        segs: &["windows", "softwaredistribution", "download"],
        label: "Windows 更新缓存",
        verdict: "safe",
        what: "已下载的 Windows 更新安装包",
        consequence: "可以删。删完系统会重新下载需要的更新。",
    },
    Sig {
        segs: &["programdata", "package cache"],
        label: "安装包缓存（Package Cache）",
        verdict: "caution",
        what: "Visual Studio、VC++ 运行库等组件的原始安装包",
        consequence: "删了之后修复或卸载这些组件时，会被要求重新提供安装包。通常能腾出几 GB。",
    },
    Sig {
        segs: &["windows", "temp"],
        label: "Windows 临时文件",
        verdict: "safe",
        what: "系统进程运行时产生的临时文件",
        consequence: "可以删，正在被占用的文件会自动跳过。",
    },
    Sig {
        segs: &["windows", "logs"],
        label: "Windows 日志归档",
        verdict: "safe",
        what: "系统与组件的历史日志",
        consequence: "可以删，不影响系统运行。排障时可能用得上最近几天的日志。",
    },
    Sig {
        segs: &["programdata", "microsoft", "windows", "wer"],
        label: "Windows 错误报告",
        verdict: "safe",
        what: "程序崩溃时生成的转储与报告文件",
        consequence: "可以删。留下的都是历史崩溃记录。",
    },
    Sig {
        segs: &["appdata", "local", "temp"],
        label: "用户临时文件",
        verdict: "safe",
        what: "各程序运行时的临时文件",
        consequence: "可以删，被占用的会自动跳过。个别软件首次启动会慢一点。",
    },
    Sig {
        segs: &["appdata", "local", "npm-cache"],
        label: "npm 缓存",
        verdict: "safe",
        what: "npm 下载过的包压缩档",
        consequence: "可以删，下次安装依赖会重新下载，装包变慢。",
    },
    Sig {
        segs: &["appdata", "roaming", "npm-cache"],
        label: "npm 缓存",
        verdict: "safe",
        what: "npm 下载过的包压缩档",
        consequence: "可以删，下次安装依赖会重新下载。",
    },
    Sig {
        segs: &["appdata", "local", "yarn", "cache"],
        label: "Yarn 缓存",
        verdict: "safe",
        what: "Yarn 下载过的包",
        consequence: "可以删，重新装依赖会重新下载。",
    },
    Sig {
        segs: &["cargo", "registry"],
        label: "Rust 依赖缓存",
        verdict: "caution",
        what: "从 crates.io 下载的源码包与编译产物",
        consequence: "可以删，但下次编译要重新下载并从头编译，可能多花十几分钟。",
    },
    Sig {
        segs: &["rustup"],
        label: "Rust 工具链",
        verdict: "caution",
        what: "编译器、标准库与目标平台支持",
        consequence: "删了 Rust 就无法编译，需要重新安装工具链。",
    },
    Sig {
        segs: &[".vscode", "extensions"],
        label: "VS Code 扩展",
        verdict: "caution",
        what: "已安装的编辑器插件",
        consequence: "删了要在编辑器里重新安装，不影响代码本身。",
    },
    Sig {
        segs: &["nuget"],
        label: "NuGet 包缓存",
        verdict: "caution",
        what: ".NET 项目引用的包",
        consequence: "删了下次还原会重新下载，构建变慢。",
    },
    Sig {
        segs: &["steamapps", "workshop"],
        label: "Steam 创意工坊内容",
        verdict: "caution",
        what: "订阅的模组、地图与素材",
        consequence: "删了重新订阅即可恢复，但要重新下载。",
    },
    Sig {
        segs: &["steamapps", "downloading"],
        label: "Steam 未完成下载",
        verdict: "safe",
        what: "下载到一半的分片文件",
        consequence: "可以删，Steam 会重新下载未完成的部分。",
    },
    Sig {
        segs: &["adobe", "common", "media cache"],
        label: "Adobe 媒体缓存",
        verdict: "safe",
        what: "Premiere / After Effects 的预览与音频缓存",
        consequence: "可以删，重新打开工程时会重建，首次预览会慢一点。",
    },
    Sig {
        segs: &["node_modules"],
        label: "Node.js 项目依赖",
        verdict: "safe",
        what: "npm / pnpm / yarn 安装的第三方包",
        consequence: "可以安全删除，进项目目录重新装依赖即可恢复。它是「小文件极多」的典型，删起来比较慢。",
    },
    Sig {
        segs: &[".git"],
        label: "Git 版本库",
        verdict: "danger",
        what: "提交历史与对象库",
        consequence: "删了仓库历史就没了，代码会变回普通文件夹。只能用 git gc 瘦身，不能手动删。",
    },
    Sig {
        segs: &["program files (x86)"],
        label: "32 位程序安装目录",
        verdict: "caution",
        what: "已安装软件的程序文件",
        consequence: "请用「软件卸载」功能卸载，直接删会留下注册表与残留。",
    },
    Sig {
        segs: &["program files"],
        label: "程序安装目录",
        verdict: "caution",
        what: "已安装软件的程序文件",
        consequence: "请用「软件卸载」功能卸载，直接删会留下注册表与残留。",
    },
    Sig {
        segs: &["downloads"],
        label: "下载文件夹",
        verdict: "caution",
        what: "浏览器与下载工具保存的文件",
        consequence: "删前逐个确认，里面常混着安装包和没来得及归档的资料。",
    },
    Sig {
        segs: &["desktop"],
        label: "桌面",
        verdict: "caution",
        what: "桌面上的文件与快捷方式",
        consequence: "删前逐个确认。",
    },
    Sig {
        segs: &["documents"],
        label: "文档目录",
        verdict: "caution",
        what: "用户文档与工作文件",
        consequence: "删前逐个确认，这里最容易出现「以为没用其实重要」的文件。",
    },
    Sig {
        segs: &["appdata", "local", "packages"],
        label: "UWP 应用数据",
        verdict: "caution",
        what: "商店应用的数据、设置与缓存",
        consequence: "不能整目录删。里面 LocalState 是用户数据，LocalCache 才是缓存。",
    },
    Sig {
        segs: &["users"],
        label: "用户目录（所有账户）",
        verdict: "caution",
        what: "每个 Windows 账户的个人数据总入口",
        consequence: "桌面、文档、下载、AppData 都在它下面。不能整体删，展开后逐项判断。",
    },
    Sig {
        segs: &["appdata", "roaming"],
        label: "程序设置与用户数据",
        verdict: "caution",
        what: "软件保存的配置、账号状态与个人数据",
        consequence: "删了软件会像刚装的一样，登录态与个人设置全丢。",
    },
    Sig {
        segs: &["appdata", "local"],
        label: "程序本地数据",
        verdict: "caution",
        what: "软件的本地数据、缓存与日志",
        consequence: "里面缓存可清、数据不可清，必须逐个子目录判断。",
    },
    Sig {
        segs: &["windows"],
        label: "Windows 系统目录",
        verdict: "caution",
        what: "系统文件、驱动与内置组件",
        consequence: "不要手动删这里的东西，请用系统自带的「磁盘清理」。",
    },
    Sig {
        segs: &["programdata"],
        label: "程序共享数据",
        verdict: "caution",
        what: "所有用户共享的软件数据",
        consequence: "按子目录判断，不要整目录删。",
    },
];

/// 显卡厂商与图形缓存的路径段（产品约定：这类内容默认不勾，只做提示）
const GPU_SEGS: &[&str] = &[
    "nvidia", "nvidia corporation", "amd", "intel", "dxcache", "glcache",
    "d3dscache", "shadercache", "gpu_cache", "graphics",
];

fn split_segs(path: &str) -> Vec<String> {
    path.to_lowercase()
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 按顺序在路径里找 `want`（子序列匹配，不要求连续）
fn matches_segs(segs: &[String], want: &[&str]) -> bool {
    if want.is_empty() || want.len() > segs.len() {
        return false;
    }
    let mut i = 0;
    for s in segs {
        if i < want.len() && s.as_str() == want[i] {
            i += 1;
            if i == want.len() {
                return true;
            }
        }
    }
    false
}

// ============ 文件头魔数 ============

/// 返回 (格式名, 常见扩展名)
fn sniff(head: &[u8]) -> Option<(&'static str, &'static str)> {
    let n = head.len();
    if n < 2 {
        return None;
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(("PNG 图像", "png"));
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("JPEG 图像", "jpg"));
    }
    if head.starts_with(b"GIF8") {
        return Some(("GIF 图像", "gif"));
    }
    if head.starts_with(b"%PDF") {
        return Some(("PDF 文档", "pdf"));
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Some(("ZIP 容器", "zip"));
    }
    if head.starts_with(b"Rar!") {
        return Some(("RAR 压缩包", "rar"));
    }
    if head.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Some(("7z 压缩包", "7z"));
    }
    if head.starts_with(b"MZ") {
        return Some(("Windows 可执行文件", "exe"));
    }
    if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Some(("OLE2 复合文档", "doc"));
    }
    if n >= 8 && &head[4..8] == b"ftyp" {
        return Some(("MP4 / MOV 视频", "mp4"));
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(("Matroska 视频", "mkv"));
    }
    if head.starts_with(b"SQLite format 3") {
        return Some(("SQLite 数据库", "db"));
    }
    if head.starts_with(&[0x7F, b'E', b'L', b'F']) {
        return Some(("ELF 可执行文件", "elf"));
    }
    if n >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(("WebP 图像", "webp"));
    }
    if head.starts_with(b"BM") {
        return Some(("BMP 图像", "bmp"));
    }
    if head.starts_with(b"OggS") {
        return Some(("Ogg 音频", "ogg"));
    }
    if head.starts_with(b"fLaC") {
        return Some(("FLAC 音频", "flac"));
    }
    if head.starts_with(b"ID3") {
        return Some(("MP3 音频", "mp3"));
    }
    None
}

/// 读前 16 字节；读不到（权限/空文件）返回 None
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD_LEN];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    buf.truncate(n);
    Some(buf)
}

fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.len() <= 8 => e.to_lowercase(),
        _ => String::new(),
    }
}

/// 这些扩展名本身就是可执行体，命中不代表「伪装」
const EXEC_EXTS: &[&str] = &["exe", "dll", "msi", "sys", "scr", "com", "cpl", "ocx", "drv", "efi", "so", "node"];

/// 系统大文件按文件名识别。返回 (label, verdict, what, consequence)。
///
/// 这几个文件用户一定会点开问「能不能删」，不能给「未知文件」。
fn file_signature(name: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match name {
        "pagefile.sys" => (
            "系统页面文件（虚拟内存）",
            "danger",
            "Windows 把内存里暂时不用的数据换到这里的磁盘文件",
            "不能删除。删了会失去虚拟内存、大程序容易崩。要调整大小请到「系统属性 - 高级 - 性能 - 虚拟内存」里改。",
        ),
        "hiberfil.sys" => (
            "休眠文件",
            "danger",
            "休眠时把整个内存内容写到磁盘，用于快速恢复现场",
            "不能直接删。如果不用休眠，可在管理员命令行执行 powercfg -h off，它会被回收，空间也一起回来。",
        ),
        "swapfile.sys" => (
            "UWP 交换文件",
            "danger",
            "供商店应用使用的轻量交换文件",
            "不能删。它由系统管理，删掉会被立刻重建。",
        ),
        "ntuser.dat" => (
            "用户注册表配置单元",
            "danger",
            "当前账户的全部注册表设置",
            "不能删，会导致该账户的配置完全损坏。",
        ),
        "bootmgr" | "bootnxt" => (
            "启动管理器",
            "danger",
            "系统启动引导文件",
            "不能删，删了系统无法启动。",
        ),
        _ => return None,
    })
}

fn fmt_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f >= K * K * K {
        format!("{:.1} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.1} MB", f / (K * K))
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{n} B")
    }
}

#[derive(Default)]
struct DirStat {
    files: usize,
    dirs: usize,
    total: u64,
    by_ext: HashMap<String, usize>,
    has_exe: bool,
    has_git: bool,
    sample: Vec<String>,
    scanned: usize,
    /// 目录读不到（权限不足 / 路径不存在）。
    ///
    /// 这个标志必须存在：`read_dir` 失败时 files 与 dirs 都是 0，
    /// 如果只看计数，就会把一个「读不到的系统目录」判定成「空文件夹，可以删」——
    /// 这是能直接误导用户删错东西的 bug。
    unreadable: bool,
}

fn scan_dir(dir: &Path) -> DirStat {
    let mut st = DirStat::default();
    let Ok(rd) = fs::read_dir(dir) else {
        st.unreadable = true;
        return st;
    };
    for e in rd {
        if st.scanned >= MAX_ENTRIES {
            break;
        }
        st.scanned += 1;
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if st.sample.len() < 24 {
            st.sample.push(name.clone());
        }
        match e.metadata() {
            Ok(md) if md.is_dir() => {
                st.dirs += 1;
                if lower == ".git" {
                    st.has_git = true;
                }
            }
            Ok(md) => {
                st.files += 1;
                st.total += md.len();
                let ext = ext_of(&lower);
                if matches!(ext.as_str(), "exe" | "dll") {
                    st.has_exe = true;
                }
                if !ext.is_empty() {
                    *st.by_ext.entry(ext).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    st
}

fn top_exts(by_ext: &HashMap<String, usize>, n: usize) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = by_ext.iter().map(|(k, c)| (k.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// 识别一个目录或文件。`size` 由前端从扫描树传入（后端不重算，避免重复遍历）。
///
/// 核心逻辑放在 `identify_impl` 里，命令只做转发 —— 这样单测可以直接调，
/// 不用经过 Tauri 的消息层。
#[tauri::command]
pub fn identify_node(path: String, kind: String, size: u64) -> Result<IdentifyResult, String> {
    identify_impl(&path, &kind, size)
}

/// 识别核心逻辑（纯函数，可单测）
pub fn identify_impl(path: &str, kind: &str, size: u64) -> Result<IdentifyResult, String> {
    if path.trim().is_empty() {
        return Err("路径为空".into());
    }
    let segs = split_segs(path);
    let name = segs.last().cloned().unwrap_or_default();

    // ---------- 文件 ----------
    if kind == "file" {
        let p = Path::new(path);
        let mut evidence: Vec<String> = Vec::new();
        let ext = ext_of(&name);
        let head = read_head(p);
        let real = head.as_deref().and_then(sniff);

        // 先看文件名：这几个大文件用户一定会点开问「能不能删」，不能答「未知文件」
        if let Some((label, verdict, what, consequence)) = file_signature(&name) {
            evidence.push(format!("文件名命中：{name}"));
            if size > 0 {
                evidence.push(format!("体积 {}", fmt_bytes(size)));
            }
            if let Some((fmt, _)) = real {
                evidence.push(format!("文件头识别为{fmt}"));
            }
            return Ok(IdentifyResult {
                label: label.into(),
                confidence: 2,
                evidence,
                verdict: verdict.into(),
                what: what.into(),
                consequence: consequence.into(),
                source: "local".into(),
                summary: String::new(),
                action_hint: "none".into(),
                truncated: false,
            });
        }

        // 扩展名伪装检测（安全价值最高的一条）
        if let Some((fmt, real_ext)) = real {
            let mismatch = !ext.is_empty()
                && !real_ext.is_empty()
                && ext != real_ext
                && !(real_ext == "exe" && EXEC_EXTS.contains(&ext.as_str()))
                && !(real_ext == "zip" && matches!(ext.as_str(), "docx" | "xlsx" | "pptx" | "jar" | "apk" | "epub" | "odt" | "vsix"))
                && !(real_ext == "doc" && matches!(ext.as_str(), "doc" | "xls" | "ppt" | "msi" | "msg"))
                && !(real_ext == "mp4" && matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "3gp" | "heic"))
                && !(real_ext == "db" && matches!(ext.as_str(), "db" | "sqlite" | "sqlite3"))
                && !(real_ext == "elf" && ext.is_empty());

            if mismatch {
                let dangerous = real_ext == "exe";
                evidence.push(format!("扩展名是 .{ext}，实际内容是 {fmt}"));
                return Ok(IdentifyResult {
                    label: if dangerous { "伪装文件（实为可执行程序）" } else { "扩展名与内容不符" }.into(),
                    confidence: 2,
                    evidence,
                    verdict: if dangerous { "danger" } else { "caution" }.into(),
                    what: format!("文件真实格式是{fmt}，与扩展名 .{ext} 不一致"),
                    consequence: if dangerous {
                        "可执行文件伪装成普通文件是常见投毒手法，请勿运行，建议先杀毒扫描。".into()
                    } else {
                        "可能是手工改过后缀，也可能是下载不完整。改名成正确扩展名后一般可正常打开。".into()
                    },
                    source: "local".into(),
                    summary: String::new(),
                    action_hint: "none".into(),
                    truncated: false,
                });
            }

            evidence.push(format!("文件头识别为{fmt}"));
            evidence.push(format!("扩展名 .{ext} 与格式一致"));
            evidence.push(format!("体积 {}", fmt_bytes(size)));
            return Ok(IdentifyResult {
                label: fmt.to_string(),
                confidence: 2,
                evidence,
                verdict: "caution".into(),
                what: format!("一个正常的{fmt}"),
                consequence: "是否为重要文件要看它所在的目录与用途，删前确认路径。".into(),
                source: "local".into(),
                summary: String::new(),
                action_hint: "locate".into(),
                truncated: false,
            });
        }

        // 读不到文件头（权限/占用）→ 只按扩展名给弱结论
        if !ext.is_empty() {
            evidence.push(format!("无法读取文件头，仅凭扩展名 .{ext} 判断"));
            evidence.push(format!("体积 {}", fmt_bytes(size)));
        } else {
            evidence.push("没有扩展名，也读不到文件头".into());
        }
        return Ok(IdentifyResult {
            label: if ext.is_empty() { "未知文件".into() } else { format!("{ext} 文件") },
            confidence: 1,
            evidence,
            verdict: "unknown".into(),
            what: "只能确认它是一个文件，具体用途判不出来。".into(),
            consequence: "在确认用途前不要删除。".into(),
            source: "local".into(),
            summary: String::new(),
            action_hint: "none".into(),
            truncated: false,
        });
    }

    // ---------- 目录 ----------
    let p = Path::new(path);
    let st = scan_dir(p);
    let truncated = st.scanned >= MAX_ENTRIES;

    let mut evidence: Vec<String> = Vec::new();
    if st.unreadable {
        evidence.push("目录无法读取（权限不足，或路径已不存在）".into());
    }
    if st.files > 0 || st.dirs > 0 {
        evidence.push(format!("直接子项：{} 个文件 / {} 个目录", st.files, st.dirs));
    }
    if truncated {
        evidence.push(format!("目录项过多，只采样了前 {MAX_ENTRIES} 项"));
    }
    let exts = top_exts(&st.by_ext, 6);
    if !exts.is_empty() {
        let s = exts
            .iter()
            .map(|(e, c)| format!(".{e} x{c}"))
            .collect::<Vec<_>>()
            .join("  ");
        evidence.push(format!("主要扩展名：{s}"));
    }
    if st.files > 0 {
        evidence.push(format!("采样得到平均文件大小 {}", fmt_bytes(st.total / st.files as u64)));
    }

    // 摘要（AI 用）：只含路径、体积、结构，不含任何文件内容
    let mut summary = String::new();
    summary.push_str(&format!("路径：{path}\n"));
    summary.push_str(&format!("体积：{}（{} 字节）\n", fmt_bytes(size), size));
    summary.push_str(&format!("直接子项：{} 个文件 / {} 个目录\n", st.files, st.dirs));
    if !exts.is_empty() {
        let s = exts.iter().map(|(e, c)| format!(".{e}({c})")).collect::<Vec<_>>().join(" ");
        summary.push_str(&format!("扩展名分布：{s}\n"));
    }
    if !st.sample.is_empty() {
        let s = st.sample.iter().take(16).cloned().collect::<Vec<_>>().join(" / ");
        summary.push_str(&format!("子项示例：{s}\n"));
    }

    // 1) 盘根：`C:\` 这种 segs 只有一个 "c:" 的形式。
    //    用户进入空间分析第一眼看到的就是盘根，给「普通文件夹 / 判不出来」等于什么都没说。
    let is_drive_root = segs.len() == 1 && segs[0].len() == 2 && segs[0].ends_with(':');
    if is_drive_root {
        let sys = p.join("Windows").is_dir();
        let letter = segs[0].trim_end_matches(':').to_uppercase();
        let mut ev = evidence;
        ev.insert(0, format!("盘符 {letter}: 的根目录"));
        if sys {
            ev.push("存在 Windows 目录，判断为系统盘".into());
        }
        return Ok(IdentifyResult {
            label: if sys { "系统盘根目录".into() } else { "分区根目录".into() },
            confidence: 2,
            evidence: ev,
            verdict: "caution".into(),
            what: if sys {
                "Windows 所在的分区根目录，系统、程序与用户数据都在它下面"
            } else {
                "一个数据分区的根目录"
            }
            .into(),
            consequence: "根目录本身不能整体处理，展开到子目录逐个判断。".into(),
            source: "local".into(),
            summary,
            action_hint: "locate".into(),
            truncated,
        });
    }

    // 2) 签名库（按数组顺序，具体的在前）
    let gpu = segs.iter().any(|s| GPU_SEGS.contains(&s.as_str()));
    for sig in SIGS {
        if matches_segs(&segs, sig.segs) {
            let mut ev = evidence.clone();
            ev.insert(0, format!("路径命中「{}」特征", sig.segs.join("\\")));
            if gpu {
                ev.push("该路径含显卡厂商/图形缓存特征".into());
            }
            let mut verdict = sig.verdict.to_string();
            // 产品约定：显卡相关一律降级为「不建议清理」
            if gpu && verdict == "safe" {
                verdict = "caution".into();
            }
            return Ok(IdentifyResult {
                label: sig.label.into(),
                confidence: 2,
                evidence: ev,
                verdict: verdict.clone(),
                what: sig.what.into(),
                consequence: if gpu && sig.verdict == "safe" {
                    "里面有游戏与创作软件的预加载内容，清掉要重新编译，首次启动会明显变慢，本工具默认不清理这类内容。".into()
                } else {
                    sig.consequence.into()
                },
                source: "local".into(),
                summary,
                action_hint: if verdict == "safe" { "clean" } else { "locate" }.into(),
                truncated,
            });
        }
    }

    // 2) 未命中签名 → 用结构特征给一个弱结论
    let avg = if st.files > 0 { st.total / st.files as u64 } else { 0 };
    let total = if size > 0 { size } else { st.total };

    let (label, verdict, what, consequence, conf) = if st.unreadable {
        (
            "无法读取的目录",
            "unknown",
            "没有权限读取，或这个路径已经不存在",
            "读不到内容就无法判断它是什么，请不要仅凭体积删除；可以试着以管理员身份重试。",
            0,
        )
    } else if st.files == 0 && st.dirs == 0 {
        ("空文件夹", "safe", "一个没有任何内容的空目录", "可以删除，不会影响任何程序。", 2)
    } else if st.has_git {
        ("Git 版本库", "danger", "一个带 .git 的代码仓库", "删了会丢掉提交历史，只能用 git gc 瘦身。", 2)
    } else if st.has_exe {
        ("程序目录", "caution", "含可执行文件的软件目录", "请用「软件卸载」功能处理，直接删会留下注册表残留。", 1)
    } else if st.files > 500 && avg < 64 * 1024 {
        ("小文件集合", "caution", "由大量小文件组成，通常是缓存、依赖或缩略图", "多半可以删，但删起来较慢；确认不是依赖目录后再动。", 1)
    } else if st.files > 0 && avg > 100 * 1024 * 1024 {
        ("大文件集合", "caution", "以少数几个大文件为主，通常是视频、镜像或备份", "删前确认内容，这类文件往往不可再生。", 1)
    } else if st.files > 0 && exts.len() <= 2 && exts.first().map(|(e, _)| e.as_str()) == Some("mp4") {
        ("视频文件集合", "caution", "以视频文件为主", "删前确认是否为原始素材。", 1)
    } else if st.dirs > 0 && st.files == 0 {
        ("多级目录容器", "unknown", "只有子目录，没有直接存放的文件", "需要展开看子目录才能判断。", 0)
    } else {
        ("普通文件夹", "unknown", "没有命中已知特征", "判不出来它属于什么软件或用途，删前请展开确认。", 0)
    };

    evidence.push(format!("总体积 {}", fmt_bytes(total)));
    let mut ev = evidence;
    ev.push("未命中已知目录特征库".into());

    Ok(IdentifyResult {
        label: label.into(),
        confidence: conf,
        evidence: ev,
        verdict: verdict.into(),
        what: what.into(),
        consequence: consequence.into(),
        source: "local".into(),
        summary,
        action_hint: if verdict == "safe" { "clean" } else { "none" }.into(),
        truncated,
    })
}

// ============ 打开所在位置（原生，不用 WebView 的浏览行为） ============

/// 在资源管理器中定位该路径。失败不影响识别结果，前端忽略错误即可。
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("路径不存在".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("explorer.exe");
        if p.is_dir() {
            cmd.arg(&path);
        } else {
            cmd.arg(format!("/select,{path}"));
        }
        // explorer 自己会开窗口，这里只是不闪父进程的黑框
        let _ = cmd.creation_flags(CREATE_NO_WINDOW).spawn();
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("仅支持 Windows".into())
    }
}

/// 把外部链接交给系统默认浏览器，避免在应用内导航后回不来。
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err("只允许打开 http/https 链接".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // 用 rundll32 而不是 cmd /c start：后者对含 & 的 URL 会被截断
        let mut cmd = std::process::Command::new("rundll32.exe");
        cmd.arg("url.dll,FileProtocolHandler").arg(u);
        let _ = cmd.creation_flags(CREATE_NO_WINDOW).spawn();
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("仅支持 Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seg_match_requires_order() {
        assert!(matches_segs(
            &split_segs("D:\\SteamLibrary\\steamapps\\common\\Half-Life 2"),
            &["steamapps", "common"]
        ));
        // 顺序反了不算命中（否则 common\steamapps 也会被当成游戏库）
        assert!(!matches_segs(&split_segs("D:\\common\\steamapps"), &["steamapps", "common"]));
        // 只命中一半不算
        assert!(!matches_segs(&split_segs("C:\\Windows"), &["windows", "winsxs"]));
    }

    #[test]
    fn drive_root_is_recognized() {
        let r = identify_impl("C:\\", "dir", 0).unwrap();
        assert_eq!(r.label, "系统盘根目录");
        assert_eq!(r.confidence, 2);
        assert_eq!(r.verdict, "caution");
    }

    #[test]
    fn pagefile_is_danger() {
        let r = identify_impl("C:\\pagefile.sys", "file", 8 * 1024 * 1024 * 1024).unwrap();
        assert_eq!(r.verdict, "danger");
        assert!(r.label.contains("页面文件"), "label = {}", r.label);
    }

    #[test]
    fn steam_library_is_danger_and_has_summary() {
        let r = identify_impl("Z:\\SteamLibrary\\steamapps\\common", "dir", 48 * 1024 * 1024 * 1024).unwrap();
        assert_eq!(r.verdict, "danger");
        assert!(r.evidence.iter().any(|e| e.contains("steamapps")), "evidence = {:?}", r.evidence);
        // 摘要必须带路径，否则 AI 分析没有上下文
        assert!(r.summary.contains("路径："), "summary = {}", r.summary);
    }

    #[test]
    fn node_modules_is_cleanable() {
        let r = identify_impl("Z:\\proj\\node_modules", "dir", 500 * 1024 * 1024).unwrap();
        assert_eq!(r.verdict, "safe");
        assert_eq!(r.action_hint, "clean");
    }

    #[test]
    fn specific_sig_beats_broad_one() {
        // WinSxS 必须优先于宽泛的 Windows —— 签名表顺序写错会退化成「系统目录，谨慎」
        let r = identify_impl("Z:\\Windows\\WinSxS", "dir", 0).unwrap();
        assert_eq!(r.verdict, "danger");
        assert!(r.label.contains("WinSxS"), "label = {}", r.label);

        let r2 = identify_impl("Z:\\Windows\\System32", "dir", 0).unwrap();
        assert_eq!(r2.verdict, "danger");
        assert!(r2.label.contains("System32"), "label = {}", r2.label);
    }

    #[test]
    fn unknown_path_is_honest() {
        let r = identify_impl("Z:\\some\\odd\\folder", "dir", 1024).unwrap();
        assert_eq!(r.confidence, 0);
        assert_eq!(r.verdict, "unknown");
        // 判不出来时必须提示别删，绝不能给 safe
        assert!(
            r.consequence.contains("不要") || r.consequence.contains("确认"),
            "consequence = {}",
            r.consequence
        );
    }
}
