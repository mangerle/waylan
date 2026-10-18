//! 中西文排版盘古之白与紧凑量词过滤核心模块
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法合成上屏文本与语音流式识别规整提供统一、唯一的排版空格规范，彻底消除跨 Crate 规则失步。
//! - **核心优势**：结合 CJK 表意汉字精准判定与现代中文排版美学，智能处理中英文相连、数字接汉字以及紧凑中文量词/时间词豁免。
//! - **代价与局限**：基于单字与常见量词表进行线性快速扫描，保持微秒级超高吞吐。

use crate::text::is_cjk_character;

/// 常见中文量词与度量衡、时间、货币单位集合（按拼音或语义分类）
pub const CHINESE_CLASSIFIERS: &[&str] = &[
    // 常见单字量词
    "个",
    "只",
    "支",
    "枝",
    "张",
    "条",
    "头",
    "匹",
    "尾",
    "块",
    "片",
    "粒",
    "颗",
    "本",
    "册",
    "部",
    "首",
    "篇",
    "幅",
    "座",
    "栋",
    "扇",
    "套",
    "件",
    "双",
    "对",
    "副",
    "打",
    "群",
    "把",
    "口",
    "根",
    "排",
    "行",
    "段",
    "节",
    "项",
    "类",
    "种",
    "束",
    "串",
    "包",
    "盒",
    "箱",
    "袋",
    "瓶",
    "罐",
    "杯",
    "碗",
    "盘",
    "锅",
    "桶",
    "盆",
    "页",
    "枚",
    "尊",
    "通",
    "场",
    "顿",
    "番",
    "次",
    "回",
    "趟",
    "遍",
    "遭",
    "阵",
    "门",
    "桩",
    "任",
    "届",
    "台",
    // 度量衡单位
    "米",
    "分米",
    "厘米",
    "毫米",
    "微米",
    "纳米",
    "千米",
    "公里",
    "里",
    "尺",
    "寸",
    "丈",
    "克",
    "千克",
    "公斤",
    "斤",
    "两",
    "钱",
    "吨",
    "磅",
    "盎司",
    "升",
    "毫升",
    "立方米",
    "平方米",
    "平方",
    "公顷",
    "亩",
    // 时间与日期
    "秒",
    "分",
    "分钟",
    "刻",
    "时",
    "小时",
    "天",
    "日",
    "周",
    "月",
    "年",
    "岁",
    "代",
    "季",
    "度",
    // 货币与比率
    "元",
    "角",
    "块",
    "毛",
    "镑",
    "刀",
    "折",
    "倍",
    "成",
    "点",
    "级",
    "百分点",
    "万",
    "亿",
    "兆",
];

/// 判定单个字符是否为紧凑型中文时间词或计量单位（紧贴数字时不插入空格）
#[inline]
pub fn is_compact_chinese_unit(c: char) -> bool {
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

/// 判断词语是否属于常见中文量词或单位（用于数字与中文紧邻时的空格豁免）
#[inline]
pub fn is_chinese_classifier(word: &str) -> bool {
    CHINESE_CLASSIFIERS.contains(&word)
}

/// 检查两个相邻文本片段之间是否需要插入盘古之白（中西文智能空格）
pub fn should_insert_pangu_space(prev_context: &str, curr_commit: &str) -> bool {
    let prev_text = prev_context.trim();
    let curr_text = curr_commit.trim();

    if prev_text.is_empty() || curr_text.is_empty() {
        return false;
    }

    let prev_ch = match prev_text.chars().last() {
        Some(c) => c,
        None => return false,
    };

    let curr_ch = match curr_text.chars().next() {
        Some(c) => c,
        None => return false,
    };

    // 若前驱结尾或当前开头已有空格，则不重复插入
    if prev_context.ends_with(' ') || curr_commit.starts_with(' ') {
        return false;
    }

    // 场景 1：中文汉字接英文单词或数字（如："测试" + "Rust" => "测试 Rust"；"第" + "10" => "第 10"）
    if is_cjk_character(prev_ch) && (curr_ch.is_ascii_alphabetic() || curr_ch.is_ascii_digit()) {
        return true;
    }

    // 场景 2：英文单词接中文汉字（如："Rust" + "代码" => "Rust 代码"）
    if prev_ch.is_ascii_alphabetic() && is_cjk_character(curr_ch) {
        return true;
    }

    // 场景 3：数字接中文汉字（如："100" + "苹果" => "100 苹果"）
    // 豁免情况：若当前词为中文量词或单位（如："100" + "米" => "100米"），则不插入空格
    if prev_ch.is_ascii_digit() && is_cjk_character(curr_ch) {
        if is_chinese_classifier(curr_text) {
            return false;
        }
        return true;
    }

    false
}

/// 对完整文本应用盘古之白空格规整（适用于语音转录等全量文本后处理）
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

/// 盘古之白门面类型
pub struct PanguSpacing;

impl PanguSpacing {
    /// 检查两个相邻文本片段之间是否需要插入盘古之白空格
    #[inline]
    pub fn should_insert_pangu_space(prev_context: &str, curr_commit: &str) -> bool {
        should_insert_pangu_space(prev_context, curr_commit)
    }

    /// 对完整文本应用盘古空格格式化
    #[inline]
    pub fn apply_pangu_spacing(input: &str) -> String {
        apply_pangu_spacing(input)
    }

    /// 判断词语是否属于常见中文量词或单位
    #[inline]
    pub fn is_chinese_classifier(word: &str) -> bool {
        is_chinese_classifier(word)
    }

    /// 判定字符是否为紧凑型中文时间词或计量单位
    #[inline]
    pub fn is_compact_chinese_unit(c: char) -> bool {
        is_compact_chinese_unit(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pangu_spacing_classifier_and_cjk() {
        // 数字接单字量词/单位豁免空格
        assert!(!PanguSpacing::should_insert_pangu_space("100", "个"));
        assert!(!PanguSpacing::should_insert_pangu_space("100", "米"));
        assert!(!PanguSpacing::should_insert_pangu_space("5", "分钟"));

        // 数字接以量词开头的普通名词不得豁免空格
        assert!(PanguSpacing::should_insert_pangu_space("100", "个人"));
        assert!(PanguSpacing::should_insert_pangu_space("100", "点心"));
        assert!(PanguSpacing::should_insert_pangu_space("10", "门口"));

        // 中西文之间应插入空格
        assert!(PanguSpacing::should_insert_pangu_space("测试", "Rust"));
        assert!(PanguSpacing::should_insert_pangu_space("Rust", "代码"));

        // 汉字零“〇”应正确识别为 CJK 字符
        assert!(PanguSpacing::should_insert_pangu_space("二〇二六", "Rust"));
    }

    #[test]
    fn test_apply_pangu_spacing_full() {
        // 中英文相连加空格
        assert_eq!(apply_pangu_spacing("用Rust编写微澜"), "用 Rust 编写微澜");
        assert_eq!(
            apply_pangu_spacing("微澜WayLan输入法"),
            "微澜 WayLan 输入法"
        );

        // 中文全角标点不加多余空格
        assert_eq!(
            apply_pangu_spacing("使用Rust，编写WayLan。"),
            "使用 Rust，编写 WayLan。"
        );

        // 数字与量词/时间词保持紧凑
        assert_eq!(
            apply_pangu_spacing("我在2026年买了1台电脑"),
            "我在 2026年买了 1台电脑"
        );
        assert_eq!(apply_pangu_spacing("降价了50%"), "降价了 50%");

        // 汉字与独立数字加空格
        assert_eq!(apply_pangu_spacing("版本2发布"), "版本 2 发布");

        // 汉字零“〇”应正确识别为 CJK 汉字
        assert_eq!(apply_pangu_spacing("二〇二六年Rust"), "二〇二六年 Rust");
    }
}
