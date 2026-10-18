//! 文本处理与字符判定核心工具
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、排版渲染与语音规整提供统一的字符判定与全半角转换基准，彻底消除各子模块散落的重复逻辑与判定区间不一致。
//! - **核心优势**：纯栈内定长快速匹配与无堆分配内联优化，纳秒级判定。
//! - **代价与局限**：CJK 字符范围收敛于当前主流 Unicode 标准中的表意文字及扩展区，不包含非汉字类的全角假名或谚文。

/// 判断字符是否为 CJK 表意汉字（覆盖汉字零“〇”、扩展 A~I 区、基本汉字区、兼容汉字及第三辅助平面汉字）
///
/// # 判定区间
/// - `\u{3007}`: 汉字零“〇”
/// - `\u{3400}`..=`\u{4DBF}`: CJK 统一表意文字扩展 A 区
/// - `\u{4E00}`..=`\u{9FFF}`: CJK 统一表意文字基本区
/// - `\u{F900}`..=`\u{FAFF}`: CJK 兼容表意文字
/// - `\u{20000}`..=`\u{2EBEF}`: CJK 统一表意文字扩展 B~F 区
/// - `\u{2F800}`..=`\u{2FA1F}`: CJK 兼容表意文字补充
/// - `\u{30000}`..=`\u{323AF}`: CJK 统一表意文字扩展 G~I 区
#[inline]
pub fn is_cjk_character(c: char) -> bool {
    matches!(
        c,
        '\u{3007}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2EBEF}'
            | '\u{2F800}'..='\u{2FA1F}'
            | '\u{30000}'..='\u{323AF}'
    )
}

/// 将半角 ASCII 字符转换为全角字符
///
/// # 映射规则
/// - 半角空格 `' '` 转换为全角空格 `'\u{3000}'`
/// - 半角可见 ASCII 字符（`0x21..=0x7E`）加上偏移量 `0xFEE0` 映射为全角字符（`0xFF01..=0xFF5E`）
/// - 其余字符保持不变
#[inline]
pub fn to_full_width_char(c: char) -> char {
    match c {
        ' ' => '\u{3000}',
        '!'..='~' => {
            let code = c as u32 + 0xfee0;
            char::from_u32(code).unwrap_or(c)
        }
        _ => c,
    }
}

/// 将包含半角 ASCII 的字符串批量转换为全角字符串
#[inline]
pub fn to_full_width_string(s: &str) -> String {
    s.chars().map(to_full_width_char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cjk_character() {
        // 基本区汉字
        assert!(is_cjk_character('中'));
        assert!(is_cjk_character('文'));
        assert!(is_cjk_character('瀾'));

        // 汉字零“〇”
        assert!(is_cjk_character('〇'));

        // 扩展区与生僻字
        assert!(is_cjk_character('\u{3400}'));
        assert!(is_cjk_character('\u{4DBF}'));
        assert!(is_cjk_character('\u{20000}'));

        // 非汉字字符应判定为 false
        assert!(!is_cjk_character('a'));
        assert!(!is_cjk_character('Z'));
        assert!(!is_cjk_character('1'));
        assert!(!is_cjk_character('，'));
        assert!(!is_cjk_character('。'));
        assert!(!is_cjk_character(' '));
    }

    #[test]
    fn test_full_width_conversion() {
        assert_eq!(to_full_width_char('a'), 'ａ');
        assert_eq!(to_full_width_char('A'), 'Ａ');
        assert_eq!(to_full_width_char('1'), '１');
        assert_eq!(to_full_width_char(' '), '\u{3000}');
        assert_eq!(to_full_width_char('中'), '中');
        assert_eq!(
            to_full_width_string("hello 123!"),
            "ｈｅｌｌｏ\u{3000}１２３！"
        );
    }
}
