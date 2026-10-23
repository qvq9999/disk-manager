//! 应用缓存自动发现（深度扫描）
//!
//! 为什么不用「一个应用一条规则」：Electron / Chromium 系应用的缓存路径高度同构
//! （`User Data\Default\Cache`、`EBWebView\Default\Code Cache` …），而应用数量远超
//! 手工维护能力 —— 本机 `%APPDATA%` 下就有 81 个应用目录、`%LOCALAPPDATA%` 下 29 个。
//! 实测按应用枚举的 68 条规则只覆盖到实际缓存体积的约 1/4（豆包单目录 1.44GB 完全漏掉）。
//! 结构性问题要用结构解：**遍历应用目录 + 精确白名单目录名**。
//!
//! 安全模型（任一不通过即跳过，宁可漏报绝不误删）：
//!   1. 只认**精确匹配**的白名单目录名（`Cache` / `Code Cache` / `GPUCache` …），
//!      绝不做「名字里含 cache」这类模糊匹配
//!   2. 路径任意一段命中黑名单（聊天记录、游戏存档、文档、程序安装目录）直接跳过
//!   3. 只允许位于 `%LOCALAPPDATA%` / `%APPDATA%` 之下
//!   4. 删除时**只删目录内文件、保留目录本身** —— 应用会自行重建，不会因目录缺失报错
//!   5. 删除前重新执行 1–3 全部校验（id 里编码绝对路径，必须防篡改）

use std::path::{Path, PathBuf};

use super::clean_rules;
use super::fsize::{SkipStats, Tally};

/// 精确白名单：目录名（小写比较）必须完全等于其中之一才认定为缓存
///
/// ⚠️ 收录标准已经收紧过一次：**目录里只能有缓存，不能同时是应用的 profile**。
///    `CefCache` 曾因「名字带 cache」被收录，实测（本机 NVIDIA App）它其实是一个完整的
///    Chromium profile —— 内部有 `Login Data`(已存密码) / `Preferences`(全部设置) /
///    `History` / `Network\Cookies`，整目录清空等于把该应用的登录态和设置全删掉。
///    正确做法是不把它当缓存根，**继续下钻**去找里面的 `Cache` / `Code Cache` / `GPUCache`。
const CACHE_NAMES: &[&str] = &[
    // Chromium / Electron 通用
    "cache",
    "code cache",
    "gpucache",
    "grshadercache",
    "shadercache",
    "media cache",
    "cachestorage",
    "component_crx_cache",
    "dawncache",
    "dawngraphitecache",
    "dawnwebgpucache",
    "crashpad",
    // ⚠️ 刻意不含 blob_storage：它存放 IndexedDB 的二进制对象（可能是离线文档、
    //    上传中的文件），删掉会丢应用数据，不属于可再生的缓存
    // ⚠️ 刻意不含 cefcache：见上，它是 CEF 的完整 profile 根目录
    // VS Code / Electron 派生
    "cacheddata",
    "cachedextensionvsixs",
    // 图形驱动着色器缓存
    "dxcache",
    "glcache",
    "d3dscache",
    // UWP 容器（删除时另有配置文件保护闸）
    "inetcache",
    "localcache",
    "tempstate",
];

/// 黑名单路径段（小写包含匹配）。命中即整枝跳过 —— 这些目录下即便有叫 Cache 的子目录，
/// 也属于用户数据或程序本体，删了可能丢聊天记录/存档，或破坏安装目录
const BLOCK_SEGS: &[&str] = &[
    // 聊天记录（微信/QQ/TIM 的文件与图片本体，不是缓存）
    "wechat files",
    "xwechat_files",
    "qqfile",
    "qq files",
    "tencent files",
    // 游戏存档
    "saves",
    "savegames",
    "save data",
    "savegames",
    "存档",
    // 用户文档
    "documents",
    "desktop",
    "downloads",
    // 程序安装目录（%LOCALAPPDATA%\Programs 是用户级安装位置）
    "programs",
    // 代码仓库
    ".git",
    ".svn",
    "node_modules",
    // 系统保护
    "system volume information",
    "windowsapps",
    "windows\\system32",
    // 认证令牌缓存：删了会触发 Microsoft 账号重新登录，体积却只有几百 KB
    // —— 收益与代价不成比例，直接跳过（用户投诉过「清理后要重新登录」）
    "tokenbroker",
];

pub struct FoundCache {
    /// 应用名（`%APPDATA%` 下的一级目录名；UWP 取包名）
    pub app: String,
    /// 该应用下发现的缓存目录（多个：Cache / Code Cache / GPUCache…）
    pub dirs: Vec<PathBuf>,
    pub size: u64,
    /// 硬链接文件数 / 占用（不计入 size：删了不释放空间）
    pub hardlinks: u64,
    pub hardlink_bytes: u64,
    /// 是否与显卡相关（着色器预编译 / 驱动预加载）
    ///
    /// 这类目录删了不会丢设置，但会让游戏与创作软件**首次启动重新编译着色器**，
    /// 表现为明显卡顿和读盘。用户明确要求默认不勾，只作为可选项列出。
    pub gpu: bool,
}

/// 显卡厂商与图形管线缓存关键词（按「路径段」匹配，不做整串包含）
const GPU_SEGS: &[&str] = &[
    "nvidia", "nvidia corporation", "amd", "ati", "radeon", "geforce",
    "intel", "dxcache", "glcache", "d3dscache", "vulkan",
];

/// 单个路径段是否属于显卡相关
fn seg_is_gpu(seg: &str) -> bool {
    let s = seg.trim().to_lowercase();
    GPU_SEGS.iter().any(|k| {
        s == *k
            || s.starts_with(&format!("{k} "))
            || s.starts_with(&format!("{k}_"))
            || s.starts_with(&format!("{k}-"))
    })
}

/// 路径（任意一段）或应用名是否与显卡相关
pub fn gpu_related(app: &str, dirs: &[PathBuf]) -> bool {
    if seg_is_gpu(app) {
        return true;
    }
    dirs.iter().any(|d| {
        d.components()
            .any(|c| seg_is_gpu(&c.as_os_str().to_string_lossy()))
    })
}

fn low(s: &str) -> String {
    s.to_lowercase()
}

fn is_cache_name(name: &str) -> bool {
    let n = low(name);
    CACHE_NAMES.iter().any(|c| *c == n)
}

fn blocked_segment(name: &str) -> bool {
    let n = low(name);
    BLOCK_SEGS.iter().any(|b| n.contains(b))
}

/// 路径任意一段命中黑名单
fn blocked_path(p: &Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        blocked_segment(&s)
    })
}

/// 允许的根目录：用户级应用数据所在位置
fn allowed_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for k in ["LOCALAPPDATA", "APPDATA"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                out.push(PathBuf::from(v));
            }
        }
    }
    out
}

fn dir_size(p: &Path) -> Tally {
    let mut t = Tally::default();
    for e in walkdir::WalkDir::new(p).follow_links(false).into_iter() {
        // 访问被拒的目录要计数，不能静默丢弃（否则"扫出来很小"其实是"没权限看"）
        let e = match e {
            Ok(e) => e,
            Err(_) => {
                t.denied_dirs += 1;
                continue;
            }
        };
        if e.file_type().is_file() {
            // 与 delete_contents 同口径：被保护闸拦下的文件不计入「可清理」
            let rel = e.path().strip_prefix(p).unwrap_or(e.path());
            if clean_rules::protected_file(rel) {
                continue;
            }
            // 与删除端同一个统计函数：AllocationSize + 硬链接不计
            t.add_stat(e.path());
        }
    }
    t
}

/// 在应用目录下递归找缓存目录；命中即收集且不再下钻（缓存内部不再拆解）
fn find_caches(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let path = e.path();
        if blocked_segment(&name) {
            continue;
        }
        if is_cache_name(&name) {
            out.push(path);
            continue; // 不再下钻
        }
        find_caches(&path, depth + 1, max_depth, out);
    }
}

/// 扫描所有应用目录，聚合出「每个应用一堆缓存目录」。
///
/// `skip` 是已被静态规则覆盖的路径集合 —— 避免同一批文件被统计两次
/// （那会让「可清理总量」虚高，清理时还会因二次删除失败把 failed 数撑起来）。
pub fn discover(skip: &[PathBuf], max_depth: usize) -> Vec<FoundCache> {
    let mut out: Vec<FoundCache> = Vec::new();
    for root in allowed_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let app_name = e.file_name().to_string_lossy().to_string();
            if blocked_segment(&app_name) {
                continue;
            }
            let app_dir = e.path();

            if app_name.eq_ignore_ascii_case("Packages") {
                // UWP：每个包是一个独立应用
                let Ok(pkgs) = std::fs::read_dir(&app_dir) else { continue };
                for pk in pkgs.flatten() {
                    let Ok(pft) = pk.file_type() else { continue };
                    if !pft.is_dir() {
                        continue;
                    }
                    let pkg_name = pk.file_name().to_string_lossy().to_string();
                    let mut dirs = Vec::new();
                    find_caches(&pk.path(), 0, max_depth, &mut dirs);
                    push_found(&mut out, short_pkg(&pkg_name), dirs, skip);
                }
                continue;
            }

            let mut dirs = Vec::new();
            find_caches(&app_dir, 0, max_depth, &mut dirs);
            push_found(&mut out, app_name, dirs, skip);
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

/// UWP 包名太长（`MicrosoftWindowsStore_8wekyb3d8bbwe`），取可读的一段
fn short_pkg(name: &str) -> String {
    name.split('_').next().unwrap_or(name).to_string()
}

fn push_found(out: &mut Vec<FoundCache>, app: String, dirs: Vec<PathBuf>, skip: &[PathBuf]) {
    let kept: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|d| !skip.iter().any(|s| d == s || d.starts_with(s) || s.starts_with(d)))
        .collect();
    if kept.is_empty() {
        return;
    }
    let mut t = Tally::default();
    for d in &kept {
        t.merge(dir_size(d));
    }
    // 太小的不算（用户看不出价值，却会撑爆列表）
    if t.bytes < 8 * 1024 * 1024 {
        return;
    }
    let gpu = gpu_related(&app, &kept);
    out.push(FoundCache {
        app,
        size: t.bytes,
        hardlinks: t.hardlinks,
        hardlink_bytes: t.hardlink_bytes,
        dirs: kept,
        gpu,
    });
}

// ---------------- 删除端：每次都要重新过一遍安全闸 ----------------

/// 校验一个缓存目录是否允许删除（id 里带绝对路径，必须防篡改）
pub fn path_allowed(p: &Path) -> bool {
    if !p.is_absolute() || !p.is_dir() {
        return false;
    }
    // 目录名必须精确命中白名单
    let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return false;
    };
    if !is_cache_name(&name) {
        return false;
    }
    if blocked_path(p) {
        return false;
    }
    // 必须位于用户级应用数据目录之下
    let roots = allowed_roots();
    if !roots.iter().any(|r| p.starts_with(r)) {
        return false;
    }
    // 层级不能过浅（防止意外指向 %LOCALAPPDATA% 本身或其一级应用目录）
    let depth = p.components().count();
    if depth < 5 {
        return false;
    }
    true
}

/// 只删目录内文件，保留目录本身与子目录结构。
/// 应用会自行重建缓存文件；保留目录可避免个别应用因目录缺失报错。
///
/// ⚠️ 删除前逐文件过「配置文件保护闸」（`clean_rules::protected_file`）：
///    `LocalCache` 这类目录里混着 UWP 设置（`Roaming`）与数据库（`*.dbf`），
///    一律清空会让用户「软件里的个人设置全没了」。宁可少清一点缓存。
pub fn delete_contents<F: FnMut(u64)>(
    dir: &Path,
    freed: &mut u64,
    skip: &mut SkipStats,
    mut on_file: F,
) {
    if !path_allowed(dir) {
        // 安全闸拒绝 ≠ 权限不足：单独归类，别混进「被占用」里误导用户
        skip.reject(0);
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for e in walkdir::WalkDir::new(dir).follow_links(false).into_iter() {
        let e = match e {
            Ok(e) => e,
            Err(_) => {
                skip.denied_dirs += 1;
                continue;
            }
        };
        if e.file_type().is_file() {
            let rel = e.path().strip_prefix(dir).unwrap_or(e.path());
            if clean_rules::protected_file(rel) {
                continue; // 设置 / 数据库 / profile 文件：保留
            }
            files.push(e.path().to_path_buf());
        }
    }
    for (i, f) in files.into_iter().enumerate() {
        if i % 256 == 0 && super::clean::cancel_requested() {
            return;
        }
        // 统一口径：删除前取 AllocationSize，硬链接不计入释放量
        let before = *freed;
        super::fsx::remove_file_tally(&f, freed, skip);
        if *freed != before {
            on_file(*freed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 显卡相关缓存必须被识别出来（默认不勾），普通应用缓存不能被误判
    #[test]
    fn 显卡相关缓存被识别() {
        let p = |s: &str| vec![PathBuf::from(s)];
        assert!(gpu_related("NVIDIA", &p(r"C:\Users\x\AppData\Local\NVIDIA\DXCache")));
        assert!(gpu_related("NVIDIA Corporation", &p(r"C:\x\NVIDIA App\CefCache\Default\GPUCache")));
        assert!(gpu_related("AMD", &p(r"C:\x\AppData\Local\AMD\DxCache")));
        assert!(gpu_related("Foo", &p(r"C:\x\AppData\Local\Foofoo\D3DSCache")));
        assert!(gpu_related("Intel", &p(r"C:\x\AppData\Local\Intel\ShaderCache")));

        // 普通应用不得误判
        assert!(!gpu_related("Microsoft", &p(r"C:\x\AppData\Local\Microsoft\Edge\User Data\Default\Cache")));
        assert!(!gpu_related("Steam", &p(r"C:\x\AppData\Local\Steam\htmlcache\Default\Cache")));
        assert!(!gpu_related("Quwan", &p(r"C:\x\AppData\Roaming\Quwan\TTVoice\EBWebView\Default\Cache")));
        // 名字里含 amd 但不是独立路径段的（如 "Pyramid"）不该命中
        assert!(!gpu_related("Pyramid", &p(r"C:\x\AppData\Local\Pyramid\Cache")));
    }

    #[test]
    fn 白名单精确匹配不做模糊() {
        assert!(is_cache_name("Cache"));
        assert!(is_cache_name("code cache"));
        assert!(is_cache_name("GPUCache"));
        // 下列都不该命中（它们要么是用户数据，要么是程序本体）
        assert!(!is_cache_name("CacheDir"));
        assert!(!is_cache_name("MyCache"));
        assert!(!is_cache_name("Cache2"));
        assert!(!is_cache_name("WeChat Files"));
    }

    /// 回归：`CefCache` 是 CEF 的完整 profile（含 Login Data / Preferences / History），
    /// 绝不能当成缓存根整目录清空；应继续下钻到它内部的真实缓存子目录。
    #[test]
    fn cef目录不下钻为缓存根() {
        assert!(!is_cache_name("CefCache"), "CefCache 不得作为缓存根");
        let Ok(local) = std::env::var("LOCALAPPDATA") else {
            eprintln!("skip: 无 LOCALAPPDATA");
            return;
        };
        let root = PathBuf::from(&local).join("__dm_cef_test__");
        let _ = std::fs::remove_dir_all(&root);
        let cef = root.join("App").join("CefCache");
        let def = cef.join("Default");
        std::fs::create_dir_all(def.join("Cache")).unwrap();
        std::fs::create_dir_all(def.join("Code Cache")).unwrap();
        std::fs::create_dir_all(def.join("Network")).unwrap();
        std::fs::write(def.join("Login Data"), b"pw").unwrap();
        std::fs::write(def.join("Preferences"), b"{}").unwrap();
        std::fs::write(def.join("History"), b"h").unwrap();
        std::fs::write(def.join("Network").join("Cookies"), b"c").unwrap();
        std::fs::write(def.join("Cache").join("data_0"), b"cache").unwrap();

        let mut dirs = Vec::new();
        find_caches(&root, 0, 5, &mut dirs);
        let names: Vec<String> = dirs
            .iter()
            .map(|d| d.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "Cache"),
            "应找到 CefCache\\Default\\Cache，实际：{names:?}"
        );
        assert!(names.iter().any(|n| n == "Code Cache"), "应找到 Code Cache");
        assert!(!names.iter().any(|n| n == "CefCache"), "CefCache 本身不得入选");

        // 即便误选了 profile 根，删除端也必须保留登录/设置文件
        for f in ["Login Data", "Preferences", "History", "Network/Cookies"] {
            assert!(
                clean_rules::protected_file(Path::new(f)),
                "{f} 必须被保护闸拦下"
            );
        }
        assert!(!clean_rules::protected_file(Path::new("data_0")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 回归：UWP `LocalCache` 里的 Roaming（设置）与 *.dbf（数据库）必须保留
    #[test]
    fn localcache保留设置与数据库() {
        let Ok(local) = std::env::var("LOCALAPPDATA") else {
            eprintln!("skip: 无 LOCALAPPDATA");
            return;
        };
        let root = PathBuf::from(&local).join("__dm_lc_test__");
        let _ = std::fs::remove_dir_all(&root);
        let lc = root.join("Packages").join("Foo_8we").join("LocalCache");
        std::fs::create_dir_all(lc.join("Roaming")).unwrap();
        std::fs::write(lc.join("blob.bin"), vec![0u8; 1024]).unwrap();
        std::fs::write(lc.join("cache.dbf"), b"db").unwrap();
        std::fs::write(lc.join("Roaming").join("settings.dat"), b"s").unwrap();
        std::fs::write(lc.join("Roaming").join("user.json"), b"{}").unwrap();

        assert!(path_allowed(&lc), "该测试路径应被允许：{}", lc.display());
        let mut freed = 0u64;
        let mut skip = SkipStats::default();
        delete_contents(&lc, &mut freed, &mut skip, |_| {});

        // 口径是**占用空间**（簇对齐）：1024 字节的文件在 4KB 簇上算 4KB，
        // 所以这里不能断言等于文件长度，只断言"统计到了且没算被保护的文件"
        assert!(freed >= 1024, "应统计到被删文件的占用：{freed}");
        assert_eq!(skip.count(), 0, "不该有跳过项");
        assert!(lc.join("cache.dbf").is_file(), "数据库必须保留");
        assert!(lc.join("Roaming").join("settings.dat").is_file(), "Roaming 必须保留");
        assert!(lc.join("Roaming").join("user.json").is_file());
        assert!(!lc.join("blob.bin").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 黑名单挡住聊天记录与存档() {
        assert!(blocked_segment("WeChat Files"));
        assert!(blocked_segment("xwechat_files"));
        assert!(blocked_segment("QQfile"));
        assert!(blocked_segment("Saves"));
        assert!(blocked_segment("Documents"));
        assert!(blocked_segment("Programs"));
        assert!(!blocked_segment("Cache"));
        assert!(!blocked_segment("Doubao"));
    }

    #[test]
    fn 指向应用目录本身或过浅路径被拒绝() {
        // 即使名字叫 Cache，只要不是白名单目录名就拒绝
        assert!(!path_allowed(Path::new(r"C:\Windows")));
        assert!(!path_allowed(Path::new(r"C:\Windows\Cache"))); // 不在用户目录下
    }

    #[test]
    fn 危险路径全部被拒绝() {
        // ① 白名单名 + 黑名单段（聊天记录）：必须拒绝
        assert!(!path_allowed(Path::new(
            r"C:\Users\x\AppData\Roaming\Tencent\WeChat Files\Cache"
        )));
        // ② 非白名单目录名：拒绝
        assert!(!path_allowed(Path::new(
            r"C:\Users\x\AppData\Local\SomeApp\User Data\Default"
        )));
        // ③ 用户级安装目录（Programs）：拒绝，哪怕里面真有叫 Cache 的子目录
        assert!(!path_allowed(Path::new(
            r"C:\Users\x\AppData\Local\Programs\SomeApp\Cache"
        )));
    }

    /// 实机验证删除行为：只删目录内文件、保留目录本身，且能处理只读文件
    #[test]
    fn 只删目录内文件并保留目录() {
        let Ok(local) = std::env::var("LOCALAPPDATA") else {
            eprintln!("skip: 无 LOCALAPPDATA");
            return;
        };
        // 构造一个层级足够深的合法缓存路径
        let root = PathBuf::from(&local).join("__dm_cache_test__");
        let cache = root
            .join("App")
            .join("User Data")
            .join("Default")
            .join("Cache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(cache.join("sub")).unwrap();
        std::fs::write(cache.join("a.bin"), vec![0u8; 1024]).unwrap();
        std::fs::write(cache.join("sub/b.bin"), vec![0u8; 2048]).unwrap();
        // 只读文件：真实缓存里很常见
        let ro = cache.join("readonly.bin");
        std::fs::write(&ro, vec![0u8; 512]).unwrap();
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&ro, perm).unwrap();

        assert!(path_allowed(&cache), "合法缓存路径应被允许");

        let mut freed = 0u64;
        let mut skip = SkipStats::default();
        delete_contents(&cache, &mut freed, &mut skip, |_| {});

        // 占用空间口径（簇对齐），断言"三个文件都被算进去了"即可
        assert!(freed >= 1024 + 2048 + 512, "应统计并删除全部文件：{freed}");
        assert_eq!(skip.count(), 0, "只读文件也应成功删除");
        assert!(cache.is_dir(), "缓存目录本身必须保留");
        assert_eq!(
            std::fs::read_dir(&cache).unwrap().count(),
            1,
            "只剩 sub 空目录（目录结构保留，文件已清空）"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
