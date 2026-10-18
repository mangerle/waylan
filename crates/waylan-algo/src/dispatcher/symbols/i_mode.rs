use crate::dispatcher::symbols::i_mode_data::NUMBER_SYMBOLS;
use crate::types::AlgoCandidate;

/// 辅助追加符号候选，保持分数从 10000 递减
fn push_sym(candidates: &mut Vec<AlgoCandidate>, text: &str, code: &str, comment: &str) {
    let score = (10_000u32).saturating_sub(candidates.len() as u32 * 500);
    candidates.push(AlgoCandidate::new(text, code, score).with_comment(comment));
}

/// 查询数字相关特殊符号（带圈序号、罗马数字、大写数字等）
fn query_number_symbols(input: &str, candidates: &mut Vec<AlgoCandidate>) {
    if input == "0" {
        push_sym(candidates, "⓪", "i0", "带圈数字");
        push_sym(candidates, "⓿", "i0", "黑底数字");
        push_sym(candidates, "〇", "i0", "中文零");
        push_sym(candidates, "零", "i0", "大写数字");
        return;
    }

    if let Some((idx, group)) = NUMBER_SYMBOLS
        .iter()
        .enumerate()
        .find(|(_, g)| g.key == input)
    {
        const CODES: &[&str] = &["i1", "i2", "i3", "i4", "i5", "i6", "i7", "i8", "i9", "i10"];
        let code = CODES.get(idx).copied().unwrap_or("i");
        push_sym(candidates, group.circled, code, "带圈序号");
        push_sym(candidates, group.parenthesized, code, "括号序号");
        push_sym(candidates, group.dotted, code, "带点序号");
        push_sym(candidates, group.roman_upper, code, "罗马数字");
        push_sym(candidates, group.roman_lower, code, "小写罗马");
        push_sym(candidates, group.chinese_cap, code, "大写数字");
        push_sym(candidates, group.ordinal, code, "中文序数");
    }
}

/// 查询数学运算与常用逻辑符号
fn query_math_symbols(input: &str, candidates: &mut Vec<AlgoCandidate>) {
    match input {
        "+" | "jia" => {
            push_sym(candidates, "±", "i+", "正负号");
            push_sym(candidates, "∓", "i+", "负正号");
            push_sym(candidates, "\u{2795}", "i+", "加号");
            push_sym(candidates, "＋", "i+", "全角加号");
        }
        "-" | "jian" => {
            push_sym(candidates, "－", "i-", "减号");
            push_sym(candidates, "\u{2796}", "i-", "减号");
            push_sym(candidates, "∓", "i-", "负正号");
            push_sym(candidates, "—", "i-", "破折号");
        }
        "*" | "cheng" => {
            push_sym(candidates, "×", "i*", "乘号");
            push_sym(candidates, "\u{2716}", "i*", "粗乘号");
            push_sym(candidates, "·", "i*", "间隔点");
            push_sym(candidates, "★", "i*", "五角星");
            push_sym(candidates, "☆", "i*", "空心星");
        }
        "/" | "chu" => {
            push_sym(candidates, "÷", "i/", "除号");
            push_sym(candidates, "\u{2797}", "i/", "除号");
            push_sym(candidates, "／", "i/", "全角斜杠");
            push_sym(candidates, "‰", "i/", "千分号");
            push_sym(candidates, "‱", "i/", "万分号");
        }
        "=" | "deng" => {
            push_sym(candidates, "≠", "i=", "不等号");
            push_sym(candidates, "≈", "i=", "约等于");
            push_sym(candidates, "≡", "i=", "恒等于");
            push_sym(candidates, "≤", "i=", "小于等于");
            push_sym(candidates, "≥", "i=", "大于等于");
            push_sym(candidates, "≌", "i=", "全等");
        }
        "pi" => {
            push_sym(candidates, "π", "ipi", "圆周率小写");
            push_sym(candidates, "Π", "ipi", "圆周率大写");
        }
        "deg" | "du" => {
            push_sym(candidates, "°", "ideg", "度数");
            push_sym(candidates, "℃", "ideg", "摄氏度");
            push_sym(candidates, "℉", "ideg", "华氏度");
        }
        "inf" => {
            push_sym(candidates, "∞", "iinf", "无穷大");
            push_sym(candidates, "∝", "iinf", "正比于");
        }
        "sum" => {
            push_sym(candidates, "∑", "isum", "求和符号");
        }
        "sqrt" | "gen" => {
            push_sym(candidates, "√", "isqrt", "平方根");
            push_sym(candidates, "∛", "isqrt", "立方根");
            push_sym(candidates, "∜", "isqrt", "四次方根");
        }
        _ => {}
    }
}

/// 查询几何图形与方向符号
fn query_geom_symbols(input: &str, candidates: &mut Vec<AlgoCandidate>) {
    match input {
        "jt" | "arrow" => {
            push_sym(candidates, "→", "ijt", "向右箭头");
            push_sym(candidates, "←", "ijt", "向左箭头");
            push_sym(candidates, "↑", "ijt", "向上箭头");
            push_sym(candidates, "↓", "ijt", "向下箭头");
            push_sym(candidates, "⇄", "ijt", "左右交换");
            push_sym(candidates, "↔", "ijt", "双向箭头");
            push_sym(candidates, "⇒", "ijt", "右双箭头");
            push_sym(candidates, "⇐", "ijt", "左双箭头");
        }
        "dui" | "gou" => {
            push_sym(candidates, "✓", "idui", "对号");
            push_sym(candidates, "✔", "idui", "粗对号");
            push_sym(candidates, "√", "idui", "根号/对勾");
        }
        "cuo" | "cha" => {
            push_sym(candidates, "✗", "icuo", "错号");
            push_sym(candidates, "✘", "icuo", "粗错号");
            push_sym(candidates, "×", "icuo", "叉号");
        }
        "xing" | "star" => {
            push_sym(candidates, "★", "ixing", "实心星");
            push_sym(candidates, "☆", "ixing", "空心星");
            push_sym(candidates, "✪", "ixing", "圈星");
            push_sym(candidates, "\u{2B50}", "ixing", "星星");
        }
        "xin" | "heart" => {
            push_sym(candidates, "\u{2764}", "ixin", "实心红心");
            push_sym(candidates, "♡", "ixin", "空心心形");
            push_sym(candidates, "♥", "ixin", "黑心");
        }
        "fang" | "square" => {
            push_sym(candidates, "■", "ifang", "实心方块");
            push_sym(candidates, "□", "ifang", "空心方块");
            push_sym(candidates, "◆", "ifang", "实心菱形");
            push_sym(candidates, "◇", "ifang", "空心菱形");
            push_sym(candidates, "▲", "ifang", "实心三角");
            push_sym(candidates, "△", "ifang", "空心三角");
        }
        "yuan" | "circle" => {
            push_sym(candidates, "●", "iyuan", "实心圆");
            push_sym(candidates, "○", "iyuan", "空心圆");
            push_sym(candidates, "◎", "iyuan", "双重圆");
            push_sym(candidates, "☉", "iyuan", "圆心点");
        }
        _ => {}
    }
}

/// 查询单位与货币符号
fn query_unit_symbols(input: &str, candidates: &mut Vec<AlgoCandidate>) {
    match input {
        "m" => {
            push_sym(candidates, "㎡", "im", "平方米");
            push_sym(candidates, "㎥", "im", "立方米");
            push_sym(candidates, "㎜", "im", "毫米");
            push_sym(candidates, "㎝", "im", "厘米");
            push_sym(candidates, "㎞", "im", "千米");
        }
        "rmb" | "money" => {
            push_sym(candidates, "¥", "irmb", "人民币/日元");
            push_sym(candidates, "￥", "irmb", "全角人民币");
            push_sym(candidates, "$", "irmb", "美元符号");
            push_sym(candidates, "€", "irmb", "欧元符号");
            push_sym(candidates, "£", "irmb", "英镑符号");
            push_sym(candidates, "₩", "irmb", "韩元符号");
        }
        _ => {}
    }
}

/// I 模式：根据输入查询特殊符号与序号候选
///
/// # 设计原理
/// - **实现初衷**：为数字序号、罗马数字、数学运算、方向箭头、常用几何图形和货币单位提供直观快捷的辅助查询。
/// - **核心优势**：查表驱动模式收敛分支深度，消除重复代码并补齐小写罗马数字完整序列。
/// - **代价与局限**：静态内置，仅收录高频常用符号类别。
pub(crate) fn query_i_mode(input: &str) -> Vec<AlgoCandidate> {
    let mut candidates = Vec::with_capacity(8);

    query_number_symbols(input, &mut candidates);
    query_math_symbols(input, &mut candidates);
    query_geom_symbols(input, &mut candidates);
    query_unit_symbols(input, &mut candidates);

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i_mode_roman_numerals_coverage() {
        // 验证 1~10 均完整包含大写和小写罗马数字
        let expected = [
            ("1", "Ⅰ", "ⅰ"),
            ("2", "Ⅱ", "ⅱ"),
            ("3", "Ⅲ", "ⅲ"),
            ("4", "Ⅳ", "ⅳ"),
            ("5", "Ⅴ", "ⅴ"),
            ("6", "Ⅵ", "ⅵ"),
            ("7", "Ⅶ", "ⅶ"),
            ("8", "Ⅷ", "ⅷ"),
            ("9", "Ⅸ", "ⅸ"),
            ("10", "Ⅹ", "ⅹ"),
        ];

        for (num, upper, lower) in expected {
            let cands = query_i_mode(num);
            assert!(
                cands.iter().any(|c| c.text == upper),
                "缺少 {} 的大写罗马数字 {}",
                num,
                upper
            );
            assert!(
                cands.iter().any(|c| c.text == lower),
                "缺少 {} 的小写罗马数字 {}",
                num,
                lower
            );
        }
    }

    #[test]
    fn test_i_mode_math_and_units() {
        let plus = query_i_mode("+");
        assert!(plus.iter().any(|c| c.text == "±"));

        let money = query_i_mode("rmb");
        assert!(money.iter().any(|c| c.text == "¥"));
    }
}
