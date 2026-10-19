//! 候选词展示文本与序号一体化排版格式化模块
//!
//! # 设计原理
//! - **实现初衷**：在双组件模式下，GNOME 等桌面环境的盒模型几何居中会导致西文数字与 CJK 汉字的
//!   排版基线（Baseline）出现 1~2 像素的垂直微小错位。本模块在引擎端将序号与候选词文本一体化合成，
//!   由底层文本排版引擎（Pango / HarfBuzz）统一在一个文本行（Text Run）中执行基线对齐光栅化，彻底根除垂直浮动。
//! - **核心优势**：
//!   1. 彻底解决基线对齐问题，排版严格坐底平齐；
//!   2. 支持现代空白风、圆圈序号、括号风、极简风及自定义选词键动态指示等丰富样式；
//!   3. 引入四分之一窄空格（Thin Space: `\u{2009}`）微排版控制，视觉紧凑且利落。
//! - **代价与局限**：视图展示文本合并了序号，向目标应用提交（Commit）文本时必须始终使用原始的 `Candidate.text`，
//!   严禁误提交格式化后的展示文本。

use crate::config::CandidateIndexStyle;

/// 常用圆圈序号字符常量映射（1..=10）
pub const CIRCLED_NUMBERS: [&str; 10] = ["①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩"];

/// 根据设定的序号展示风格与页内相对索引，将候选词文本一体化格式化为整行展示字符串
///
/// # 参数
/// - `style`: 候选词序号风格
/// - `index`: 候选词在当前页内的相对索引（从 0 开始）
/// - `text`: 候选词原始文本（如“你好”）
/// - `custom_keys`: 可选的自定义选词键映射（如 `['a', 's', 'd', 'f', 'g']`），若提供则优先使用按键字符指示
pub fn format_candidate_display(
    style: CandidateIndexStyle,
    index: usize,
    text: &str,
    custom_keys: Option<&[char]>,
) -> String {
    // 优先支持自定义按键映射指示（如 a.你好、s.世界）
    if let Some(keys) = custom_keys {
        if let Some(&ch) = keys.get(index) {
            return format!("{ch}. {text}");
        }
    }

    // 默认数字键选词指示：第 1~9 项显示 1~9，第 10 项显示 0
    let key_num = (index + 1) % 10;

    match style {
        // 现代空白风：纯数字 + 半角空格，干净清爽无噪点
        CandidateIndexStyle::ModernSpace => format!("{key_num} {text}"),
        // 经典点号风：传统输入法习惯（1. ~ 9.，0.）
        CandidateIndexStyle::ClassicDot => format!("{key_num}. {text}"),
        // 圆圈序号风：① ~ ⑩，紧凑优雅，后接四分之一窄空格
        CandidateIndexStyle::Circled => {
            let sym = CIRCLED_NUMBERS.get(index).copied().unwrap_or("•");
            format!("{sym}\u{2009}{text}")
        }
        // 方括号提示风：[1] ~ [9]，[0]
        CandidateIndexStyle::SquareBracket => format!("[{key_num}] {text}"),
        // 圆括号提示风：(1) ~ (9)，(0)
        CandidateIndexStyle::Parenthesis => format!("({key_num}) {text}"),
        // 全极简无序号：仅在首选词加小圆点前缀，后续候选词保持纯文本
        CandidateIndexStyle::MinimalFirstDot => {
            if index == 0 {
                format!("•\u{2009}{text}")
            } else {
                text.to_string()
            }
        }
        // 纯文本完全无序号
        CandidateIndexStyle::None => text.to_string(),
    }
}
