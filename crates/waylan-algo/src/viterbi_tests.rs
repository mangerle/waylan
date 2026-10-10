use super::*;
use crate::table::LexiconTable;

#[test]
fn test_viterbi_single_word() {
    let table = LexiconTable::with_sample_data();
    let syllables = vec!["ni".to_string(), "hao".to_string()];
    let best = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("应解码成功");
    assert_eq!(best.text, "你好");
    assert!(best.comment.is_some());
}

#[test]
fn test_viterbi_long_sentence() {
    let mut table = LexiconTable::with_sample_data();
    table.insert("lai", "来", 8_000);
    table.insert("le", "了", 8_500);

    let syllables = vec![
        "wo".to_string(),
        "men".to_string(),
        "lai".to_string(),
        "le".to_string(),
    ];
    let best = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("应解码成功");
    assert_eq!(best.text, "我们来了");
}

#[test]
fn test_viterbi_remaining_expansion() {
    let table = LexiconTable::with_sample_data();
    let syllables = vec!["zhong".to_string()];
    let best = ViterbiDecoder::decode_sentence(&syllables, "g", &table).expect("应解码成功");
    assert_eq!(best.text, "中国");
}

#[test]
fn test_viterbi_prefix_binding_prefers_compound_word() {
    let mut table = LexiconTable::with_sample_data();
    table.insert("shi'jian", "时间", 50_000);
    table.insert("de", "的", 80_000);

    let syllables = vec![
        "wo".to_string(),
        "men".to_string(),
        "de".to_string(),
        "shi".to_string(),
    ];
    let best = ViterbiDecoder::decode_sentence(&syllables, "j", &table).expect("应解码成功");
    assert_eq!(
        best.text, "我们的时间",
        "末尾音节与声母应优先绑定为双字词‘时间’，而非碎字"
    );
}

#[test]
fn test_viterbi_multi_initial_acronym_binding() {
    let mut table = LexiconTable::with_sample_data();
    table.insert("gong'tong", "共同", 50_000);
    table.insert("de", "的", 80_000);
    table.insert("gan'jue", "感觉", 60_000);

    let syllables = vec![
        "wo".to_string(),
        "men".to_string(),
        "gong".to_string(),
        "tong".to_string(),
        "de".to_string(),
        "g".to_string(),
    ];
    let best = ViterbiDecoder::decode_sentence(&syllables, "j", &table).expect("应解码成功");
    assert_eq!(
        best.text, "我们共同的感觉",
        "末尾多声母应整体绑定为简拼‘感觉’，绝不碎拼为‘个及’"
    );
}

#[test]
fn test_viterbi_bigram_context_disambiguation() {
    let mut table = LexiconTable::with_sample_data();
    // 插入具有同音竞争的词条："bei'jing" => "北京" 与 "背景"（静态分故意设相同或相近）
    table.insert("bei'jing", "背景", 15_000);
    table.insert("shi'jian", "时间", 10_000);

    let syllables = vec!["bei'jing".to_string(), "shi'jian".to_string()];
    let best = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("应解码成功");
    assert_eq!(
        best.text, "北京时间",
        "由于Bigram转移亲和度，应优先命中北京时间"
    );
}

#[test]
fn test_viterbi_commit_context_disambiguation() {
    let mut table = LexiconTable::with_sample_data();
    // 插入竞争词条："shi'jian" => "事件"（静态分 14,000）与 "时间"（静态分 10,000）
    table.insert("shi'jian", "事件", 14_000);
    table.insert("shi'jian", "时间", 10_000);

    let syllables = vec!["shi".to_string(), "jian".to_string()];
    // 在无上下文时，静态分更高的“事件”会胜出
    let no_ctx = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("应解码成功");
    assert_eq!(no_ctx.text, "事件");

    // 当注入上屏历史上下文“北京”时，由于（北京->时间）具有高亲和度，“时间”将逆袭胜出
    let with_ctx =
        ViterbiDecoder::decode_sentence_with_context(&syllables, "", &table, Some("北京"))
            .expect("应解码成功");
    assert_eq!(
        with_ctx.text, "时间",
        "注入上屏上下文北京后，应优先推荐时间"
    );
}
