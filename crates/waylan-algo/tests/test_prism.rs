use waylan_algo::prism::PrismEngine;
use waylan_algo::types::FuzzyConfig;

#[test]
fn test_dag_ambiguity_xian() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("xian", &fuzzy);
    let key_strings: Vec<String> = paths.iter().map(|p| p.to_key_string()).collect();

    // 必须同时包含单音节 xian 与双音节 xi'an
    assert!(key_strings.contains(&"xian".to_string()));
    assert!(key_strings.contains(&"xi'an".to_string()));
}

#[test]
fn test_explicit_split_quote() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("xi'an", &fuzzy);
    assert!(!paths.is_empty());
    assert_eq!(paths[0].syllables, vec!["xi", "an"]);

    let paths_prefix = PrismEngine::build_paths("xi'a", &fuzzy);
    assert!(paths_prefix.iter().any(|p| p.syllables == vec!["xi", "a"]));
    assert!(paths_prefix
        .iter()
        .any(|p| p.syllables == vec!["xi"] && p.remaining == "a"));
}

#[test]
fn test_partial_remaining() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("shux", &fuzzy);
    assert!(!paths.is_empty());

    let has_shu_x = paths
        .iter()
        .any(|p| p.syllables == vec!["shu"] && p.remaining == "x");
    assert!(has_shu_x, "shux 应当被切分为 syllables=[shu], remaining=x");
}

#[test]
fn test_strict_raw_initials_segmentation() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("qwdwwqswsqw", &fuzzy);
    assert!(!paths.is_empty(), "切分路径不应为空");
    let best_path = &paths[0];
    assert_eq!(
        best_path.to_key_string(),
        "q'w'd'w'w'q's'w's'q'w",
        "声母切分音节必须严格保持用户输入的原始字母，绝不允许换字母"
    );
}

#[test]
fn test_zero_initial_borrowing_split_cost() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("jintianxingqitian", &fuzzy);
    assert!(!paths.is_empty());
    assert_eq!(paths[0].to_key_string(), "jin'tian'xing'qi'tian");
    let sub_split_opt = paths
        .iter()
        .find(|p| p.to_key_string() == "jin'tian'xing'qi'ti'an");
    if let Some(sub_path) = sub_split_opt {
        assert!(
            paths[0].cost < sub_path.cost,
            "最优完整星期天路径代价值必须低于借调零声母碎切路径"
        );
    }
}

#[test]
fn test_typo_adjacent_transpose_graph() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("cehng", &fuzzy);
    assert!(!paths.is_empty(), "切分路径不应为空");
    let has_cheng = paths
        .iter()
        .any(|p| p.syllables.contains(&"cheng".to_string()));
    assert!(
        has_cheng,
        "输入 cehng 应通过相邻颠倒虚边成功容错切分出 cheng"
    );
}

#[test]
fn test_typo_spatial_substitution_graph() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("nihso", &fuzzy);
    assert!(!paths.is_empty(), "切分路径不应为空");
    let has_ni_hao = paths.iter().any(|p| p.to_key_string() == "ni'hao");
    assert!(
        has_ni_hao,
        "输入 nihso 应通过高斯邻键虚边成功容错切分出 ni'hao"
    );
}

#[test]
fn test_fuzzy_nine_rules_expansion() {
    let fuzzy = FuzzyConfig::all_enabled();
    let zong_alts = PrismEngine::expand_fuzzy_syllables("zong", &fuzzy);
    assert!(zong_alts.contains(&"zhong".to_string()));

    let lu_alts = PrismEngine::expand_fuzzy_syllables("lu", &fuzzy);
    assert!(lu_alts.contains(&"nu".to_string()));
    assert!(lu_alts.contains(&"ru".to_string()));

    let fa_alts = PrismEngine::expand_fuzzy_syllables("fa", &fuzzy);
    assert!(fa_alts.contains(&"ha".to_string()));

    let lan_alts = PrismEngine::expand_fuzzy_syllables("lan", &fuzzy);
    assert!(lan_alts.contains(&"lang".to_string()));
    assert!(lan_alts.contains(&"nan".to_string()));
    assert!(lan_alts.contains(&"nang".to_string()));

    let sen_alts = PrismEngine::expand_fuzzy_syllables("sen", &fuzzy);
    assert!(sen_alts.contains(&"seng".to_string()));
    assert!(sen_alts.contains(&"shen".to_string()));
    assert!(sen_alts.contains(&"sheng".to_string()));
}

#[test]
fn test_fuzzy_engine_lookup_zongguo() {
    let config = waylan_algo::types::AlgoConfig {
        fuzzy: FuzzyConfig::all_enabled(),
        ..Default::default()
    };
    let mut engine = waylan_algo::AlgoEngine::with_sample_data(config);
    for ch in "zongguo".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    assert!(!cands.is_empty(), "模糊音开启时输入 zongguo 应当召回候选词");
    let has_china = cands.iter().any(|c| c.text == "中国");
    assert!(
        has_china,
        "zongguo 应当通过 z<=>zh 与 eng<=>ong 模糊音规则召回“中国”"
    );
}
