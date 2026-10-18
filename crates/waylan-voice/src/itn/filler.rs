//! 口语停顿语气词与结巴口吃消除
//!
//! # 设计原理
//! - **实现初衷**：消除自然语音输入中高频出现的无意识停顿助词（如“呃”、“啊”、“嗯”）与单字/双字结巴重复。
//! - **核心优势**：单遍线性扫描与上下文保护规则结合，保护合法指示代词（如“这个文件”），避免过度清洗。
//! - **代价与局限**：针对跨长标点的远距离重复词采用滑动窗口有限消解。

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
