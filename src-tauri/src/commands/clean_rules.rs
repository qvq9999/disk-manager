//! 清理规则引擎（对齐 Winapp2.ini 的声明式语义）
//!
//! 参考来源（GitHub 上规则量最大、被最多清理软件共用的方案）：
//!   · MoscaDotTo/Winapp2 —— 社区维护 15 年、数千条规则，被 CCleaner / BleachBit /
//!     System Ninja / R-Wipe&Clean / HDCleaner 等共用
//!   · bleachbit/bleachbit —— CleanerML 与 Winapp2 解析器，规则与程序解耦
//!
//! 采纳的核心机制：
//!   ① DetectFile 门控：没装对应软件就不显示条目（避免一堆无关项）
//!   ② FileKey：显式路径 + 文件名模式 + RECURSE / REMOVESELF 标志
//!   ③ ExcludeKey：排除名单（防止误删）
//!   ④ 显式路径优先于「按扩展名全盘扫」——这是 Winapp2 反复强调的安全原则
//!   ⑤ 危险项带 Warning 且默认不勾选
//!
//! 规则全部为数据（`RULES` 表），新增规则只需加一行，不碰逻辑。

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Browser,
    Chat,
    Media,
    Game,
    Office,
    Dev,
    /// 应用缓存自动发现（覆盖未逐条建规则的 Electron / Chromium 系应用）
    AppCache,
    System,
    Update,
    Recycle,
    Used,
    /// 已卸载软件残留（Geek / HiBit 式孤儿目录检测）
    Residual,
}

impl Cat {
    pub fn id(self) -> &'static str {
        match self {
            Cat::Browser => "browser",
            Cat::Chat => "chat",
            Cat::Media => "media",
            Cat::Game => "game",
            Cat::Office => "office",
            Cat::Dev => "dev",
            Cat::AppCache => "appcache",
            Cat::System => "system",
            Cat::Update => "update",
            Cat::Recycle => "recycle",
            Cat::Used => "used",
            Cat::Residual => "residual",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Cat::Browser => "浏览器缓存",
            Cat::Chat => "通讯软件",
            Cat::Media => "影音娱乐",
            Cat::Game => "游戏平台",
            Cat::Office => "办公软件",
            Cat::Dev => "开发缓存",
            Cat::AppCache => "应用缓存（深度扫描）",
            Cat::System => "系统垃圾",
            Cat::Update => "系统更新残留",
            Cat::Recycle => "回收站",
            Cat::Used => "系统占用（不可删）",
            Cat::Residual => "卸载残留（软件已移除）",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Cat::Browser => "Globe",
            Cat::Chat => "MessageSquare",
            Cat::Media => "PlayCircle",
            Cat::Game => "Gamepad2",
            Cat::Office => "FileText",
            Cat::Dev => "Code",
            Cat::AppCache => "Layers",
            Cat::System => "ScrollText",
            Cat::Update => "Download",
            Cat::Recycle => "Trash2",
            Cat::Used => "HardDrive",
            Cat::Residual => "PackageX",
        }
    }
    pub const ALL: [Cat; 12] = [
        Cat::Browser, Cat::Chat, Cat::Media, Cat::Game, Cat::Office,
        Cat::Dev, Cat::AppCache, Cat::System, Cat::Update, Cat::Recycle,
        Cat::Used, Cat::Residual,
    ];
}

pub struct PathSpec {
    pub path: &'static str,
    pub pattern: &'static str,
    pub recurse: bool,
    /// REMOVESELF：删完文件后清理空的子目录
    pub remove_self: bool,
    /// 是否启用「配置文件保护闸」（见 `protected_file`）。
    /// 只对**整目录清空**类规则需要：像 UWP `LocalCache` 这种既是缓存又存应用数据的位置，
    /// 不加闸会把 `.dbf`/`Roaming` 里的设置一起清掉。
    pub protect: bool,
}

const fn fp(path: &'static str, pattern: &'static str, recurse: bool) -> PathSpec {
    PathSpec { path, pattern, recurse, remove_self: false, protect: false }
}

/// 带配置文件保护闸的路径项（用于整目录清空且混有用户数据的场景）
const fn fp_protect(path: &'static str, pattern: &'static str, recurse: bool) -> PathSpec {
    PathSpec { path, pattern, recurse, remove_self: false, protect: true }
}

pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    pub cat: Cat,
    /// DetectFile：任一存在才显示；空 = 无条件显示
    pub detect: &'static [&'static str],
    pub paths: &'static [PathSpec],
    pub exclude: &'static [&'static str],
    pub default_on: bool,
    pub advanced: bool,
    pub warning: Option<&'static str>,
    /// 是否为「逐盘」规则（路径含 %Drive%）
    pub per_drive: bool,
}

/// 通用排除（所有规则生效）：仅排除系统保护目录与锁文件
/// ⚠️ 不能把 $RECYCLE.BIN 放进通用排除，否则回收站规则会把自己排除掉（曾踩过）
const EXCL_COMMON: &[&str] = &["System Volume Information", "*.lock"];

/// `Windows\Logs` 专用排除。
///
/// `drive.winlogs` 是整棵 `%Drive%\Windows\Logs` 递归 `*.*`，而全局规则 `win.wulogs`
/// 已经单独覆盖了 `Logs\WindowsUpdate`（只取 etl/log）。两条规则同时生效时，
/// 同一批更新日志会被统计两次 —— 「可清理总量」凭空多出一份，而清理时第二遍
/// 只能删到空气（metadata 取不到），于是出现「说有 20GB，只释放 12GB」。
const EXCL_WINLOGS: &[&str] = &["System Volume Information", "*.lock", "WindowsUpdate"];

// ---------------------------------------------------------------- 规则表
pub static RULES: &[Rule] = &[
    // ======================= 浏览器 =======================
    Rule {
        id: "chrome.cache", name: "Google Chrome 缓存", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\Google\\Chrome\\User Data"],
        paths: &[
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Code Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\GPUCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Media Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Service Worker\\CacheStorage", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\Crashpad", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "edge.cache", name: "Microsoft Edge 缓存", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\Microsoft\\Edge\\User Data"],
        paths: &[
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Code Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\GPUCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Media Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Service Worker\\CacheStorage", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\Crashpad", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "firefox.cache", name: "Firefox 缓存", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles"],
        paths: &[
            fp("%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\cache2", "*.*", true),
            fp("%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\startupCache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "browser360.cache", name: "360 浏览器缓存", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\360Chrome\\Chrome\\User Data"],
        paths: &[
            fp("%LOCALAPPDATA%\\360Chrome\\Chrome\\User Data\\*\\Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\360Chrome\\Chrome\\User Data\\*\\Code Cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "qqbrowser.cache", name: "QQ 浏览器缓存", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\Tencent\\QQBrowser\\User Data"],
        paths: &[
            fp("%LOCALAPPDATA%\\Tencent\\QQBrowser\\User Data\\*\\Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Tencent\\QQBrowser\\User Data\\*\\Code Cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "inetcache", name: "Internet 临时文件（IE/系统）", cat: Cat::Browser,
        detect: &["%LOCALAPPDATA%\\Microsoft\\Windows\\INetCache"],
        paths: &[fp("%LOCALAPPDATA%\\Microsoft\\Windows\\INetCache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        // 浏览器把着色器缓存放在 User Data **根目录**（不是 profile 子目录），
        // 上面的 chrome/edge 规则只覆盖 profile 内的 GPUCache，这几项一直漏着。
        // 归显卡相关：默认不勾（与 app_cache 的 gpu 判定口径一致）。
        id: "browser.shadercache", name: "浏览器着色器缓存（显卡相关）", cat: Cat::Browser,
        detect: &[
            "%LOCALAPPDATA%\\Google\\Chrome\\User Data",
            "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data",
        ],
        paths: &[
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\ShaderCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\GrShaderCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\GPUPersistentCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Google\\Chrome\\User Data\\old_GrShaderCache_*", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\ShaderCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\GrShaderCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\GPUPersistentCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\old_GrShaderCache_*", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("显卡相关缓存（着色器预编译）：删除后页面首次渲染要重新编译，可能短暂卡顿。默认不勾选。"),
        per_drive: false,
    },

    // ======================= 通讯 =======================
    Rule {
        // ⚠️ 路径曾经写成 `%APPDATA%\Tencent\xwechat_files` —— 那是错的。
        //    微信 4.x 的数据目录默认在 `%USERPROFILE%\Documents\xwechat_files`（可在设置里改盘），
        //    缓存子目录叫 `cache`（按 YYYY-MM 分月）与 `temp\head_image`，没有 `wxcache`。
        //    旧写法永远不命中，等于这条规则从未生效。
        id: "wechat.cache", name: "微信缓存（不含聊天记录）", cat: Cat::Chat,
        detect: &["%USERPROFILE%\\Documents\\xwechat_files"],
        paths: &[
            fp("%USERPROFILE%\\Documents\\xwechat_files\\*\\cache", "*.*", true),
            fp("%USERPROFILE%\\Documents\\xwechat_files\\*\\temp\\head_image", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("只清朋友圈/视频号缓存与头像缓存。聊天图片、视频、文件与聊天数据库（db_storage）属于你的数据，不在清理范围内 —— 那些删了就永久丢失。"),
        per_drive: false,
    },
    Rule {
        id: "wecom.cache", name: "企业微信缓存", cat: Cat::Chat,
        detect: &["%APPDATA%\\Tencent\\WXWork"],
        paths: &[
            fp("%APPDATA%\\Tencent\\WXWork\\*\\Cache", "*.*", true),
            fp("%APPDATA%\\Tencent\\WXWork\\*\\Emotion", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "qq.cache", name: "QQ 缓存与临时文件", cat: Cat::Chat,
        detect: &["%LOCALAPPDATA%\\Tencent\\QQ", "%APPDATA%\\Tencent\\QQ"],
        paths: &[
            fp("%LOCALAPPDATA%\\Tencent\\QQ\\Temp", "*.*", true),
            // NT 版把更新包暂存在这，删了下次更新重新下，不影响使用
            fp("%APPDATA%\\Tencent\\QQ\\Temp\\QQNTdownloadcache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        // QQNT（2023 年后的新版本）缓存位置和老 QQ 不同：在 Roaming 下，
        // 其中 WebView2 是内置浏览器缓存，社区实测常到 2~5GB。聊天记录在 `QQ\NT\User\`，不在此列。
        id: "qqnt.cache", name: "QQ（NT 新版）内置浏览器缓存", cat: Cat::Chat,
        detect: &["%APPDATA%\\Tencent\\QQ\\NT"],
        paths: &[
            fp("%APPDATA%\\Tencent\\QQ\\Cache", "*.*", true),
            fp("%APPDATA%\\Tencent\\QQ\\WebView2", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("含内置浏览器（WebView2）缓存，删除后首次打开小程序/公众号文章会重新加载；聊天记录与接收的文件不在范围内。"),
        per_drive: false,
    },
    Rule {
        id: "dingtalk.cache", name: "钉钉缓存", cat: Cat::Chat,
        detect: &["%LOCALAPPDATA%\\DingTalk"],
        paths: &[
            fp("%LOCALAPPDATA%\\DingTalk\\cache", "*.*", true),
            fp("%LOCALAPPDATA%\\DingTalk\\media_cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "telegram.cache", name: "Telegram 缓存", cat: Cat::Chat,
        detect: &["%APPDATA%\\Telegram Desktop"],
        paths: &[fp("%APPDATA%\\Telegram Desktop\\tdata\\user_data\\cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "discord.cache", name: "Discord 缓存", cat: Cat::Chat,
        detect: &["%APPDATA%\\discord"],
        paths: &[
            fp("%APPDATA%\\discord\\Cache", "*.*", true),
            fp("%APPDATA%\\discord\\Code Cache", "*.*", true),
            fp("%APPDATA%\\discord\\GPUCache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },

    // ======================= 影音 =======================
    Rule {
        id: "netease.cache", name: "网易云音乐缓存", cat: Cat::Media,
        detect: &["%LOCALAPPDATA%\\Netease\\CloudMusic"],
        paths: &[
            fp("%LOCALAPPDATA%\\Netease\\CloudMusic\\Cache", "*.*", true),
            fp("%LOCALAPPDATA%\\Netease\\CloudMusic\\webdata\\cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "qqmusic.cache", name: "QQ 音乐缓存", cat: Cat::Media,
        detect: &["%LOCALAPPDATA%\\Tencent\\QQMusic"],
        paths: &[fp("%LOCALAPPDATA%\\Tencent\\QQMusic\\Cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "bilibili.cache", name: "哔哩哔哩客户端缓存", cat: Cat::Media,
        // detect 必须覆盖两个可能的安装目录名：只认英文目录时，
        // 装了中文目录版（`哔哩哔哩`）的用户整条规则都不显示
        detect: &["%LOCALAPPDATA%\\bilibili", "%LOCALAPPDATA%\\哔哩哔哩"],
        paths: &[
            fp("%LOCALAPPDATA%\\bilibili\\cache", "*.*", true),
            fp("%LOCALAPPDATA%\\哔哩哔哩\\cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "potplayer.cache", name: "PotPlayer 日志与缩略图", cat: Cat::Media,
        detect: &["%APPDATA%\\PotPlayerMini64"],
        paths: &[
            fp("%APPDATA%\\PotPlayerMini64\\Log", "*.*", true),
            fp("%APPDATA%\\PotPlayerMini64\\Thumbnail", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "vlc.cache", name: "VLC 封面缓存", cat: Cat::Media,
        detect: &["%APPDATA%\\vlc"],
        paths: &[fp("%APPDATA%\\vlc\\art", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },

    // ======================= 游戏 =======================
    Rule {
        id: "steam.cache", name: "Steam 网页缓存与日志", cat: Cat::Game,
        // 三个 detect 都要有：Steam 可能装在 Program Files / (x86) / 其它盘，
        // 而新版客户端的网页缓存固定写在 %LOCALAPPDATA%\Steam
        detect: &[
            "%PROGRAMFILES(X86)%\\Steam",
            "%PROGRAMFILES%\\Steam",
            "%LOCALAPPDATA%\\Steam",
        ],
        paths: &[
            fp("%PROGRAMFILES(X86)%\\Steam\\appcache", "*.*", true),
            fp("%PROGRAMFILES(X86)%\\Steam\\htmlcache", "*.*", true),
            fp("%PROGRAMFILES(X86)%\\Steam\\logs", "*.log", true),
            fp("%PROGRAMFILES(X86)%\\Steam\\config\\htmlcache", "*.*", true),
            fp("%PROGRAMFILES%\\Steam\\appcache", "*.*", true),
            fp("%PROGRAMFILES%\\Steam\\htmlcache", "*.*", true),
            fp("%PROGRAMFILES%\\Steam\\logs", "*.log", true),
            // 新客户端（CEF）把网页缓存放用户目录，安装目录下反而没有
            fp("%LOCALAPPDATA%\\Steam\\htmlcache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "steam.depotcache", name: "Steam 下载缓存（会重新下载）", cat: Cat::Game,
        detect: &[
            "%PROGRAMFILES(X86)%\\Steam\\depotcache",
            "%PROGRAMFILES%\\Steam\\depotcache",
        ],
        paths: &[
            fp("%PROGRAMFILES(X86)%\\Steam\\depotcache", "*.*", true),
            fp("%PROGRAMFILES%\\Steam\\depotcache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后更新/校验游戏时会重新下载，可能很耗时"), per_drive: false,
    },
    Rule {
        id: "epic.cache", name: "Epic 启动器缓存", cat: Cat::Game,
        detect: &["%LOCALAPPDATA%\\EpicGamesLauncher"],
        paths: &[
            fp("%LOCALAPPDATA%\\EpicGamesLauncher\\Saved\\webcache", "*.*", true),
            fp("%LOCALAPPDATA%\\EpicGamesLauncher\\Saved\\Logs", "*.log", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "wegame.cache", name: "WeGame 缓存", cat: Cat::Game,
        detect: &["%LOCALAPPDATA%\\Tencent\\WeGame"],
        paths: &[fp("%LOCALAPPDATA%\\Tencent\\WeGame\\Cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "gamepp.cache", name: "游戏加加 日志与缓存", cat: Cat::Game,
        detect: &["%LOCALAPPDATA%\\GamePP", "%PROGRAMDATA%\\GamePP"],
        paths: &[
            fp("%LOCALAPPDATA%\\GamePP\\cache", "*.*", true),
            fp("%LOCALAPPDATA%\\GamePP\\log", "*.log", true),
            fp("%LOCALAPPDATA%\\GamePP\\logs", "*.log", true),
            fp("%PROGRAMDATA%\\GamePP\\log", "*.log", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },

    // ======================= 办公 =======================
    Rule {
        id: "office.filecache", name: "Office 文件缓存", cat: Cat::Office,
        detect: &["%LOCALAPPDATA%\\Microsoft\\Office"],
        paths: &[fp("%LOCALAPPDATA%\\Microsoft\\Office\\16.0\\OfficeFileCache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "wps.cache", name: "WPS 缓存与日志", cat: Cat::Office,
        detect: &["%APPDATA%\\kingsoft"],
        paths: &[
            fp("%APPDATA%\\kingsoft\\office6\\cache", "*.*", true),
            fp("%APPDATA%\\kingsoft\\office6\\log", "*.log", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "adobe.cache", name: "Adobe 媒体缓存", cat: Cat::Office,
        detect: &["%APPDATA%\\Adobe\\Common"],
        paths: &[
            fp("%APPDATA%\\Adobe\\Common\\Media Cache Files", "*.*", true),
            fp("%APPDATA%\\Adobe\\Common\\Media Cache", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "jianying.cache", name: "剪映草稿缓存", cat: Cat::Office,
        detect: &["%LOCALAPPDATA%\\JianyingPro"],
        paths: &[fp("%LOCALAPPDATA%\\JianyingPro\\User Data\\Cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },

    // ======================= 开发 =======================
    Rule {
        id: "npm.cache", name: "npm 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\npm-cache"],
        paths: &[fp("%LOCALAPPDATA%\\npm-cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "pnpm.cache", name: "pnpm 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\pnpm-cache", "%LOCALAPPDATA%\\pnpm\\store"],
        paths: &[
            fp("%LOCALAPPDATA%\\pnpm-cache", "*.*", true),
            fp("%LOCALAPPDATA%\\pnpm\\store", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "yarn.cache", name: "Yarn 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\Yarn\\Cache"],
        paths: &[fp("%LOCALAPPDATA%\\Yarn\\Cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "pip.cache", name: "pip 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\pip\\cache"],
        paths: &[fp("%LOCALAPPDATA%\\pip\\cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "nuget.cache", name: "NuGet 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\NuGet\\v3-cache"],
        paths: &[fp("%LOCALAPPDATA%\\NuGet\\v3-cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "gradle.cache", name: "Gradle 缓存", cat: Cat::Dev,
        detect: &["%USERPROFILE%\\.gradle\\caches"],
        paths: &[fp("%USERPROFILE%\\.gradle\\caches", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后首次构建需重新下载依赖"), per_drive: false,
    },
    Rule {
        id: "maven.cache", name: "Maven 本地仓库", cat: Cat::Dev,
        detect: &["%USERPROFILE%\\.m2\\repository"],
        paths: &[fp("%USERPROFILE%\\.m2\\repository", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("这是本地依赖仓库，删除后离线构建会失败，需重新下载"), per_drive: false,
    },
    Rule {
        id: "cargo.cache", name: "Cargo 下载缓存", cat: Cat::Dev,
        detect: &["%USERPROFILE%\\.cargo\\registry\\cache"],
        paths: &[fp("%USERPROFILE%\\.cargo\\registry\\cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "go.cache", name: "Go 构建缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\go-build"],
        paths: &[fp("%LOCALAPPDATA%\\go-build", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "vscode.cache", name: "VS Code 缓存与日志", cat: Cat::Dev,
        detect: &["%APPDATA%\\Code"],
        paths: &[
            fp("%APPDATA%\\Code\\Cache", "*.*", true),
            fp("%APPDATA%\\Code\\CachedData", "*.*", true),
            fp("%APPDATA%\\Code\\Code Cache", "*.*", true),
            fp("%APPDATA%\\Code\\GPUCache", "*.*", true),
            fp("%APPDATA%\\Code\\logs", "*.log", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "jetbrains.cache", name: "JetBrains IDE 缓存与日志", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\JetBrains"],
        paths: &[
            fp("%LOCALAPPDATA%\\JetBrains\\*\\caches", "*.*", true),
            fp("%LOCALAPPDATA%\\JetBrains\\*\\log", "*.log", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后 IDE 首次打开项目需重建索引"), per_drive: false,
    },
    Rule {
        id: "unity.cache", name: "Unity 缓存", cat: Cat::Dev,
        detect: &["%LOCALAPPDATA%\\Unity\\cache"],
        paths: &[fp("%LOCALAPPDATA%\\Unity\\cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },

    // ======================= 系统（用户态，非逐盘） =======================
    Rule {
        id: "temp.user", name: "用户临时文件", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\Temp"],
        paths: &[fp("%LOCALAPPDATA%\\Temp", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        // Win11 起缩略图并入 iconcache_*.db（实测 Win11 的 Explorer 目录里
        // 一个 thumbcache_*.db 都没有，只有 8 个 iconcache_*.db）——
        // 只清 thumbcache 的话这条规则在新系统上命中 0。
        id: "thumbcache", name: "缩略图与图标缓存", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer"],
        paths: &[
            fp("%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "thumbcache_*.db", false),
            fp("%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "iconcache_*.db", false),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false,
        warning: Some("图标缓存常被资源管理器占用而删除失败；需要时重启资源管理器（任务管理器 → 重启 Windows 资源管理器）后重试"),
        per_drive: false,
    },
    Rule {
        id: "crashdumps", name: "应用崩溃转储", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\CrashDumps"],
        paths: &[fp("%LOCALAPPDATA%\\CrashDumps", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "uwp.cache", name: "微软商店应用缓存（保留应用设置）", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\Packages"],
        // ⚠️ LocalCache 既是缓存目录，也是 UWP 的 ApplicationData 之一：
        //    其中 `Roaming` 存应用设置、`*.dbf` 是应用数据库，必须走保护闸
        paths: &[fp_protect("%LOCALAPPDATA%\\Packages\\*\\LocalCache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        // 微信 4.x 的安装包暂存目录，常留几百 MB；删了下次更新重新下
        id: "xwechat.update", name: "微信更新包缓存", cat: Cat::System,
        detect: &["%APPDATA%\\Tencent\\xwechat\\update"],
        paths: &[fp("%APPDATA%\\Tencent\\xwechat\\update", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "onedrive.logs", name: "OneDrive 同步日志", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\Microsoft\\OneDrive\\logs"],
        paths: &[fp("%LOCALAPPDATA%\\Microsoft\\OneDrive\\logs", "*.log", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true, warning: None, per_drive: false,
    },
    Rule {
        id: "d3dshader", name: "DirectX 着色器缓存（游戏会重编译）", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\D3DSCache"],
        paths: &[fp("%LOCALAPPDATA%\\D3DSCache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后游戏首次启动会重新编译着色器，进游戏变慢"), per_drive: false,
    },

    Rule {
        id: "win.webcache", name: "Windows 资源管理器缓存", cat: Cat::System,
        detect: &["%LOCALAPPDATA%\\Microsoft\\Windows\\WebCache"],
        paths: &[
            fp("%LOCALAPPDATA%\\Microsoft\\Windows\\WebCache", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Windows\\Caches", "*.*", true),
            fp("%LOCALAPPDATA%\\Microsoft\\Windows\\Notifications", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "win.wulogs", name: "Windows 更新日志", cat: Cat::System,
        detect: &["%WINDIR%\\Logs\\WindowsUpdate"],
        paths: &[fp("%WINDIR%\\Logs\\WindowsUpdate", "*.etl", true), fp("%WINDIR%\\Logs\\WindowsUpdate", "*.log", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: false,
    },
    Rule {
        id: "vs.packagecache", name: "Visual Studio 安装包缓存", cat: Cat::Dev,
        detect: &["%PROGRAMDATA%\\Package Cache", "%PROGRAMDATA%\\Microsoft\\VisualStudio\\Packages"],
        paths: &[
            fp("%PROGRAMDATA%\\Package Cache", "*.*", true),
            fp("%PROGRAMDATA%\\Microsoft\\VisualStudio\\Packages", "*.*", true),
        ],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后修复/修改 VS 组件时需要重新下载安装包"), per_drive: false,
    },
    Rule {
        id: "recent.items", name: "最近使用记录（隐私）", cat: Cat::System,
        detect: &["%APPDATA%\\Microsoft\\Windows\\Recent"],
        paths: &[fp("%APPDATA%\\Microsoft\\Windows\\Recent", "*.*", false)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("清空「最近使用的文件」列表，不影响文件本身"), per_drive: false,
    },

    // ======================= 系统（逐盘） =======================
    Rule {
        id: "drive.wintemp", name: "Windows 临时文件", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\Temp", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        // Win11 22H2+ 新增：系统服务专用的临时目录，与 Windows\Temp 并列存在
        // （本机实测 C:\Windows\SystemTemp 存在）。漏掉它等于漏了一整类系统临时文件。
        id: "drive.systtemp", name: "Windows 系统服务临时文件（SystemTemp）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\SystemTemp", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.winlogs", name: "Windows 系统日志", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\Logs", "*.*", true)],
        // 排除 WindowsUpdate：那一份由全局规则 win.wulogs 负责，否则重复统计
        exclude: EXCL_WINLOGS, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.winupdate", name: "Windows 更新缓存", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\SoftwareDistribution\\Download", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.winminidump", name: "Windows 蓝屏转储", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\Minidump", "*.*", true), fp("%Drive%\\Windows", "MEMORY.DMP", false)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.winprefetch", name: "Windows Prefetch 预读", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\Prefetch", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.wer", name: "Windows 错误报告", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\ProgramData\\Microsoft\\Windows\\WER", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.fontcache", name: "系统字体缓存", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\ServiceProfiles\\LocalService\\AppData\\Local\\FontCache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.deliveryopt", name: "Windows 传递优化缓存", cat: Cat::System,
        detect: &[],
        paths: &[fp("%Drive%\\Windows\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.setupapi", name: "系统安装日志（setupapi）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Windows\\INF", "setupapi.*.log", false)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.perflogs", name: "性能日志目录（PerfLogs）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\PerfLogs", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.configmsi", name: "MSI 安装残留（Config.Msi）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Config.Msi", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("若正处于安装过程中请勿清理"), per_drive: true,
    },
    Rule {
        id: "drive.temp", name: "根目录临时文件夹（Temp）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Temp", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("非系统创建的根目录 Temp 可能存放你自己的临时文件，请先确认内容"), per_drive: true,
    },
    Rule {
        id: "drive.tmp", name: "根目录临时文件夹（tmp）", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\tmp", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("非系统创建的根目录 tmp 可能存放你自己的临时文件，请先确认内容"), per_drive: true,
    },

    // 驱动安装包残留（高风险，默认不勾）
    Rule {
        id: "drive.nvidia", name: "NVIDIA 驱动安装残留", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\NVIDIA", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("驱动安装包解压残留，删除不影响已安装驱动"), per_drive: true,
    },
    Rule {
        id: "drive.amd", name: "AMD 驱动安装残留", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\AMD", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("驱动安装包解压残留，删除不影响已安装驱动"), per_drive: true,
    },
    Rule {
        id: "drive.intel", name: "Intel 驱动安装残留", cat: Cat::System,
        detect: &[], paths: &[fp("%Drive%\\Intel", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("驱动安装包解压残留，删除不影响已安装驱动"), per_drive: true,
    },

    // ======================= 系统更新残留（逐盘） =======================
    Rule {
        id: "drive.winre", name: "恢复环境更新残留（$WinREAgent）", cat: Cat::Update,
        detect: &[], paths: &[fp("%Drive%\\$WinREAgent", "*.*", true)],
        exclude: EXCL_COMMON, default_on: true, advanced: false,
        warning: Some("Windows 更新过程中生成的恢复环境暂存文件，删除不影响已安装系统"), per_drive: true,
    },
    Rule {
        id: "drive.winold", name: "旧 Windows 安装（Windows.old）", cat: Cat::Update,
        detect: &[], paths: &[fp("%Drive%\\Windows.old", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true,
        warning: Some("删除后无法回滚到上一版本 Windows；其中的个人文件请在删除前先取出"), per_drive: true,
    },
    Rule {
        id: "drive.bt", name: "升级残留（$WINDOWS.~BT）", cat: Cat::Update,
        detect: &[], paths: &[fp("%Drive%\\$WINDOWS.~BT", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true, warning: None, per_drive: true,
    },
    Rule {
        id: "drive.ws", name: "升级临时文件（$WINDOWS.~WS）", cat: Cat::Update,
        detect: &[], paths: &[fp("%Drive%\\$WINDOWS.~WS", "*.*", true)],
        exclude: EXCL_COMMON, default_on: false, advanced: true, warning: None, per_drive: true,
    },

    // ======================= 回收站（逐盘） =======================
    Rule {
        id: "recycle", name: "回收站", cat: Cat::Recycle,
        detect: &[], paths: &[fp("%Drive%\\$Recycle.Bin", "*.*", true)],
        exclude: &["System Volume Information"], default_on: true, advanced: false,
        warning: None, per_drive: true,
    },
];

/// 系统占用文件（只读展示，不可删）
pub static USED_FILES: &[(&'static str, &'static str, &'static str)] = &[
    ("pagefile.sys", "页面文件（虚拟内存）", "系统关键文件；可在「系统属性 → 高级 → 虚拟内存」中调整或移到其他盘"),
    ("hiberfil.sys", "休眠文件", "不用休眠时可执行 powercfg /h off 释放该空间（会同时关闭快速启动）"),
    ("swapfile.sys", "UWP 交换文件", "系统关键文件，不建议删除"),
    ("DumpStack.log.tmp", "转储堆栈日志", "系统关键文件，不建议删除"),
];

// ---------------------------------------------------------------- 引擎实现

fn env_pairs() -> Vec<(&'static str, String)> {
    let keys = [
        "LOCALAPPDATA", "APPDATA", "PROGRAMDATA", "WINDIR", "SystemRoot",
        "PROGRAMFILES", "ProgramW6432", "ProgramFiles(x86)", "USERPROFILE", "TEMP",
    ];
    let mut out = Vec::new();
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                out.push((k, v));
            }
        }
    }
    out
}

/// 解析单个占位符（`%NAME%` 里的 NAME）。**大小写不敏感**。
///
/// 曾经用 `str::replace("%PROGRAMFILES(X86)%", ...)` 逐条替换，而 env_pairs 产出的是
/// `%ProgramFiles(x86)%` —— 精确匹配失败，于是 Steam 那两条规则的路径永远不会展开，
/// `exists()` 恒假，规则静默失效（装了 Steam 也扫不出一个字节）。
fn lookup_token(token: &str, drive: Option<char>) -> Option<String> {
    if token.eq_ignore_ascii_case("drive") {
        return drive.map(|d| format!("{d}:"));
    }
    for (k, v) in env_pairs() {
        if k.eq_ignore_ascii_case(token) {
            return Some(v);
        }
    }
    None
}

fn subst(raw: &str, drive: Option<char>) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    let mut rest = raw;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else {
            out.push_str(&rest[i..]); // 落单的 % ：原样输出
            return out;
        };
        let token = &after[..j];
        match lookup_token(token, drive) {
            Some(v) => out.push_str(&v),
            // 未知占位符原样保留：让路径展开成「不存在」而跳过，
            // 绝不要静默拼出一个看起来合法、实则指向别处的路径
            None => {
                out.push('%');
                out.push_str(token);
                out.push('%');
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// 简单 glob：仅支持 `*`，大小写不敏感
pub fn glob_match(name: &str, pat: &str) -> bool {
    let n = name.to_lowercase();
    let p = pat.to_lowercase();
    if p == "*" || p == "*.*" {
        return true;
    }
    if !p.contains('*') {
        return n == p;
    }
    let parts: Vec<&str> = p.split('*').collect();
    let mut pos = 0usize;
    for (i, seg) in parts.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        if i == 0 {
            if !n.starts_with(seg) {
                return false;
            }
            pos = seg.len();
        } else if i == parts.len() - 1 {
            if n.len() < pos + seg.len() {
                return false;
            }
            return n[pos..].ends_with(seg);
        } else {
            match n[pos..].find(seg) {
                Some(idx) => pos += idx + seg.len(),
                None => return false,
            }
        }
    }
    true
}

/// 把带通配符的路径展开为真实路径列表（逐段解析 `*`）
pub fn expand_paths(raw: &str, drive: Option<char>) -> Vec<PathBuf> {
    let s = subst(raw, drive);
    let parts: Vec<&str> = s.split('\\').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let mut bases: Vec<PathBuf> = {
        let first = parts[0];
        if first.len() == 2 && first.ends_with(':') {
            vec![PathBuf::from(format!("{first}\\"))]
        } else {
            vec![PathBuf::from(first)]
        }
    };
    for seg in &parts[1..] {
        let mut next = Vec::new();
        for b in &bases {
            if seg.contains('*') {
                if let Ok(rd) = std::fs::read_dir(b) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if glob_match(&n, seg) {
                            next.push(b.join(&n));
                        }
                    }
                }
            } else {
                next.push(b.join(seg));
            }
        }
        bases = next;
        if bases.is_empty() {
            break;
        }
    }
    bases
}

/// 配置文件保护闸：命中即跳过删除。
///
/// 背景（实测）：`%LOCALAPPDATA%\Packages\<包>\LocalCache` 是 UWP 的 ApplicationData
/// 之一，里面既有可再生的缓存，也有 `Roaming`（应用设置）与 `.dbf` 数据库。
/// 整目录清空会把应用设置一并抹掉，用户看到的就是「软件里的个人设置全没了」。
///
/// 判据刻意宽松（宁可少清几百 KB 缓存，也不能错删一份设置）：
///   ① 相对路径中任一段叫 `Roaming` → 整枝保护（UWP 设置区）
///   ② 扩展名属于配置/数据库类 → 保护
///   ③ 文件名精确等于 Chromium profile 的关键文件 → 保护（兜底）
const PROTECT_EXTS: &[&str] = &[
    "ini", "cfg", "conf", "json", "xml", "yaml", "yml", "toml",
    "db", "db3", "sqlite", "sqlite3", "dbf", "mdb",
    "reg", "plist", "properties", "settings", "prefs", "dat",
    "key", "pem", "pfx", "sav",
];

/// Chromium profile 的关键文件名（无扩展名，靠名字识别）
const PROTECT_NAMES: &[&str] = &[
    "login data", "cookies", "preferences", "secure preferences", "history",
    "web data", "local state", "bookmarks", "top sites", "visited links",
    "sync data", "affiliation database", "network action predictor",
];

/// `rel` 是被清理根目录下的相对路径（不是绝对路径）
pub fn protected_file(rel: &Path) -> bool {
    // ① Roaming 子树（UWP 应用设置）
    for comp in rel.components() {
        if comp.as_os_str().to_string_lossy().eq_ignore_ascii_case("roaming") {
            return true;
        }
    }
    // ② 配置 / 数据库类扩展名
    let ext = rel
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !ext.is_empty() && PROTECT_EXTS.iter().any(|x| *x == ext) {
        return true;
    }
    // ③ Chromium profile 关键文件（兜底）
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    PROTECT_NAMES.iter().any(|x| *x == name)
}

/// 单个文件是否因「配置文件保护闸」而跳过
fn spec_protects(spec: &PathSpec, root: &Path, file: &Path) -> bool {
    if !spec.protect {
        return false;
    }
    let rel = file.strip_prefix(root).unwrap_or(file);
    protected_file(rel)
}

fn is_excluded(path: &Path, extra: &[&str]) -> bool {
    for comp in path.components() {
        let name = comp.as_os_str().to_string_lossy();
        for pat in extra.iter().chain(EXCL_COMMON.iter()) {
            if glob_match(&name, pat) {
                return true;
            }
        }
    }
    false
}

/// DetectFile 判定：任一存在即认为该软件已安装
pub fn detect_ok(rule: &Rule) -> bool {
    if rule.detect.is_empty() {
        return true;
    }
    rule.detect.iter().any(|d| {
        expand_paths(d, None).iter().any(|p| p.exists())
    })
}

/// 统计一条规则在某个盘上的可清理体积。
///
/// ⚠️ 口径由 `fsize::Tally` 统一决定（AllocationSize + 硬链接不计），
/// 与 `delete_rule` 用的是**同一个函数** —— 否则「显示 8MB、实际清 0B」会再次出现。
pub fn scan_rule(rule: &Rule, drive: Option<char>) -> super::fsize::Tally {
    let mut total = super::fsize::Tally::default();
    for spec in rule.paths {
        for root in expand_paths(spec.path, drive) {
            if !root.exists() {
                continue;
            }
            let mut walker = walkdir::WalkDir::new(&root).follow_links(false);
            if !spec.recurse {
                walker = walker.max_depth(1);
            }
            for e in walker.into_iter() {
                // ⚠️ 不能用 filter_map(|e| e.ok())：访问被拒的目录会被整棵丢掉，
                //    扫描数字悄悄偏小，界面却显示"没有可清理项"。必须计数并透出。
                let e = match e {
                    Ok(e) => e,
                    Err(_) => {
                        total.denied_dirs += 1;
                        continue;
                    }
                };
                if !e.file_type().is_file() {
                    continue;
                }
                let name = e.file_name().to_string_lossy();
                if !glob_match(&name, spec.pattern) {
                    continue;
                }
                if is_excluded(e.path(), rule.exclude) {
                    continue;
                }
                // 统计口径必须与删除口径一致，否则「显示 8MB、实际清 0B」
                if spec_protects(spec, &root, e.path()) {
                    continue;
                }
                total.add_stat(e.path());
            }
        }
    }
    total
}

/// 删除一条规则在某盘上的匹配文件
pub fn delete_rule<F: FnMut(u64, &str)>(
    rule: &Rule,
    drive: Option<char>,
    freed: &mut u64,
    skip: &mut super::fsize::SkipStats,
    mut on_progress: F,
) {
    for spec in rule.paths {
        for root in expand_paths(spec.path, drive) {
            if !root.exists() {
                continue;
            }
            let mut walker = walkdir::WalkDir::new(&root).follow_links(false);
            if !spec.recurse {
                walker = walker.max_depth(1);
            }
            let mut files: Vec<PathBuf> = Vec::new();
            let mut dirs: Vec<PathBuf> = Vec::new();
            for e in walker.into_iter() {
                let e = match e {
                    Ok(e) => e,
                    Err(_) => {
                        // 读不进去的目录：删除阶段同样无法处理，如实计入「权限不足」
                        skip.denied_dirs += 1;
                        continue;
                    }
                };
                if e.file_type().is_dir() {
                    if spec.remove_self && e.depth() > 0 {
                        dirs.push(e.path().to_path_buf());
                    }
                    continue;
                }
                let name = e.file_name().to_string_lossy();
                if !glob_match(&name, spec.pattern) {
                    continue;
                }
                if is_excluded(e.path(), rule.exclude) {
                    continue;
                }
                if spec_protects(spec, &root, e.path()) {
                    continue;
                }
                files.push(e.path().to_path_buf());
            }
            for (i, f) in files.into_iter().enumerate() {
                // 每 256 个文件查一次取消标志：单次检查是原子读，开销可忽略，
                // 但能把「点停止」的响应延迟压到毫秒级（否则要等整个分类删完）
                if i % 256 == 0 && super::clean::cancel_requested() {
                    return;
                }
                // fsx 里统一记账：AllocationSize 口径、硬链接不计、错误码分类。
                // 缓存目录里「被占用」与「权限不足」都很常见，以前它们都被统称为"权限不足"。
                let before = *freed;
                super::fsx::remove_file_tally(&f, freed, skip);
                if *freed != before {
                    on_progress(
                        *freed,
                        &f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    );
                }
            }
            if spec.remove_self {
                // 由深到浅删空目录
                dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
                for d in dirs {
                    let empty = std::fs::read_dir(&d).map(|mut r| r.next().is_none()).unwrap_or(false);
                    if empty {
                        let _ = std::fs::remove_dir(&d);
                    }
                }
            }
        }
    }
}

pub fn rule_by_id(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// 静态规则实际覆盖到的路径集合。
///
/// 用途：应用缓存自动发现要跳过这些路径 —— 否则同一批文件会被统计两次，
/// 导致「可清理总量」虚高，清理时还会因为二次删除失败把 failed 计数撑起来。
pub fn covered_cache_paths(drives: &[char]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for rule in RULES {
        if !detect_ok(rule) {
            continue;
        }
        for spec in rule.paths {
            if rule.per_drive {
                for &d in drives {
                    out.extend(expand_paths(spec.path, Some(d)));
                }
            } else {
                out.extend(expand_paths(spec.path, None));
            }
        }
    }
    out
}

// ============================================================ 卸载残留识别
//
// Geek Uninstaller / HiBit 的核心能力之一：软件已从「已安装列表」消失，
// 但 Program Files / AppData 里还留着它的目录（多半是卸载器没清干净，
// 或用户当初就跳过了清理）。
//
// 判定策略（**保守，宁可漏报也绝不误删**）：
//   ① 先扫两个 Uninstall 根键，收集全部 DisplayName / Publisher /
//      InstallLocation 的词元，建「已安装软件关键词集」；
//   ② 逐个枚举 Program Files / Program Files (x86) / AppData /
//      LocalAppData / ProgramData 的顶层目录；
//   ③ 目录名命中任一关键词 → 认为软件仍在，跳过；
//   ④ 目录名落在系统白名单（Common Files / WindowsApps / Package Cache …）→ 跳过；
//   ⑤ 其余目录且体积 / 文件数达阈值 → 判为「疑似卸载残留」，
//      **默认不勾选 + 带明确警告**，由用户确认后再删。

/// 系统 / 共享目录白名单：这些目录不是「某个软件的残留」，删了会出事
const RESIDUAL_SKIP_DIRS: &[&str] = &[
    "common files", "windowsapps", "microsoft", "package cache", "programs",
    "temp", "crashdumps", "tempstate", "microsoft.net", "windows kits",
    "dotnet", "nvidia corporation", "nvidia", "amd", "intel", "reference assemblies",
    "packagemanagement", "microsoft sdks", "microsoft shared", "windows webcache",
    "installer", "microsoft office", "microsoft onedrive", "microsoft edge",
    "uninstall", "complus", "packages", "grouppolicy", "winsxs",
    "assembly", "gac", "bluestacks", "tencent", "baidu",
];

/// 名字里带这些词的目录一律跳过（游戏存档 / 用户数据，删了就是丢数据）
const RESIDUAL_SKIP_WORDS: &[&str] = &[
    "save", "saves", "savegame", "saved games", "存档", "备份", "backup",
    "documents", "桌面", "desktop", "downloads",
];

/// 一个疑似卸载残留目录
#[derive(Clone)]
pub struct OrphanDir {
    /// 展示用路径（保留 %VAR% 前缀，便于用户辨认）
    pub shown: String,
    /// 真实绝对路径
    pub path: PathBuf,
    pub size: u64,
    pub files: usize,
}

fn push_token(out: &mut Vec<String>, raw: &str) {
    let low = raw.trim().to_lowercase();
    if low.len() < 3 {
        return;
    }
    // ASCII 词元（连续字母数字，长度 >= 3）
    for part in low.split(|c: char| !c.is_ascii_alphanumeric()) {
        if part.len() >= 3 {
            out.push(part.to_string());
        }
    }
    // 含中文时整串也算一个关键词（「网易云音乐」这类无法按空格拆）
    if low.chars().any(|c| !c.is_ascii()) {
        out.push(low);
    }
}

/// 目录内是否含 .exe（含 .exe 说明这本身就是**程序目录**，不是残留数据）
///
/// 深度放到 6：很多软件把 exe 放在 `<目录>\<子目录>\runtime\` 这类深层路径里，
/// 深度 3 会漏判，进而把「还在用的软件目录」误报成卸载残留（实测踩到过 duowan / .minecraft）。
fn has_exe(p: &Path, max_depth: usize) -> bool {
    walkdir::WalkDir::new(p)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .map(|x| x.eq_ignore_ascii_case("exe"))
                    .unwrap_or(false)
        })
}

/// 目录探测：一次遍历同时得到 (体积, 文件数, 是否含 exe, 最近写入时间)
///
/// 最近写入时间是判定「软件是否还在用」最有力的证据：
/// 真正卸载掉的软件，其遗留数据不会再被写入；还在用的便携软件则一直在更新配置。
fn dir_probe(p: &Path, max_depth: usize) -> (u64, usize, bool, Option<std::time::SystemTime>) {
    let mut size = 0u64;
    let mut files = 0usize;
    let mut exe = false;
    let mut newest: Option<std::time::SystemTime> = None;
    for e in walkdir::WalkDir::new(p)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !e.file_type().is_file() {
            continue;
        }
        files += 1;
        if !exe
            && e.path()
                .extension()
                .map(|x| x.eq_ignore_ascii_case("exe"))
                .unwrap_or(false)
        {
            exe = true;
        }
        // 占用空间口径：残留项在界面上和垃圾项并列显示，数字必须同一把尺子
        size += super::fsize::stat(e.path())
            .map(|s| if s.links > 1 { 0 } else { s.alloc })
            .unwrap_or_else(|| e.metadata().map(|m| m.len()).unwrap_or(0));
        if let Ok(m) = e.metadata() {
            if let Ok(t) = m.modified() {
                if newest.map(|cur| t > cur).unwrap_or(true) {
                    newest = Some(t);
                }
            }
        }
    }
    (size, files, exe, newest)
}

/// 归一化：去掉空格与符号，只留字母数字和中文字符（`HBuilder X` → `hbuilderx`）
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// 开始菜单快捷方式名（含「所有用户」与「当前用户」，递归 3 层）
///
/// 很多软件是便携版 / 用户级安装，**不会写 Uninstall 注册表**，
/// 但一定会在开始菜单留快捷方式。用它做关键词能拦掉「软件还在用却被当残留」的误报。
fn shortcut_names() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        if let Ok(v) = std::env::var(var) {
            dirs.push(PathBuf::from(v).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        for e in walkdir::WalkDir::new(&d)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !e.file_type().is_file() {
                continue;
            }
            let n = e.file_name().to_string_lossy();
            if let Some(stem) = n.strip_suffix(".lnk").or_else(|| n.strip_suffix(".url")) {
                for part in stem.split(|c: char| c == '-' || c == '_' || c == ' ') {
                    if part.chars().count() >= 3 {
                        out.push(part.to_lowercase());
                    }
                }
                if stem.chars().any(|c| !c.is_ascii()) {
                    out.push(stem.to_lowercase());
                }
            }
        }
    }
    out
}

/// 常见安装根目录下的目录名（便携 / 用户级安装的软件也在这里留痕）
fn installed_dir_names() -> Vec<String> {
    let mut bases: Vec<PathBuf> = Vec::new();
    for var in ["PROGRAMFILES", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(v) = std::env::var(var) {
            let p = PathBuf::from(v);
            bases.push(if var == "LOCALAPPDATA" { p.join("Programs") } else { p });
        }
    }
    let mut out = Vec::new();
    for b in bases {
        if let Ok(rd) = std::fs::read_dir(&b) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    push_token(&mut out, &e.file_name().to_string_lossy());
                }
            }
        }
    }
    out
}

/// 已安装软件关键词集 —— 全局缓存
///
/// 每次清理扫描都要用它做判定，而枚举注册表不便宜；缓存 5 分钟。
static KEYWORD_CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>> =
    std::sync::Mutex::new(None);
const KEYWORD_TTL: std::time::Duration = std::time::Duration::from_secs(300);

fn installed_keywords_cached() -> Vec<String> {
    let mut g = KEYWORD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = g.as_ref() {
        if at.elapsed() < KEYWORD_TTL {
            return v.clone();
        }
    }
    let fresh = installed_keywords();
    *g = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// 目录名是否命中「已安装软件关键词」→ 命中说明软件还在，不算残留
///
/// 双向包含 + 归一化比较：`HBuilder X` 与快捷方式 `HBuilderX` 视为同一个。
fn matches_installed(name_low: &str, keywords: &[String]) -> bool {
    let n = norm(name_low);
    if n.chars().count() < 3 {
        return false;
    }
    keywords.iter().any(|k| {
        let kn = norm(k);
        if kn.chars().count() < 3 {
            return false;
        }
        n.contains(&kn) || kn.contains(&n) || name_low.contains(k.as_str())
    })
}

/// 已安装软件关键词集（两个 Uninstall 根键）
fn installed_keywords() -> Vec<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    const ROOTS: &[&str] = &[
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];
    let mut out: Vec<String> = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for r in ROOTS {
            let full = if hive == HKEY_CURRENT_USER {
                format!("Software\\{}", r.trim_start_matches("SOFTWARE\\"))
            } else {
                r.to_string()
            };
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(full, KEY_READ) else {
                continue;
            };
            for k in root.enum_keys().flatten() {
                let Ok(sub) = root.open_subkey_with_flags(&k, KEY_READ) else {
                    continue;
                };
                for name in ["DisplayName", "Publisher", "InstallLocation"] {
                    let Ok(v) = sub.get_value::<String, _>(name) else {
                        continue;
                    };
                    push_token(&mut out, &v);
                    if name == "InstallLocation" {
                        for seg in v.split(['\\', '/']) {
                            push_token(&mut out, seg);
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    // 追加「开始菜单快捷方式名」与「安装根目录名」：
    // 便携版 / 用户级安装的软件不写 Uninstall 注册表，只有这两处留痕
    out.extend(shortcut_names());
    out.extend(installed_dir_names());
    out.sort();
    out.dedup();
    out
}

/// 统计目录体积与文件数（限深，够用且快）
/// 目录体积与文件数（保留给诊断与测试；清理路径已改走 fsx 的实际计数）
#[allow(dead_code)]
pub fn dir_size_and_files(p: &Path, max_depth: usize) -> (u64, usize) {
    let mut size = 0u64;
    let mut files = 0usize;
    for e in walkdir::WalkDir::new(p)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.file_type().is_file() {
            files += 1;
            size += super::fsize::stat(e.path())
                .map(|s| if s.links > 1 { 0 } else { s.alloc })
                .unwrap_or_else(|| e.metadata().map(|m| m.len()).unwrap_or(0));
        }
    }
    (size, files)
}

/// 删除前的最后一道安全闸：路径必须位于允许的根目录之下、名字不在白名单、
/// 且目录名未命中「已安装软件关键词」。
///
/// 即使前端传来被篡改的 id，也不会把 `C:\Windows` 之类的目录删掉；
/// 软件在扫描与清理之间被重新安装（目录名重新出现在已安装列表）时也会被拦下。
pub fn residual_path_allowed(p: &Path) -> bool {
    if !p.is_absolute() || !p.is_dir() {
        return false;
    }
    const BASES: &[&str] = &["PROGRAMFILES", "ProgramFiles(x86)", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA"];
    let mut under = false;
    for var in BASES {
        let Ok(base) = std::env::var(var) else { continue };
        if base.trim().is_empty() {
            continue;
        }
        let Ok(base) = std::fs::canonicalize(&base) else { continue };
        let Ok(target) = std::fs::canonicalize(p) else { continue };
        // 必须是 base 的直接子目录（不允许嵌套更深的路径被单独删除）
        if target.parent() == Some(base.as_path()) {
            under = true;
            break;
        }
    }
    if !under {
        return false;
    }
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let low = name.to_lowercase();
    if low.len() < 3 {
        return false;
    }
    if RESIDUAL_SKIP_DIRS.iter().any(|s| low == *s) {
        return false;
    }
    if RESIDUAL_SKIP_WORDS.iter().any(|w| low.contains(w)) {
        return false;
    }
    if has_exe(p, 6) {
        return false;
    }
    // 近期仍有写入 → 软件可能还在用，拒绝删除（与扫描端同一判据）
    let (_, _, _, newest) = dir_probe(p, 6);
    if let Some(t) = newest {
        if t.elapsed().map(|d| d.as_secs() < IDLE_SECS).unwrap_or(false) {
            return false;
        }
    }
    // 目录名命中已安装软件 → 拒绝删除
    !matches_installed(&low, &installed_keywords_cached())
}

/// 卸载残留扫描结果缓存：`collect()` 在扫描与清理时各调一次，
/// 而全盘目录统计要递归数体积，不缓存会明显拖慢「开始清理」。
static SCAN_CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<OrphanDir>)>> =
    std::sync::Mutex::new(None);
const SCAN_TTL: std::time::Duration = std::time::Duration::from_secs(120);
const RESIDUAL_MIN_BYTES: u64 = 8 * 1024 * 1024;
const RESIDUAL_MIN_FILES: usize = 12;
/// 目录「静默期」：最近 45 天内还有写入 → 认为软件仍在用，不算残留
///
/// 取值依据（本机实测）：静默期 0 天识别 6 项（含 Godot / HBuilder X 等仍在用的软件）、
/// 7 天 3 项、30 天 2 项、90 天 0 项。45 天既能覆盖「卸载一个多月后的遗留」，
/// 又不会被活跃使用的软件误触发。
const IDLE_SECS: u64 = 45 * 24 * 3600;

/// 带缓存的残留扫描（阈值固定为 8MB / 12 个文件，避免把零碎目录全列出来）
pub fn scan_uninstall_residuals_cached() -> Vec<OrphanDir> {
    let mut g = SCAN_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = g.as_ref() {
        if at.elapsed() < SCAN_TTL {
            return v.clone();
        }
    }
    let fresh = scan_uninstall_residuals_impl(RESIDUAL_MIN_BYTES, RESIDUAL_MIN_FILES, IDLE_SECS);
    *g = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// 扫描「疑似已卸载软件残留」目录
pub fn scan_uninstall_residuals() -> Vec<OrphanDir> {
    scan_uninstall_residuals_cached()
}

/// 扫描「疑似已卸载软件残留」目录
///
/// `min_bytes` / `min_files` 为保守阈值，避免把小零碎目录全列出来
fn scan_uninstall_residuals_impl(min_bytes: u64, min_files: usize, idle_secs: u64) -> Vec<OrphanDir> {
    let keywords = installed_keywords_cached();
    if keywords.is_empty() {
        return Vec::new(); // 读不到已安装列表 → 宁可不报，避免误删
    }
    const BASES: &[(&str, &str)] = &[
        ("%ProgramFiles%", "PROGRAMFILES"),
        ("%ProgramFiles(x86)%", "ProgramFiles(x86)"),
        ("%APPDATA%", "APPDATA"),
        ("%LocalAppData%", "LOCALAPPDATA"),
        ("%ProgramData%", "PROGRAMDATA"),
    ];
    let mut out: Vec<OrphanDir> = Vec::new();
    for (tag, var) in BASES {
        let Ok(base) = std::env::var(var) else { continue };
        if base.trim().is_empty() {
            continue;
        }
        let base = PathBuf::from(base);
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let low = name.to_lowercase();
            if low.len() < 3 {
                continue;
            }
            // 系统 / 共享目录白名单
            if RESIDUAL_SKIP_DIRS.iter().any(|s| low == *s) {
                continue;
            }
            // 存档 / 备份 / 文档类目录一律跳过（删了就是丢用户数据）
            if RESIDUAL_SKIP_WORDS.iter().any(|w| low.contains(w)) {
                continue;
            }
            // 命中已安装关键词 → 软件仍在，不算残留
            if matches_installed(&low, &keywords) {
                continue;
            }
            let (size, files, exe, newest) = dir_probe(&p, 6);
            // 含 .exe → 本身就是程序目录（深度 6，覆盖 `<目录>\runtime\` 这类深层）
            if exe {
                continue;
            }
            if size < min_bytes || files < min_files {
                continue;
            }
            // 近期仍在写入 → 软件很可能还在用（便携版 / 用户级安装不写注册表），不报
            if let Some(t) = newest {
                if t.elapsed().map(|d| d.as_secs() < idle_secs).unwrap_or(false) {
                    continue;
                }
            }
            out.push(OrphanDir {
                shown: format!("{tag}\\{name}"),
                path: p,
                size,
                files,
            });
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out.truncate(60);
    out
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    /// 提取 `%NAME%` 里的 NAME
    fn tokens(raw: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = raw;
        while let Some(i) = rest.find('%') {
            let after = &rest[i + 1..];
            let Some(j) = after.find('%') else { break };
            out.push(&after[..j]);
            rest = &after[j + 1..];
        }
        out
    }

    /// 回归测试：**所有**规则的占位符都必须能被解析。
    ///
    /// 这条锁的是 Steam 那个 bug：规则里写 `%PROGRAMFILES(X86)%`，
    /// 而环境变量叫 `ProgramFiles(x86)`，大小写不匹配 → 路径不展开 → 规则静默失效。
    /// 新增规则时若打错占位符，这里会直接失败，而不是等到用户反馈"扫不出来"。
    #[test]
    fn 规则占位符全部可解析() {
        for rule in RULES {
            for spec in rule.paths {
                for t in tokens(spec.path) {
                    if t.eq_ignore_ascii_case("drive") {
                        assert!(rule.per_drive, "规则 {} 用了 %Drive% 却没标 per_drive", rule.id);
                        continue;
                    }
                    assert!(
                        lookup_token(t, None).is_some(),
                        "规则 {} 的路径 `{}` 用了无法解析的占位符 %{}%",
                        rule.id, spec.path, t
                    );
                }
            }
            for d in rule.detect {
                for t in tokens(d) {
                    assert!(
                        lookup_token(t, None).is_some(),
                        "规则 {} 的 detect `{}` 用了无法解析的占位符 %{}%",
                        rule.id, d, t
                    );
                }
            }
        }
    }

    #[test]
    fn 占位符大小写不敏感() {
        let upper = subst("%PROGRAMFILES(X86)%\\Steam", None);
        let mixed = subst("%ProgramFiles(x86)%\\Steam", None);
        assert!(!upper.starts_with('%'), "大写形式没被替换：{upper}");
        if std::env::var("ProgramFiles(x86)").is_ok() {
            assert_eq!(upper, mixed);
        }
    }

    #[test]
    fn 逐盘占位符只在给了盘符时替换() {
        assert_eq!(subst("%Drive%\\Windows\\Temp", Some('D')), "D:\\Windows\\Temp");
        // 全局规则误用 %Drive% 时应原样保留（路径展开失败 → 跳过），不能猜一个盘
        assert_eq!(subst("%Drive%\\Windows\\Temp", None), "%Drive%\\Windows\\Temp");
    }

    /// 诊断：打印每条规则在 C 盘的真实命中情况（只读，不删任何东西）
    ///
    /// `cargo test --lib 诊断清理规则命中 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn 诊断清理规则命中() {
        let mut total = 0u64;
        for rule in RULES {
            if !detect_ok(rule) {
                println!("{:<24} detect 未通过（未安装）", rule.id);
                continue;
            }
            let drive = if rule.per_drive { Some('C') } else { None };
            let t = scan_rule(rule, drive);
            if t.bytes == 0 && t.hardlinks == 0 {
                continue;
            }
            total += t.bytes;
            println!(
                "{:<24} {:>10.2} MiB  files={:<7} hardlinks={:<5} hardlinkMiB={:>8.2} 无法读属性={}",
                rule.id,
                t.bytes as f64 / 1048576.0,
                t.files,
                t.hardlinks,
                t.hardlink_bytes as f64 / 1048576.0,
                t.unknown
            );
        }
        println!("---- 合计（占用空间口径）{:>10.2} MiB", total as f64 / 1048576.0);
    }

    #[test]
    fn 系统日志不与更新日志重复统计() {
        let r = RULES.iter().find(|r| r.id == "drive.winlogs").expect("drive.winlogs 必须存在");
        assert!(
            r.exclude.iter().any(|x| x.eq_ignore_ascii_case("WindowsUpdate")),
            "drive.winlogs 必须排除 WindowsUpdate，否则与 win.wulogs 双计"
        );
        assert!(
            RULES.iter().any(|r| r.id == "win.wulogs"),
            "win.wulogs 仍然存在，说明排除是配对的"
        );
    }

    #[test]
    fn 微信缓存指向Documents而不是Roaming() {
        let r = RULES.iter().find(|r| r.id == "wechat.cache").unwrap();
        assert!(
            r.paths.iter().all(|s| s.path.starts_with("%USERPROFILE%\\Documents\\xwechat_files")),
            "微信 4.x 的数据目录在 Documents，写 Roaming 会永不命中"
        );
        // 绝不能碰聊天数据
        for s in r.paths {
            let p = s.path.to_lowercase();
            for bad in ["db_storage", "msg\\file", "msg\\attach", "msg\\video", "filestorage"] {
                assert!(!p.contains(bad), "微信规则不得包含用户数据目录：{}", s.path);
            }
        }
    }

    #[test]
    fn 显卡相关缓存默认不勾() {
        // 用户明确要求：GPU 缓存类必须 opt-in
        let ids = ["browser.shadercache", "d3dshader"];
        for id in ids {
            let r = RULES.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("{id} 不存在"));
            assert!(!r.default_on, "{id} 是显卡相关缓存，必须默认不勾");
        }
    }
}

#[cfg(test)]
mod protect_tests {
    use super::*;

    #[test]
    fn 配置文件与设置目录被保护() {
        // UWP 设置区
        assert!(protected_file(Path::new(r"Roaming\settings.json")));
        assert!(protected_file(Path::new(r"roaming\anything")));
        // 数据库 / 配置类扩展名
        for f in ["cache.dbf", "config.json", "app.ini", "state.db", "prefs.xml", "x.dat"] {
            assert!(protected_file(Path::new(f)), "{f} 应被保护");
        }
        // Chromium profile 关键文件（无扩展名）
        for f in ["Login Data", "Preferences", "History", "Cookies", "Local State"] {
            assert!(protected_file(Path::new(f)), "{f} 应被保护");
        }
    }

    #[test]
    fn 普通缓存文件不被保护() {
        for f in ["data_0", "f_000001", "index", "Cache_Data\\f_000002", "blob.bin"] {
            assert!(!protected_file(Path::new(f)), "{f} 不应被保护");
        }
    }
}

#[cfg(test)]
mod residual_tests {
    use super::*;

    #[test]
    fn 关键词拆分正确() {
        let mut v = Vec::new();
        push_token(&mut v, "网易云音乐");
        push_token(&mut v, "CloudMusic 2.0");
        push_token(&mut v, "ab");
        assert!(v.iter().any(|s| s == "cloudmusic"), "应拆出 ASCII 词元");
        assert!(v.iter().any(|s| s == "网易云音乐"), "中文整串应保留");
        assert!(!v.iter().any(|s| s == "ab"), "过短词元应丢弃");
    }

    #[test]
    fn 系统白名单目录不会被判为残留() {
        for d in ["Common Files", "WindowsApps", "Package Cache", "Microsoft"] {
            assert!(
                RESIDUAL_SKIP_DIRS.iter().any(|s| d.eq_ignore_ascii_case(*s)),
                "{d} 必须在白名单内"
            );
        }
    }
}
