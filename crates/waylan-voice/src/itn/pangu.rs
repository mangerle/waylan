//! 语音排版盘古空格算法（风格 B：优雅紧凑）
//!
//! # 设计原理
//! - **实现初衷**：在语音识别结果中自动协调中文汉字与西文字母、数字的排版间隙，提供更符合现代中文排版美学的文本呈现。
//! - **核心优势**：直接复用 `waylan_core::is_cjk_character` 全面覆盖表意文字；结合口语场景对紧凑度量衡与时间单位（如“2026年”、“50%”）进行免空格保护。
//! - **代价与局限**：线性预扫描单字前后上下文，未结合分词词典进行跨词深度分析。

use waylan_core::is_cjk_character;

/// 中英文与数字自动盘古空格（风格 B：优雅紧凑）
///
/// # 设计规则
/// 1. 汉字与半角英文字母相连时插入空格（如 `用Rust` -> `用 Rust`）。
/// 2. 汉字全角标点与英文相连时不加空格，避免多余视觉空白（如 `Rust，提高效率`）。
/// 3. 数字后紧跟常见中文时间词、量词或百分号时保持紧凑，不加空格（如 `2026年`、`第1个`、`50%`）。
/// 4. 汉字与独立数字相连时插入空格（如 `版本 2 发布`）。
pub fn apply_pangu_spacing(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut result = String::with_capacity(input.len() + 16);

    for i in 0..n {
        let curr = chars[i];
        result.push(curr);

        if i + 1 < n {
            let next = chars[i + 1];

            // 规则判定：中英文相连、汉字与数字相连（紧凑量词/时间词除外）插入盘古空格
            let should_insert_space = (is_cjk_character(curr) && next.is_ascii_alphabetic())
                || (curr.is_ascii_alphabetic() && is_cjk_character(next))
                || (is_cjk_character(curr) && next.is_ascii_digit())
                || (curr.is_ascii_digit()
                    && is_cjk_character(next)
                    && !is_compact_chinese_unit(next));

            if should_insert_space {
                result.push(' ');
            }
        }
    }

    result
}

/// 判定字符是否为紧凑型中文时间词或计量单位（紧贴数字时不加空格）
#[inline]
pub(crate) fn is_compact_chinese_unit(c: char) -> bool {
    matches!(
        c,
        '年' | '月'
            | '日'
            | '号'
            | '点'
            | '分'
            | '秒'
            | '个'
            | '只'
            | '支'
            | '本'
            | '张'
            | '条'
            | '台'
            | '度'
            | '元'
            | '块'
            | '万'
            | '亿'
            | '吨'
            | '克'
            | '米'
            | '里'
            | '%'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pangu_spacing_style_b() {
        // 1. 中英文相连加空格
        assert_eq!(apply_pangu_spacing("用Rust编写微澜"), "用 Rust 编写微澜");
        assert_eq!(
            apply_pangu_spacing("微澜WayLan输入法"),
            "微澜 WayLan 输入法"
        );

        // 2. 中文全角标点不加多余空格
        assert_eq!(
            apply_pangu_spacing("使用Rust，编写WayLan。"),
            "使用 Rust，编写 WayLan。"
        );

        // 3. 数字与量词/时间词保持紧凑
        assert_eq!(
            apply_pangu_spacing("我在2026年买了1台电脑"),
            "我在 2026年买了 1台电脑"
        );
        assert_eq!(apply_pangu_spacing("降价了50%"), "降价了 50%");

        // 4. 汉字与独立数字加空格
        assert_eq!(apply_pangu_spacing("版本2发布"), "版本 2 发布");

        // 5. 汉字零“〇”应正确识别为 CJK 汉字
        assert_eq!(apply_pangu_spacing("二〇二六年Rust"), "二〇二六年 Rust");
    }
}
