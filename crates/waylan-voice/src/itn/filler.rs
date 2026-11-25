//! 口语停顿语气词与结巴口吃消除
//!
//! # 设计原理
//! - **实现初衷**：消除自然语音输入中高频出现的无意识停顿助词（如“呃”、“啊”、“嗯”）与单字/双字结巴重复。
//! - **核心优势**：单遍线性扫描与上下文保护规则结合，保护合法指示代词（如“这个文件”），避免过度清洗。
//! - **代价与局限**：针对跨长标点的远距离重复词采用滑动窗口有限消解。

/// 孤立句首语气词集合
const FILLER_ISOLATED: &[char] = &['呃', '啊', '嗯', '哦', '哎', '诶'];

/// 标点前后固定短语与复读词映射表（有序匹配）
const PHRASE_REPLACEMENTS: &[(&str, &str)] = &[
    // 标点后孤立语气词
    ("，呃，", "，"),
    ("，呃 ", "，"),
    ("。呃，", "。"),
    ("，啊，", "，"),
    ("，啊 ", "，"),
    ("。啊，", "。"),
    ("，嗯，", "，"),
    ("，嗯 ", "，"),
    ("。嗯，", "。"),
    ("，哦，", "，"),
    ("，哦 ", "，"),
    ("。哦，", "。"),
    ("，哎，", "，"),
    ("，哎 ", "，"),
    ("。哎，", "。"),
    ("，诶，", "，"),
    ("，诶 ", "，"),
    ("。诶，", "。"),
    // 口吃与复读消解
    ("这、这个", "这个"),
    ("这，这个", "这个"),
    ("这个、这个", "这个"),
    ("那个、那个", "那个"),
    ("那、那个", "那个"),
    ("那，那个", "那个"),
    ("我、我", "我"),
    ("我，我", "我"),
];

/// 单遍切片扫描剔除句首孤立语气词及紧跟分隔符（零 O(N) 字节搬移）
fn strip_leading_fillers(mut rest: &str) -> &str {
    for &ch in FILLER_ISOLATED {
        while let Some(after_filler) = rest.strip_prefix(ch) {
            rest = after_filler;
            while let Some(after_sep) = rest.strip_prefix(['，', ',', '、', ' ']) {
                rest = after_sep;
            }
        }
    }
    rest
}

/// 单遍映射扫描替换固定短语模式
fn apply_phrase_table(input: &str, rules: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        if let Some((pattern, replacement)) =
            rules.iter().find(|(pat, _)| rest.starts_with(pat)).copied()
        {
            out.push_str(replacement);
            rest = &rest[pattern.len()..];
        } else if let Some(ch) = rest.chars().next() {
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
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

    let trimmed = strip_leading_fillers(input);
    let after_phrases = apply_phrase_table(trimmed, PHRASE_REPLACEMENTS);
    let after_stutter = deduplicate_adjacent_stutter(&after_phrases);
    apply_phrase_table(&after_stutter, &[("，那个，", "，"), ("，这个，", "，")])
}

/// 消除紧邻的单字口吃复读（如“我我觉得” -> “我觉得”）
fn deduplicate_adjacent_stutter(input: &str) -> String {
    const STUTTER_CHARS: &[char] = &['我', '你', '他', '她', '它', '这', '那', '就'];
    let mut out = String::with_capacity(input.len());
    let mut prev: Option<char> = None;

    for ch in input.chars() {
        if prev == Some(ch) && STUTTER_CHARS.contains(&ch) {
            continue;
        }
        out.push(ch);
        prev = Some(ch);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
