use super::*;

#[test]
fn test_deduplicate() {
    let raw = vec![
        AlgoCandidate::new("中国", "zhong'guo", 10_000),
        AlgoCandidate::new("中国", "zhong'guo", 5_000),
        AlgoCandidate::new("中", "zhong", 8_000),
    ];
    let config = AlgoConfig::default();
    let result = CandidatePipeline::process(raw, &config, 0);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].text, "中国");
    assert_eq!(result[0].score, 10_000);
}

#[test]
fn test_traditional_conversion() {
    let raw = vec![
        AlgoCandidate::new("中国", "zhong'guo", 10_000),
        AlgoCandidate::new("头发", "tou'fa", 9_000),
        AlgoCandidate::new("发展", "fa'zhan", 8_500),
        AlgoCandidate::new("皇后", "huang'hou", 8_000),
        AlgoCandidate::new("发", "fa", 7_000),
    ];
    let config = AlgoConfig {
        traditional_chinese: true,
        ..Default::default()
    };

    let result = CandidatePipeline::process(raw, &config, 0);
    assert_eq!(result[0].text, "中國");
    assert_eq!(result[1].text, "頭髮");
    assert_eq!(result[2].text, "發展");
    assert_eq!(result[3].text, "皇后");
    assert_eq!(result[4].text, "發");
    assert_eq!(result[5].text, "髮");
}

#[test]
fn test_pagination() {
    let raw: Vec<AlgoCandidate> = (0..20)
        .map(|i| AlgoCandidate::new(format!("词{}", i), "ci", 20 - i as u32))
        .collect();

    let config = AlgoConfig {
        page_size: 5,
        ..Default::default()
    };

    let page0 = CandidatePipeline::process(raw.clone(), &config, 0);
    assert_eq!(page0.len(), 5);
    assert_eq!(page0[0].text, "词0");

    let page1 = CandidatePipeline::process(raw, &config, 1);
    assert_eq!(page1.len(), 5);
    assert_eq!(page1[0].text, "词5");
}

#[test]
fn test_filter_pua_and_replacement_chars_retains_cjk_extensions() {
    let raw = vec![
        AlgoCandidate::new("常用字", "chang'yong'zi", 10_000),
        AlgoCandidate::new("\u{20000}", "he", 9_000), // CJK 扩展 B 区生僻字，必须保留
        AlgoCandidate::new("\u{E000}", "pua", 8_000), // BMP 私用区，应过滤
        AlgoCandidate::new("\u{F0000}", "spua", 7_000), // 补充私用区 A，应过滤
        AlgoCandidate::new("乱码\u{FFFD}", "luan'ma", 6_000), // 替换字符，应过滤
    ];
    let config = AlgoConfig {
        filter_uncommon_charset: true,
        filter_rare_ext_chars: false,
        ..Default::default()
    };

    let result = CandidatePipeline::process(raw, &config, 0);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].text, "常用字");
    assert_eq!(result[1].text, "\u{20000}");
}

#[test]
fn test_filter_rare_ext_chars_blocks_uncommon_ext_b() {
    let raw = vec![
        AlgoCandidate::new("常用字", "chang'yong'zi", 10_000),
        AlgoCandidate::new("\u{20000}", "he", 9_000), // 非常用扩展 B 生僻单字，应屏蔽
        AlgoCandidate::new("𰻝", "biang", 8_500),     // 通用扩展字白名单，应保留
        AlgoCandidate::new("𩽾𩾌", "an'kang", 8_000), // 扩展字组成的多字词，应保留
    ];
    let config = AlgoConfig::default();

    let result = CandidatePipeline::process(raw, &config, 0);
    let texts: Vec<&str> = result.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["常用字", "𰻝", "𩽾𩾌"]);
}

#[test]
fn test_deduplicate_preserves_order_and_highest_score() {
    let raw = vec![
        AlgoCandidate::new("测试", "ce'shi", 100),
        AlgoCandidate::new("其他", "qi'ta", 90),
        AlgoCandidate::new("测试", "ce'shi", 80), // 重复项，应被剔除
        AlgoCandidate::new("微澜", "wei'lan", 70),
    ];
    let deduped = CandidatePipeline::deduplicate(raw);
    assert_eq!(deduped.len(), 3);
    assert_eq!(deduped[0].text, "测试");
    assert_eq!(deduped[0].score, 100);
    assert_eq!(deduped[1].text, "其他");
    assert_eq!(deduped[2].text, "微澜");
}

#[test]
fn test_has_illegal_syllable() {
    assert!(has_illegal_syllable("yixai"));
    assert!(has_illegal_syllable("yi'xai"));
    assert!(has_illegal_syllable("xai"));
    assert!(!has_illegal_syllable("yixia"));
    assert!(!has_illegal_syllable("yi'xia"));
    assert!(!has_illegal_syllable("nihao"));
    assert!(!has_illegal_syllable(""));
}

#[test]
fn test_single_typo_over_mixed_competition() {
    let raw = vec![
        AlgoCandidate::new("胰腺癌", "yi'xian'ai", 20_285)
            .with_tier(CandidateTier::SentenceViterbi)
            .with_comment("(yi'xian'ai)")
            .with_typo_count(0),
        AlgoCandidate::new("一下", "yi'xia", 29_478)
            .with_tier(CandidateTier::TypoCorrection)
            .with_comment("(yi'xia)")
            .with_typo_count(1),
        AlgoCandidate::new("以下", "yi'xia", 18_000)
            .with_tier(CandidateTier::TypoCorrection)
            .with_comment("(yi'xia)")
            .with_typo_count(1),
        AlgoCandidate::new("异想天开", "yi'xiang'tian'kai", 35_000)
            .with_tier(CandidateTier::TypoCorrection)
            .with_comment("(yi'xiang'tian'kai)")
            .with_typo_count(2),
    ];

    let config = AlgoConfig::default();

    // 场景 A：输入为非法音节 yixai，单次纠错“一下”应反超混拼“胰腺癌”，多次纠错不参与反超
    let boosted =
        CandidatePipeline::process_core_with_context(raw.clone(), &config, None, Some("yixai"));
    assert_eq!(boosted[0].text, "一下");
    assert_eq!(boosted[1].text, "胰腺癌");
    assert_eq!(boosted[2].text, "以下");
    assert_eq!(boosted[3].text, "异想天开");

    // 场景 B：无非法音节上下文或合法音节，混拼档仍保持高于纠错档
    let normal = CandidatePipeline::process_core_with_context(raw, &config, None, Some("yixia"));
    assert_eq!(normal[0].text, "胰腺癌");
    assert_eq!(normal[1].text, "异想天开");
    assert_eq!(normal[2].text, "一下");
    assert_eq!(normal[3].text, "以下");
}

#[test]
fn test_pure_acronym_not_affected_by_single_typo() {
    let raw = vec![
        AlgoCandidate::new("北京", "bj", 50_000)
            .with_tier(CandidateTier::Acronym)
            .with_typo_count(0),
        AlgoCandidate::new("比", "bi", 60_000)
            .with_tier(CandidateTier::TypoCorrection)
            .with_comment("(bi)")
            .with_typo_count(1),
    ];

    let config = AlgoConfig::default();
    // 纯简拼 bj 没有混拼候选，不应触发单次纠错提权，Acronym 必须保持在 TypoCorrection 之上
    let res = CandidatePipeline::process_core_with_context(raw, &config, None, Some("bj"));
    assert_eq!(res[0].text, "北京");
    assert_eq!(res[1].text, "比");
}

#[test]
fn test_candidate_ranker_exact_protection_plan_a() {
    // 场景 1：存在原生精确词时，首位必须保护给最高分精确词，高频模糊音词排第 2
    let mut list1 = vec![
        AlgoCandidate::new("常", "chang", 90_000).with_tier(CandidateTier::RuleNormalization),
        AlgoCandidate::new("长", "chang", 85_000).with_tier(CandidateTier::RuleNormalization),
        AlgoCandidate::new("藏", "cang", 30_000).with_tier(CandidateTier::ExactWord),
        AlgoCandidate::new("仓", "cang", 20_000).with_tier(CandidateTier::ExactWord),
    ];
    CandidateRanker::rank(&mut list1, false);
    assert_eq!(list1[0].text, "藏", "首选必须由最高分原生精确词锁定");
    assert_eq!(list1[1].text, "常", "次位开始高频模糊音词自然按分值呈现");
    assert_eq!(list1[2].text, "长");
    assert_eq!(list1[3].text, "仓");

    // 场景 2：若无原生精确词（仅有模糊音词），最高分模糊音词直接为第 1 位
    let mut list2 = vec![
        AlgoCandidate::new("常", "chang", 90_000).with_tier(CandidateTier::RuleNormalization),
        AlgoCandidate::new("长", "chang", 85_000).with_tier(CandidateTier::RuleNormalization),
    ];
    CandidateRanker::rank(&mut list2, false);
    assert_eq!(list2[0].text, "常");
    assert_eq!(list2[1].text, "长");
}
