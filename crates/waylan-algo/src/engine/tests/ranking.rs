use super::super::*;
use crate::types::CandidateTier;

#[test]
fn test_rule_normalized_candidate_outranks_broken_sentence() {
    let mut table = LexiconTable::new();
    table.insert("gu", "股", 9_000);
    table.insert("ei", "诶", 8_000);
    table.insert("gui", "贵", 7_000);
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    for ch in "guei".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.all_candidates();
    let rank = candidates
        .iter()
        .position(|c| c.text == "贵")
        .expect("guei 应经 uei->ui 归一召回 贵");
    assert_eq!(candidates[rank].tier, CandidateTier::RuleNormalization);
    assert_eq!(
        rank, 0,
        "确定性归一候选必须压过两音节生造整句（股诶）占据首位"
    );
}

#[test]
fn test_rule_normalized_candidate_stays_below_exact_word() {
    let mut table = LexiconTable::new();
    table.insert("ni'ou", "尼欧", 9_000);
    table.insert("ni", "你", 9_500);
    table.insert("ou", "欧", 8_000);
    table.insert("niu", "牛", 9_000);
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    for ch in "niou".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.all_candidates();
    let ni_ou = candidates
        .iter()
        .position(|c| c.text == "尼欧")
        .expect("ni'ou 精确成词必须存在");
    let niu = candidates
        .iter()
        .position(|c| c.text == "牛")
        .expect("niou 应经 iou->iu 归一召回 牛");

    // 有意保留的保守取舍：归一候选属对键入的二次解释，不得劫持原生成词的首位，
    // 但必须进入首页可见区，避免用户误以为纠错完全失效。
    assert!(ni_ou < niu, "精确成词应排在归一候选之前");
    assert_eq!(candidates[niu].tier, CandidateTier::RuleNormalization);
    assert!(niu < 7, "归一候选必须落在首页可翻页范围内");
}

#[test]
fn test_complete_tail_extension_and_exact_word_protection() {
    let mut table = LexiconTable::new();
    table.insert("ni", "你", 9_000);
    table.insert("ha", "哈", 5_000);
    table.insert("ni'hao", "你好", 50_000);
    table.insert("zhong'di", "种地", 2_000);
    table.insert("zhong'dian", "重点", 80_000);

    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    // 1. niha 虽已能拆为 ni'ha，仍必须保留半成品补全「你好」
    for ch in "niha".chars() {
        engine.push_char(ch);
    }
    let niha_texts: Vec<&str> = engine
        .all_candidates()
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert!(niha_texts.contains(&"你好"), "niha 应保留补全候选 你好");

    // 2. zhongdi 精确整词「种地」词频低于「重点」，但仍必须稳居第一位
    engine.reset();
    for ch in "zhongdi".chars() {
        engine.push_char(ch);
    }
    let cands = engine.all_candidates();
    assert_eq!(cands[0].text, "种地", "精确全拼整词必须排在第一位");
    assert!(
        cands.iter().any(|c| c.text == "重点"),
        "zhongdi 应同时给出补全候选 重点"
    );
}

#[test]
fn test_sentence_head_segment_partial_commit() {
    let mut table = LexiconTable::new();
    table.insert("ni'hao", "你好", 50_000);
    table.insert("zhong'guo", "中国", 50_000);
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    for ch in "nihaozhongguo".chars() {
        engine.push_char(ch);
    }

    let cands = engine.all_candidates();
    assert_eq!(cands[0].text, "你好中国");
    let head_idx = cands
        .iter()
        .position(|c| c.text == "你好" && c.consumed_len == Some(5))
        .expect("长句应包含句首词段候选 你好 且 consumed_len 为 5");
    assert!(head_idx <= 2, "句首词段候选应紧跟在整句之后");

    let (committed, _) = engine
        .select_candidate(head_idx, 1000)
        .expect("选中句首词段应成功");
    assert_eq!(committed, "你好");
    assert_eq!(engine.raw_input(), "zhongguo");
}

#[test]
fn test_mixed_full_pinyin_and_jianpin_recall() {
    let mut table = LexiconTable::new();
    table.insert("wan", "完", 10_000);
    table.insert("yong", "用", 10_000);
    table.insert("wen", "文", 10_000);
    table.insert("wan'quan'ke'yi", "完全可以", 50_000);
    table.insert("wei'qu'kuan'yu", "委屈宽裕", 90_000);
    table.insert("yong'zhong'wen", "用中文", 48_000);
    table.insert("yong'zhe'wu", "用者无", 90_000);

    let config = AlgoConfig {
        enable_acronym: true,
        ..Default::default()
    };
    let mut engine = AlgoEngine::new(table, config);

    // 1. wanqky -> 完全可以 (wan'q'k'y)
    for ch in "wanqky".chars() {
        engine.push_char(ch);
    }
    let cands = engine.all_candidates();
    assert!(
        cands.iter().any(|c| c.text == "完全可以"),
        "wanqky 应通过全拼混简拼召回「完全可以」，实际: {:?}",
        cands.iter().take(5).map(|c| &c.text).collect::<Vec<_>>()
    );

    // 2. yongzwen -> 用中文 (yong'z'wen)
    engine.reset();
    for ch in "yongzwen".chars() {
        engine.push_char(ch);
    }
    let cands = engine.all_candidates();
    assert!(
        cands.iter().any(|c| c.text == "用中文"),
        "yongzwen 应通过全拼混简拼召回「用中文」，实际: {:?}",
        cands.iter().take(5).map(|c| &c.text).collect::<Vec<_>>()
    );
}

#[test]
fn test_realtime_emoji_suggest_at_third_slot() {
    let mut table = LexiconTable::new();
    table.insert("kai'xin", "开心", 60_000);
    table.insert("kai'xin", "凯新", 30_000);

    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    for ch in "kaixin".chars() {
        engine.push_char(ch);
    }
    let cands = engine.all_candidates();
    assert_eq!(cands[0].text, "开心");
    assert!(cands.len() >= 3, "开心 候选后应在第三位插入 Emoji");
    assert_eq!(
        cands[2].text, "\u{1F60A}",
        "kaixin 应在第三候选位推荐笑脸 Emoji"
    );

    // 关闭开关后不应插入 Emoji
    let mut table2 = LexiconTable::new();
    table2.insert("kai'xin", "开心", 60_000);
    table2.insert("kai'xin", "凯新", 30_000);
    let mut engine_off = AlgoEngine::new(
        table2,
        AlgoConfig {
            enable_emoji_suggest: false,
            ..Default::default()
        },
    );
    for ch in "kaixin".chars() {
        engine_off.push_char(ch);
    }
    let cands_off = engine_off.all_candidates();
    assert!(
        !cands_off.iter().any(|c| c.text == "\u{1F60A}"),
        "关闭 enable_emoji_suggest 后不应插入 Emoji"
    );
}

#[test]
fn test_fuzzy_fallback_when_fuzzy_disabled() {
    let mut table = LexiconTable::new();
    table.insert("zi", "自", 1000);
    table.insert("dao", "到", 1000);
    table.insert("zhi'dao", "知道", 80_000);
    table.insert("dan'si", "单丝", 200);
    table.insert("dan'shi", "但是", 120_000);

    // 默认关闭模糊音
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    assert!(!engine.config.fuzzy.has_active_rules());

    // 1. zidao 无精确整词，应兜底召回高频模糊整词「知道」
    for ch in "zidao".chars() {
        engine.push_char(ch);
    }
    let cands = engine.all_candidates();
    assert!(
        cands.iter().any(|c| c.text == "知道"),
        "关闭模糊音时 zidao 应兜底召回「知道」，实际: {:?}",
        cands.iter().take(5).map(|c| &c.text).collect::<Vec<_>>()
    );

    // 2. dansi 仅有低频精确整词「单丝」，应兜底召回高频模糊整词「但是」
    engine.reset();
    for ch in "dansi".chars() {
        engine.push_char(ch);
    }
    let cands2 = engine.all_candidates();
    assert!(
        cands2.iter().any(|c| c.text == "但是"),
        "关闭模糊音时 dansi 应兜底召回「但是」，实际: {:?}",
        cands2.iter().take(5).map(|c| &c.text).collect::<Vec<_>>()
    );
}
