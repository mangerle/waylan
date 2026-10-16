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

#[test]
fn test_viterbi_beam_deduplication_preserves_diversity() {
    let mut table = LexiconTable::new();
    // 第 1 步构造多个不同前驱字以填满前驱束状态
    for (i, &ch) in ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛"]
        .iter()
        .enumerate()
    {
        table.insert("yi", ch, 20_000 - (i as u32) * 100);
    }
    // 第 2 步："qing'hua" 存在超高频词“情话”（无后续搭配）与次高频词“清华”（与第 3 步“大学”强搭配）
    // 若第 2 步未按末端词去重，8 个前驱节点与“情话”组合将占满全部 8 个 BEAM_WIDTH 槽位，把“清华”挤出束外
    table.insert("qing'hua", "情话", 50_000);
    table.insert("qing'hua", "清华", 10_000);
    // 第 3 步："da'xue" => "大学"，与“清华”具有 +20,000 Bigram 亲和度
    table.insert("da'xue", "大学", 20_000);

    let syllables = vec![
        "yi".to_string(),
        "qing".to_string(),
        "hua".to_string(),
        "da".to_string(),
        "xue".to_string(),
    ];
    let best = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("应解码成功");
    assert_eq!(
        best.text, "甲清华大学",
        "按末端词去重后，次高分词‘清华’不应被‘情话’的多前驱副本挤出束外"
    );
}

#[test]
fn test_dynamic_beam_width_scaling() {
    assert_eq!(ViterbiDecoder::dynamic_beam_width(1), 8);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(4), 8);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(5), 12);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(8), 12);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(9), 16);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(15), 16);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(16), 20);
    assert_eq!(ViterbiDecoder::dynamic_beam_width(30), 20);
}

#[test]
fn test_viterbi_super_long_sentence_beam_stability() {
    let mut table = LexiconTable::new();
    let words = [
        ("wo'men", "我们", 30_000),
        ("dou", "都", 25_000),
        ("shi", "是", 28_000),
        ("zhi'neng", "智能", 20_000),
        ("shu'ru'fa", "输入法", 22_000),
        ("de", "的", 40_000),
        ("zhong'shi", "忠实", 15_000),
        ("yong'hu", "用户", 26_000),
    ];
    for (pinyin, hanzi, score) in words {
        table.insert(pinyin, hanzi, score);
    }

    let syllables = vec![
        "wo".to_string(),
        "men".to_string(),
        "dou".to_string(),
        "shi".to_string(),
        "zhi".to_string(),
        "neng".to_string(),
        "shu".to_string(),
        "ru".to_string(),
        "fa".to_string(),
        "de".to_string(),
        "zhong".to_string(),
        "shi".to_string(),
        "yong".to_string(),
        "hu".to_string(),
    ];
    let best = ViterbiDecoder::decode_sentence(&syllables, "", &table).expect("超长句应解码成功");
    assert_eq!(best.text, "我们都是智能输入法的忠实用户");
}

#[test]
fn test_viterbi_decode_k_best_multi_candidates() {
    let mut table = LexiconTable::with_sample_data();
    table.insert("zhong'guo", "中国", 80_000);
    table.insert("zhong'guo", "钟表", 10_000);
    table.insert("ren'min", "人民", 70_000);
    table.insert("ren'min", "仁民", 5_000);

    let syllables = vec![
        "zhong".to_string(),
        "guo".to_string(),
        "ren".to_string(),
        "min".to_string(),
    ];

    // 请求 Top-3 候选路径
    let candidates = ViterbiDecoder::decode_k_best(&syllables, "", &table, 3);
    assert!(!candidates.is_empty(), "K-Best 应至少返回 1 条路径");
    assert!(candidates.len() <= 3, "返回条数不得超过 k");

    // 验证最优解为“中国人民”
    assert_eq!(candidates[0].text, "中国人民");

    // 验证所有候选文本互不相同（去重性）
    let mut unique_texts = std::collections::HashSet::new();
    for cand in &candidates {
        assert!(
            unique_texts.insert(cand.text.clone()),
            "K-Best 输出结果中不得包含重复文本: {}",
            cand.text
        );
    }

    // 验证分数单调不增（降序排列）
    for window in candidates.windows(2) {
        assert!(
            window[0].score >= window[1].score,
            "K-Best 路径得分必须保持降序排列"
        );
    }
}
