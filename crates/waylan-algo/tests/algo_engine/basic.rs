//! 算法引擎基础拼音召回、V 模式与扩展选词集成测试

use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;

#[test]
fn test_engine_end_to_end_xian() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "xian".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty());

    let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
    // 验证多路径召回：同时出现“先”与“西安”
    assert!(texts.contains(&"先"));
    assert!(texts.contains(&"西安"));
}

#[test]
fn test_engine_v_mode() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "v20*5".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty());
    assert_eq!(candidates[0].text, "100");

    let (text, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选词");
    assert_eq!(text, "100");
    assert_eq!(remaining, 0);
    assert!(engine.raw_input().is_empty());
}

#[test]
fn test_engine_select_and_partial_commit() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "wo'men".chars() {
        engine.push_char(ch);
    }

    let (text, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选词");
    assert_eq!(text, "我们");
    assert_eq!(remaining, 0);
}

#[test]
fn test_engine_women_exact_first_and_preedit() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "women".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty());
    // 整词“我们”必须排在第 1 位
    assert_eq!(candidates[0].text, "我们");
    // 预编辑文本必须智能显示为 "wo'men"
    assert_eq!(engine.formatted_preedit(), "wo'men");
}

#[test]
fn test_engine_dynamic_date_expand_and_select() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "riqi".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(candidates.len() >= 2);
    // 验证第 2 位为可展开的动态日期功能项
    assert!(candidates[1].text.contains('年'));
    assert!(candidates[1].is_expandable);
    assert_eq!(candidates[1].sub_candidates.len(), 7);

    // 验证展开控制（支持在 index 0 聚焦时按 Tab 自动定位并展开）
    assert!(engine.toggle_expand(0));
    assert!(engine.is_expanded());
    assert_eq!(engine.selected_index(), 1);

    // 验证二级卡片选择
    let (chosen_text, remaining) = engine.select_expanded_item(1, 2).expect("选择连字符日期");
    assert!(chosen_text.contains('-'));
    assert_eq!(remaining, 0);
}

#[test]
fn test_engine_acronym_matching() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 1. 测试 bj 简拼召回 北京
    for ch in "bj".chars() {
        engine.push_char(ch);
    }
    let candidates_bj = engine.candidates();
    assert!(!candidates_bj.is_empty());
    assert_eq!(candidates_bj[0].text, "北京");

    // 2. 测试 yyds 简拼召回 永远的神
    engine.reset();
    for ch in "yyds".chars() {
        engine.push_char(ch);
    }
    let candidates_yyds = engine.candidates();
    assert!(!candidates_yyds.is_empty());
    assert_eq!(candidates_yyds[0].text, "永远的神");
}

#[test]
fn test_engine_custom_phrase_and_english_and_symbol() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine
        .resources_mut()
        .custom_phrases
        .add_entry("mymail".to_string(), "user@waylan.org".to_string());

    // 1. 测试自定义短语置顶
    for ch in "mymail".chars() {
        engine.push_char(ch);
    }
    let cands_mail = engine.candidates();
    assert!(!cands_mail.is_empty());
    assert_eq!(cands_mail[0].text, "user@waylan.org");

    // 2. 测试英文单词联想 (rust)
    engine.reset();
    for ch in "rust".chars() {
        engine.push_char(ch);
    }
    let cands_rust = engine.candidates();
    assert!(cands_rust.iter().any(|c| c.text == "rust"));

    // 3. 测试符号联想 (dianzan)
    engine.reset();
    for ch in "dianzan".chars() {
        engine.push_char(ch);
    }
    let cands_zan = engine.candidates();
    assert!(cands_zan.iter().any(|c| c.text == "\u{1F44D}"));
}

#[test]
fn test_engine_entity_protection() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 1. 测试内置实体置顶推荐 (wayland)
    for ch in "wayland".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    assert!(!cands.is_empty(), "候选列表不应为空");
    assert_eq!(cands[0].text, "wayland");
    assert_eq!(cands[0].comment.as_deref(), Some("专名"));

    // 2. 测试动态注册实体 (deepmind)
    engine.reset();
    engine.register_entity("DeepMind");
    for ch in "deepmind".chars() {
        engine.push_char(ch);
    }
    let dm_cands = engine.candidates();
    assert!(!dm_cands.is_empty());
    assert_eq!(dm_cands[0].text, "DeepMind");
    assert_eq!(dm_cands[0].comment.as_deref(), Some("专名"));
}

#[test]
fn test_engine_shenme_and_initial_l_recall() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine.table_mut().insert("shen'me", "什么", 2_500_000);
    engine.table_mut().insert("shen'me", "甚么", 30_000);
    engine.table_mut().insert("le", "了", 5_521_194);
    engine.table_mut().insert("lai", "来", 963_666);

    for ch in "shenme".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    assert!(!cands.is_empty());
    assert_eq!(cands[0].text, "什么", "输入 shenme 时‘什么’必须排在首位");

    engine.reset();
    for ch in "shen'me".chars() {
        engine.push_char(ch);
    }
    let cands_sep = engine.candidates();
    assert!(!cands_sep.is_empty());
    assert_eq!(
        cands_sep[0].text, "什么",
        "输入带分词符 shen'me 时‘什么’必须排在首位"
    );

    engine.reset();
    engine.push_char('l');
    let cands_l = engine.candidates();
    assert!(!cands_l.is_empty());
    assert_eq!(cands_l[0].text, "了", "单声母 l 展开单字时‘了’必须排在首位");
}
