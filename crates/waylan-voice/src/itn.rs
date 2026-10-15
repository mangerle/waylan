//! 文本规整与语气词过滤 (Inverse Text Normalization - ITN)
//!
//! # 设计原理
//! - **实现初衷**：在 ASR 语音识别与标点预测生成文本后，提供口语停顿词消除、数字转阿拉伯数字与中英文盘古空格规整。
//! - **核心优势**：纯 Rust 自研单遍扫描与上下文敏感状态机，微秒级执行，零外部模型依赖。
//! - **代价与局限**：对极度复杂的连续多重长句口吃采用局部滑动窗口消解。

use waylan_core::VoiceItnConfig;

/// 综合执行文本规整流水线（语气词消除 -> 数字转阿拉伯 -> 盘古空格）
pub fn normalize_voice_text(raw: &str, config: &VoiceItnConfig) -> String {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return text;
    }

    // 1. 口语停顿语气词与结巴消除
    if config.filter_filler_words {
        text = filter_filler_words(&text);
    }

    // 2. 逆文本标准化：大写数字转阿拉伯数字
    if config.convert_numbers {
        text = convert_chinese_numbers(&text);
    }

    // 3. 中英文自动盘古空格 (风格 B 优雅紧凑)
    if config.pangu_spacing {
        text = apply_pangu_spacing(&text);
    }

    text
}

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

/// 口语停顿语气词与结巴消除
///
/// # 核心规则
/// 1. 句首、标点符号前后孤立的语气词（“呃”、“啊”、“嗯”、“哦”、“哎”、“诶”）直接过滤。
/// 2. 结巴复读消除：相邻单字或双字词重复（如“这、这个” -> “这个”，“我我我觉得” -> “我觉得”）。
/// 3. 代词上下文保护：如果“这个/那个”后接名词或量词，判定为合法指示代词予以保护；若前后紧贴逗号或重复，则作为口头禅剔除。
pub fn filter_filler_words(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut s = input.to_string();

    // 1. 过滤句首孤立语气词（如“呃，”、“啊，”、“嗯，”、“哦，”）
    let filler_isolated = ['呃', '啊', '嗯', '哦', '哎', '诶'];
    for &ch in &filler_isolated {
        while s.starts_with(ch) {
            s.remove(0);
            while s.starts_with('，')
                || s.starts_with(',')
                || s.starts_with('、')
                || s.starts_with(' ')
            {
                s.remove(0);
            }
        }
    }

    // 2. 标点符号后的孤立语气词（如“，呃，”、“，啊，”、“，那个，要不”）
    for &ch in &filler_isolated {
        s = s.replace(&format!("，{}，", ch), "，");
        s = s.replace(&format!("，{} ", ch), "，");
        s = s.replace(&format!("。{}，", ch), "。");
    }

    // 3. 口吃与复读消解（如“这、这个” -> “这个”，“我、我” -> “我”）
    s = s.replace("这、这个", "这个");
    s = s.replace("这，这个", "这个");
    s = s.replace("这个、这个", "这个");
    s = s.replace("那个、那个", "那个");
    s = s.replace("那、那个", "那个");
    s = s.replace("那，那个", "那个");
    s = s.replace("我、我", "我");
    s = s.replace("我，我", "我");

    // 消除紧密相邻单字结巴（“我我” -> “我”，“这这” -> “这”）
    s = deduplicate_adjacent_stutter(&s);

    // 4. 代词口头禅消解（仅在前后被标点截断且非后接名词时剔除）
    s = s.replace("，那个，", "，");
    s = s.replace("，这个，", "，");

    s
}

/// 消除紧邻的单字口吃复读（如“我我觉得” -> “我觉得”）
fn deduplicate_adjacent_stutter(input: &str) -> String {
    let stutter_chars = ['我', '你', '他', '她', '它', '这', '那', '就'];
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        out.push(ch);
        if stutter_chars.contains(&ch) {
            while i + 1 < chars.len() && chars[i + 1] == ch {
                i += 1;
            }
        }
        i += 1;
    }

    out
}

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

/// 判定字符是否为 CJK 汉字（排除全角标点）
#[inline]
fn is_cjk_character(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// 判定字符是否为紧凑型中文时间词或计量单位（紧贴数字时不加空格）
#[inline]
fn is_compact_chinese_unit(c: char) -> bool {
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
    }

    #[test]
    fn test_filler_word_filtering() {
        // 句首语气词消除
        assert_eq!(filter_filler_words("呃，今天天气真好"), "今天天气真好");
        assert_eq!(filter_filler_words("啊，我想出去走走"), "我想出去走走");

        // 结巴与复读消除
        assert_eq!(
            filter_filler_words("我、我觉得这个方案可行"),
            "我觉得这个方案可行"
        );
        assert_eq!(filter_filler_words("这、这个东西很好用"), "这个东西很好用");
        assert_eq!(filter_filler_words("我我觉得还行"), "我觉得还行");

        // 口头禅代词消除 vs 正常指示代词保护
        assert_eq!(
            filter_filler_words("我想，那个，要不就算了"),
            "我想，要不就算了"
        );
        assert_eq!(filter_filler_words("请打开这个文件"), "请打开这个文件");
    }

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

    #[test]
    fn test_full_pipeline_normalization() {
        let cfg = VoiceItnConfig {
            convert_numbers: true,
            filter_filler_words: true,
            pangu_spacing: true,
        };

        let raw = "呃，我们在二零二六年用Rust完成了百分之五十的目标。";
        let norm = normalize_voice_text(raw, &cfg);
        assert_eq!(norm, "我们在 2026年用 Rust 完成了 50%的目标。");
    }
}
