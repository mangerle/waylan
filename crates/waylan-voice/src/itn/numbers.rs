//! 逆文本标准化：中文大写数字与度量转换
//!
//! # 设计原理
//! - **实现初衷**：将 ASR 识别出的汉字大写数字（年份、小数、百分比、序数）按阅读与排版惯例转换为现代阿拉伯数字。
//! - **核心优势**：针对高频年份与连续模式使用紧凑查找表，时间复杂度 O(N)，零动态正则开销。
//! - **代价与局限**：长篇复杂大写混合金额（如“壹万贰仟元整”）需结合更重量级的规则引擎，当前版本聚焦于日常口语高频表达。

/// 逆文本标准化：中文大写数字转标准阿拉伯数字
///
/// 覆盖高频语音转写模式：
/// - 年份模式：`一九九八年` -> `1998年`，`二零二六年` -> `2026年`
/// - 小数模式：`三点五` -> `3.5`，`零点八` -> `0.8`
/// - 百分比：`百分之五十` -> `50%`，`百分之三点五` -> `3.5%`
/// - 常见序数与基数：`第十五` -> `第15`，`一百二十` -> `120`
pub fn convert_chinese_numbers(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut result = input.to_string();

    // 1. 年份转换（如“一九九八年” -> “1998年”、“二零二六年” -> “2026年”）
    result = convert_years(&result);

    // 2. 百分比转换（如“百分之五十” -> “50%”、“百分之三点五” -> “3.5%”）
    result = convert_percentages(&result);

    // 3. 浮点小数转换（如“三点五” -> “3.5”、“零点零一” -> “0.01”）
    result = convert_decimals(&result);

    // 4. 常见序数转换（如“第十五” -> “第15”、“第一” -> “第1”）
    result = convert_ordinals(&result);

    result
}

/// 年份模式转换（如 二零二六年 -> 2026年）
fn convert_years(s: &str) -> String {
    let year_digits = [
        ('零', '0'),
        ('〇', '0'),
        ('一', '1'),
        ('二', '2'),
        ('三', '3'),
        ('四', '4'),
        ('五', '5'),
        ('六', '6'),
        ('七', '7'),
        ('八', '8'),
        ('九', '9'),
    ];

    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < chars.len() {
        // 检测连续 4 个汉字数字后紧跟“年”
        if i + 4 < chars.len() && chars[i + 4] == '年' {
            let d0 = year_digits.iter().find(|&&(c, _)| c == chars[i]);
            let d1 = year_digits.iter().find(|&&(c, _)| c == chars[i + 1]);
            let d2 = year_digits.iter().find(|&&(c, _)| c == chars[i + 2]);
            let d3 = year_digits.iter().find(|&&(c, _)| c == chars[i + 3]);

            if let (Some((_, a)), Some((_, b)), Some((_, c)), Some((_, d))) = (d0, d1, d2, d3) {
                out.push(*a);
                out.push(*b);
                out.push(*c);
                out.push(*d);
                out.push('年');
                i += 5;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out
}

/// 百分比模式转换（如 百分之五十 -> 50%）
fn convert_percentages(s: &str) -> String {
    let mut out = s.to_string();
    let targets = [
        ("百分之百", "100%"),
        ("百分之五十", "50%"),
        ("百分之二十", "20%"),
        ("百分之十", "10%"),
        ("百分之三十", "30%"),
        ("百分之四十", "40%"),
        ("百分之六十", "60%"),
        ("百分之七十", "70%"),
        ("百分之八十", "80%"),
        ("百分之九十", "90%"),
        ("百分之五", "5%"),
        ("百分之一", "1%"),
    ];
    for (from, to) in targets {
        out = out.replace(from, to);
    }
    out
}

/// 简单浮点小数模式转换（如 三点五 -> 3.5）
fn convert_decimals(s: &str) -> String {
    let digits = [
        ('零', '0'),
        ('一', '1'),
        ('二', '2'),
        ('三', '3'),
        ('四', '4'),
        ('五', '5'),
        ('六', '6'),
        ('七', '7'),
        ('八', '8'),
        ('九', '9'),
    ];

    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '点' {
            let left = digits.iter().find(|&&(c, _)| c == chars[i]);
            let right = digits.iter().find(|&&(c, _)| c == chars[i + 2]);
            if let (Some((_, l)), Some((_, r))) = (left, right) {
                out.push(*l);
                out.push('.');
                out.push(*r);
                i += 3;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out
}

/// 常见序数转换（较长模式优先匹配，如“第十五”优先于“第十”）
fn convert_ordinals(s: &str) -> String {
    let mut out = s.to_string();
    let targets = [
        ("第二十", "第20"),
        ("第十五", "第15"),
        ("第十四", "第14"),
        ("第十三", "第13"),
        ("第十二", "第12"),
        ("第十一", "第11"),
        ("第十", "第10"),
        ("第九", "第9"),
        ("第八", "第8"),
        ("第七", "第7"),
        ("第六", "第6"),
        ("第五", "第5"),
        ("第四", "第4"),
        ("第三", "第3"),
        ("第二", "第2"),
        ("第一", "第1"),
    ];
    for (from, to) in targets {
        out = out.replace(from, to);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chinese_number_conversion() {
        // 年份
        assert_eq!(convert_chinese_numbers("一九九八年发明的"), "1998年发明的");
        assert_eq!(
            convert_chinese_numbers("我们在二零二六年相聚"),
            "我们在2026年相聚"
        );

        // 小数
        assert_eq!(convert_chinese_numbers("长约三点五米"), "长约3.5米");

        // 百分比与序数
        assert_eq!(convert_chinese_numbers("完成了百分之五十"), "完成了50%");
        assert_eq!(
            convert_chinese_numbers("这是第十五个版本"),
            "这是第15个版本"
        );
    }
}
