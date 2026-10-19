//! 应用知识库：内置 FAQ + README/CHANGELOG 切片，按问题做关键词检索。
//!
//! 为什么需要它：模型没见过这个小众软件，问「1.3.6 改了什么」「为什么标准权限扫得少」
//! 只能靠猜。这里把**产品自己的文档**切成小段，命中才塞进提示词，没命中就不占 token。
//!
//! 为什么不上向量库：全部语料不到 10KB、几十段，中文 bigram 覆盖率检索已经够用；
//! 上 embedding 要么加依赖（拉不动），要么每次请求多一次网络往返（慢且费钱）。

use std::sync::OnceLock;

/// 一段可检索资料
pub struct Chunk {
    pub title: String,
    pub body: String,
    /// 是否为人工整理的 FAQ。整理过的答案优先于文档原文 —— 更新日志里
    /// 可能恰好写着"AI 能答作者是谁"，但那不是答案本身。
    pub faq: bool,
}

const README: &str = include_str!("../../../README.md");
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// 内置 FAQ：这些是用户最常问、且模型最容易答错的问题。
/// 内容必须与实现保持一致 —— 改功能时同步改这里。
fn faq() -> Vec<Chunk> {
    use crate::app_meta as m;
    // 作者与版本两条要走 app_meta 常量（单一身份源），其余是静态文案
    let mut out = vec![
        Chunk {
            faq: true,
            title: "作者 开发者 谁做的 开源 仓库 GitHub star".into(),
            body: format!(
                "本软件由 {} 开发并维护，以 {} 协议开源，仓库地址 {}。\
                 如果觉得好用，可以去仓库点一个 Star 支持一下。",
                m::AUTHOR, m::LICENSE, m::REPO_LABEL
            ),
        },
        Chunk {
            faq: true,
            title: "版本号 当前版本 最新版本 更新日志".into(),
            body: format!(
                "当前版本 {}。完整更新记录在设置页的「更新日志」里，也可以看仓库的 releases 页面：{}/releases。",
                m::VERSION, m::REPO_LABEL
            ),
        },
    ];
    let items: [(&str, &str); 12] = [
        (
            "标准权限 提权 管理员 扫描结果偏小 权限不足",
            "界面右下角显示「标准权限」时，系统目录（如 C:\\Windows、System Volume Information）\
             会因为 ACL 限制读不到，扫描结果会偏小。点侧边栏的「提权」以管理员身份重启，结果才完整。\
             提权不影响清理的安全规则，只是让统计更准。",
        ),
        (
            "清理 能恢复吗 回收站 误删",
            "不能恢复。清理是直接删除、不进回收站，确认框里也写明了这一点。\
             所以默认只勾选低风险项（临时文件、缓存、日志），有风险的项目需要你自己勾。",
        ),
        (
            "残留 删不掉 被占用 权限不足 重启后删除",
            "残留清理会逐项给出原因：被其他进程占用、权限被拒绝、或者需要重启后生效。\
             显示「需重启后生效」的项，文件已登记为重启时删除，重启后空间才会释放；\
             被占用的项建议先退出对应软件再清一次。",
        ),
        (
            "扫描 慢 多久 空间分析 原理 MFT",
            "空间分析在 NTFS 分区上用 $MFT 元文件枚举，整盘通常 1~2 秒；其他文件系统或权限不足时\
             回退为目录遍历，会慢一些但结果一致。扫描可以随时停止，中途停止不会留下半成品数据。",
        ),
        (
            "显卡缓存 GPU 默认不勾 DXCache 驱动缓存",
            "NVIDIA / AMD / Intel 的着色器缓存与驱动缓存默认**不勾选**。它们删了会自动重建，\
             但可能导致游戏首次启动卡顿或着色器重新编译，需要你自己决定要不要清。",
        ),
        (
            "安装包 空文件夹 默认不勾",
            "「安装包清理」和「空文件夹」两类默认都不勾：安装包可能是你的装机备份，\
             空文件夹可能是某些软件要求的占位目录。清理安装包只删文件，不会卸载已安装的软件。",
        ),
        (
            "卸载残留 原理 检测不全 漏检",
            "卸载残留靠两条线索：① 卸载**之前**先把注册表里的安装目录、发布者、卸载命令快照下来；\
             ② 目录与注册表都按「厂商\\产品」两层枚举。只有名称可参考时（比如商店应用）检出会少一些。",
        ),
        (
            "自动更新 更新失败 检查更新",
            "设置页可以手动检查更新，启动时也会自动检查一次。更新包带签名校验，\
             下载完成后重启安装即可。若检查失败，多半是网络问题，可以稍后再试。",
        ),
        (
            "隐私 上传 数据 联网 安全",
            "磁盘扫描、清理、卸载全部在本机完成，**不会上传文件内容或路径**。\
             只有你主动使用 AI 助手时，才会把当页的摘要（如 CPU 型号、剩余空间）发给模型服务；\
             具体发了什么，看设置里「允许 AI 读取当前页面」这个开关。",
        ),
        (
            "支持系统 Windows 10 11 要求",
            "支持 64 位 Windows 10 / 11。空间分析的高速路径需要 NTFS，其他文件系统会自动回退。",
        ),
        (
            "图标 不显示 空白 黑边",
            "应用图标取自文件自带资源；个别程序把图标放在专用 DLL 里或本身没有图标，\
             会显示为通用图标。这是资源缺失，不是读取失败。",
        ),
        (
            "开机自启 关闭按钮 托盘 后台",
            "设置页可以开启开机自启（写入当前用户的启动项，不需要管理员权限），\
             也可以选择点关闭按钮时询问、退到后台还是直接退出。",
        ),
    ];
    out.extend(items.into_iter().map(|(t, b)| Chunk { faq: true, title: t.into(), body: b.into() }));
    out
}

fn build() -> Vec<Chunk> {
    let mut v = faq();
    v.extend(split_md("更新日志", CHANGELOG));
    v.extend(split_md("说明", README));
    v
}

/// 按二级标题切分 markdown，每段截断到 700 字（够回答，又不至于一次塞太多）
fn split_md(prefix: &str, md: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut title = String::new();
    let mut body = String::new();
    let flush = |title: &mut String, body: &mut String, out: &mut Vec<Chunk>| {
        if title.is_empty() || body.trim().is_empty() {
            title.clear();
            body.clear();
            return;
        }
        let text: String = body.trim().chars().take(700).collect();
        out.push(Chunk { faq: false, title: format!("{prefix}：{}", title.trim()), body: text });
        title.clear();
        body.clear();
    };
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            flush(&mut title, &mut body, &mut out);
            title = h.to_string();
        } else if !title.is_empty() {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&mut title, &mut body, &mut out);
    out
}

fn chunks() -> &'static Vec<Chunk> {
    static C: OnceLock<Vec<Chunk>> = OnceLock::new();
    C.get_or_init(build)
}

/// 归一化：只留字母数字与汉字，其余变空格（标点不该影响匹配）
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(' ');
        }
    }
    out
}

/// 中文按二元组切、英文/数字按词切。
///
/// 为什么中文用 bigram：没有分词词典，"扫描结果偏小"必须能命中只写了"扫描"的段落；
/// bigram 覆盖率的粒度恰好够用，成本只有一次字符串遍历。
fn grams(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = norm(s).chars().collect();
    for w in chars.windows(2) {
        if w[0] == ' ' || w[1] == ' ' {
            continue;
        }
        out.push(format!("{}{}", w[0], w[1]));
    }
    out
}

/// 提取形如 `1.3.6` / `v1.3.6` 的版本号，去掉 `v` 与点，便于精确比对。
///
/// 单独处理的原因：归一化会把 `.` 变成空格，`1.3.6` 被切成 `1 3 6`，
/// 用 bigram 根本匹配不上 —— 而"这版改了什么"恰恰是最常见的问法。
fn versions(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.')) {
        let t = raw.trim_start_matches(['v', 'V']);
        if t.len() >= 3 && t.contains('.') && t.chars().any(|c| c.is_ascii_digit()) {
            out.push(t.replace('.', "").to_ascii_lowercase());
        }
    }
    out
}

/// 查询 gram 在文本里命中了几个（去重，避免同一 gram 反复计数）
fn count_hits(q: &[String], text: &str) -> usize {
    let hay = norm(text);
    let mut seen = std::collections::HashSet::new();
    let mut hit = 0;
    for g in q {
        if seen.insert(g.as_str()) && hay.contains(g.as_str()) {
            hit += 1;
        }
    }
    hit
}

/// 检索：返回相关度最高的若干段。
///
/// 打分与阈值都是按中文短问句实测出来的：
/// · 分母取 `min(gram 数, 8)` —— 长问句不该因为字数多而被稀释（否则"为什么扫描出来的结果比实际小"
///   命中 2 个词也只有 0.15，永远进不了候选）；
/// · 阈值 0.18 —— 无关问题（"今天天气怎么样"）通常 0 命中，被挡住；命中 2 个词的相关问题能过。
/// · 版本号是硬匹配，命中直接置顶：用户问"1.3.6 改了什么"，只想要那一版。
pub fn retrieve(query: &str, limit: usize) -> Vec<&'static Chunk> {
    let q = grams(query);
    let qv = versions(query);
    if q.is_empty() && qv.is_empty() {
        return Vec::new();
    }
    let denom = q.len().clamp(1, 8) as f32;

    let mut scored: Vec<(f32, &'static Chunk)> = chunks()
        .iter()
        .map(|c| {
            // 标题命中权重 ×2：问"谁做的"时，标题是「作者 开发者 谁做的…」的 FAQ
            // 必须压过正文里恰好出现"是谁做的"的更新日志（后者不含答案）。
            let th = count_hits(&q, &c.title);
            let bh = count_hits(&q, &c.body);
            let raw = (th * 2 + bh) as f32 / denom;
            // FAQ 抬一档：整理过的答案（作者、权限、残留…）优先于文档原文。
            // 更新日志里有"AI 助手知道这个软件是谁做的"这种句子，覆盖率能到满分，
            // 但它不含答案；不加这一档，问"谁做的"会被它抢走。
            //
            // ⚠️ 这一档只在**至少命中两个词**时给：否则任何 FAQ 的底分都是 0.35，
            // 无关问题（"今天天气怎么样"）也会被塞进几段资料，白花 token。
            let boost = if c.faq && th + bh >= 2 { 0.35 } else { 0.0 };
            let mut score = raw * 0.6 + boost;
            if !qv.is_empty() {
                let text = format!("{} {}", c.title, c.body);
                if versions(&text).iter().any(|v| qv.contains(v)) {
                    score = score.max(0.95);
                }
            }
            (score, c)
        })
        .filter(|(s, _)| *s >= 0.18)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// 把检索结果拼成提示词片段，总长度受 `max_chars` 限制。
pub fn context_for(query: &str, limit: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for c in retrieve(query, limit) {
        let piece = format!("【{}】{}\n", c.title, c.body);
        if out.chars().count() + piece.chars().count() > max_chars {
            break;
        }
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_meta as m;

    #[test]
    fn 问作者能命中作者那一条() {
        let hits = retrieve("这个软件是谁做的", 3);
        assert!(!hits.is_empty(), "作者类问题必须能检索到内容");
        let top = &hits[0];
        assert!(top.body.contains(m::AUTHOR), "应回答 GitHub 账号，实际：{}", top.body);
        assert!(top.body.contains(m::REPO_LABEL), "应带仓库地址");
    }

    #[test]
    fn 问版本能命中更新日志() {
        let hits = retrieve("1.3.6 改了什么", 3);
        assert!(!hits.is_empty());
        assert!(
            hits[0].title.contains("1.3.6") || hits[0].body.contains("1.3.6"),
            "应命中对应版本的更新日志，实际标题：{}",
            hits[0].title
        );
    }

    #[test]
    fn 问权限能命中提权说明() {
        let hits = retrieve("为什么扫描出来的结果比实际小", 3);
        assert!(!hits.is_empty());
        assert!(
            hits.iter().any(|c| c.body.contains("权限") || c.body.contains("提权")),
            "应命中权限相关说明"
        );
    }

    #[test]
    fn 无关问题不硬凑资料() {
        assert!(retrieve("今天天气怎么样", 3).is_empty(), "无关问题不应塞资料，白花 token");
        assert!(retrieve("", 3).is_empty());
    }

    #[test]
    fn 资料总长度受控() {
        let ctx = context_for("清理 残留 扫描 权限 作者 版本", 5, 600);
        assert!(ctx.chars().count() <= 600, "注入内容不得超过预算");
    }

    #[test]
    fn 知识库包含作者与仓库常量() {
        let all: String = chunks().iter().map(|c| format!("{}{}", c.title, c.body)).collect();
        assert!(all.contains(m::AUTHOR));
        assert!(all.contains(m::REPO_LABEL));
        assert!(all.contains(m::VERSION));
    }
}
