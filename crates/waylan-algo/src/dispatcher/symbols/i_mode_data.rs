//! I 模式序号与罗马数字静态数据表
//!
//! # 设计原理
//! - **实现初衷**：将固定的一至十序号、带圈数字、括号数字及罗马数字静态表外置，使查询算法保持紧凑。
//! - **核心优势**：查表驱动数据结构内聚，便于集中维护与国际化扩展。
//! - **代价与局限**：静态编译期数组，常驻于只读数据段。

/// 数字序号与多风格表示的静态数据表项
pub(crate) struct NumberSymbolGroup {
    pub(crate) key: &'static str,
    pub(crate) circled: &'static str,
    pub(crate) parenthesized: &'static str,
    pub(crate) dotted: &'static str,
    pub(crate) roman_upper: &'static str,
    pub(crate) roman_lower: &'static str,
    pub(crate) chinese_cap: &'static str,
    pub(crate) ordinal: &'static str,
}

/// 预置 1~10 序号与罗马数字静态查表
pub(crate) const NUMBER_SYMBOLS: &[NumberSymbolGroup] = &[
    NumberSymbolGroup {
        key: "1",
        circled: "①",
        parenthesized: "⑴",
        dotted: "⒈",
        roman_upper: "Ⅰ",
        roman_lower: "ⅰ",
        chinese_cap: "壹",
        ordinal: "第一",
    },
    NumberSymbolGroup {
        key: "2",
        circled: "②",
        parenthesized: "⑵",
        dotted: "⒉",
        roman_upper: "Ⅱ",
        roman_lower: "ⅱ",
        chinese_cap: "贰",
        ordinal: "第二",
    },
    NumberSymbolGroup {
        key: "3",
        circled: "③",
        parenthesized: "⑶",
        dotted: "⒊",
        roman_upper: "Ⅲ",
        roman_lower: "ⅲ",
        chinese_cap: "叁",
        ordinal: "第三",
    },
    NumberSymbolGroup {
        key: "4",
        circled: "④",
        parenthesized: "⑷",
        dotted: "⒋",
        roman_upper: "Ⅳ",
        roman_lower: "ⅳ",
        chinese_cap: "肆",
        ordinal: "第四",
    },
    NumberSymbolGroup {
        key: "5",
        circled: "⑤",
        parenthesized: "⑸",
        dotted: "⒌",
        roman_upper: "Ⅴ",
        roman_lower: "ⅴ",
        chinese_cap: "伍",
        ordinal: "第五",
    },
    NumberSymbolGroup {
        key: "6",
        circled: "⑥",
        parenthesized: "⑹",
        dotted: "⒍",
        roman_upper: "Ⅵ",
        roman_lower: "ⅵ",
        chinese_cap: "陆",
        ordinal: "第六",
    },
    NumberSymbolGroup {
        key: "7",
        circled: "⑦",
        parenthesized: "⑺",
        dotted: "⒎",
        roman_upper: "Ⅶ",
        roman_lower: "ⅶ",
        chinese_cap: "柒",
        ordinal: "第七",
    },
    NumberSymbolGroup {
        key: "8",
        circled: "⑧",
        parenthesized: "⑻",
        dotted: "⒏",
        roman_upper: "Ⅷ",
        roman_lower: "ⅷ",
        chinese_cap: "捌",
        ordinal: "第八",
    },
    NumberSymbolGroup {
        key: "9",
        circled: "⑨",
        parenthesized: "⑼",
        dotted: "⒐",
        roman_upper: "Ⅸ",
        roman_lower: "ⅸ",
        chinese_cap: "玖",
        ordinal: "第九",
    },
    NumberSymbolGroup {
        key: "10",
        circled: "⑩",
        parenthesized: "⑽",
        dotted: "⒑",
        roman_upper: "Ⅹ",
        roman_lower: "ⅹ",
        chinese_cap: "拾",
        ordinal: "第十",
    },
];
