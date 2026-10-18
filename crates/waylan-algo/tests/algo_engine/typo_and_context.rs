//! 算法引擎上下文联想、空间相邻键位容错与拼写纠错集成测试

use waylan_algo::types::{AlgoConfig, CandidateTier};
use waylan_algo::AlgoEngine;

#[test]
fn test_engine_commit_context_association() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 插入具有同音竞争的词条："shi'jian" => "事件"（静态分 14,000）与 "时间"（静态分 10,000）
    engine.resources.table.insert("shi'jian", "事件", 14_000);
    engine.resources.table.insert("shi'jian", "时间", 10_000);

    // 1. 在无上屏历史上下文时，输入 shijian 优先命中静态分更高的“事件”
    for ch in "shijian".chars() {
        engine.push_char(ch);
    }
    assert_eq!(engine.candidates()[0].text, "事件");
    engine.reset();
    assert!(engine.last_commit_context().is_none());

    // 2. 用户先输入 beijing 并选择“北京”上屏
    for ch in "beijing".chars() {
        engine.push_char(ch);
    }
    let (committed, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选中北京");
    assert_eq!(committed, "北京");
    assert_eq!(remaining, 0);
    assert_eq!(engine.last_commit_context(), Some("北京"));

    // 3. 用户紧接着输入 shijian：跨句历史上下文生效，由于“北京时间”强亲和度，“时间”逆袭首选！
    for ch in "shijian".chars() {
        engine.push_char(ch);
    }
    let candidates = engine.candidates();
    assert!(!candidates.is_empty());
    assert_eq!(
        candidates[0].text, "时间",
        "在跨句上下文‘北京’的驱动下，‘时间’应成功逆袭置顶首位"
    );

    // 4. 用户按 Esc/reset，上下文重置清空
    engine.reset();
    assert!(engine.last_commit_context().is_none());
}

#[test]
fn test_engine_spatial_typo_tolerance_nihso() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 模拟用户键入 nihso（其中 s 为 a 的 QWERTY 键盘紧邻误触按键，原本意图为 nihao 你好）
    for ch in "nihso".chars() {
        engine.push_char(ch);
    }

    // 验证 1：原始击键字母绝对保真（绝不篡改底层输入缓冲区）
    assert_eq!(engine.raw_input(), "nihso");

    // 验证 2：预编辑格式化文本绝不被改写为 nihao，且按容错跨度切分为 ni'hso
    assert_ne!(engine.formatted_preedit(), "ni'hao");
    assert_eq!(engine.formatted_preedit(), "ni'hso");

    let candidates = engine.candidates();
    assert!(!candidates.is_empty(), "应包含智能纠错置顶候选与降级备选");

    // 验证 3：拼写断流态下，首位直接置顶带有 [纠错] 标识的邻键候选词“你好”
    let typo_cand = &candidates[0];
    assert_eq!(typo_cand.text, "你好");
    assert_eq!(typo_cand.tier, CandidateTier::TypoCorrection);
    assert!(
        typo_cand
            .comment
            .as_ref()
            .unwrap()
            .contains("[纠错: ni'hao]"),
        "注释应包含纠错提示"
    );

    // 验证 4：选中首位容错候选上屏，完全消费掉全部 5 个误触字符并清空输入
    let (committed, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选容错词");
    assert_eq!(committed, "你好");
    assert_eq!(remaining, 0, "选中纠错候选应完全清空输入缓冲区");
    assert_eq!(engine.raw_input(), "");
}

#[test]
fn test_engine_typo_adjacent_transpose_cehng() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 词库准备目标词条："cheng" => "成"
    engine.resources.table.insert("cheng", "成", 10_000);

    // 模拟用户键入 cehng（eh 为相邻字母颠倒误敲，原本意图为 cheng）
    for ch in "cehng".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty(), "候选列表不应为空");
    assert_eq!(candidates[0].text, "成");
    assert_eq!(candidates[0].tier, CandidateTier::TypoCorrection);
}

#[test]
fn test_engine_typo_transpose_yingxaing_top_ranked() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine.resources.table.insert("ying'xiang", "影响", 50_000);
    engine.resources.table.insert("ying'xiang", "影像", 40_000);
    engine.resources.table.insert("ying", "应", 30_000);
    engine.resources.table.insert("xian", "先", 30_000);
    engine.resources.table.insert("ai", "爱", 30_000);
    engine.resources.table.insert("neng'gou", "能够", 35_000);
    engine.resources.table.insert("na'ge", "那个", 34_000);

    for ch in "yingxaing".chars() {
        engine.push_char(ch);
    }

    // 预编辑文本应按两音节跨度格式化为 ying'xaing，而非碎切为 ying'x'ai'n'g
    assert_eq!(engine.formatted_preedit(), "ying'xaing");

    let candidates = engine.candidates();
    assert!(candidates.len() >= 2);
    // 前两位必须直接置顶纠错成词“影响”、“影像”
    assert_eq!(candidates[0].text, "影响");
    assert_eq!(candidates[1].text, "影像");
    // 绝不允许出现由碎切路径捏造出的“应先爱能够”、“应先爱那个”
    assert!(!candidates.iter().any(|c| c.text.starts_with("应先爱")));
}

#[test]
fn test_engine_typo_guard_suppression_on_valid_prefix() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine.resources.table.insert("shi'jian", "时间", 50_000);
    engine.resources.table.insert("de", "的", 80_000);

    for ch in "womendeshij".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    // 验证状态机门禁：合法前缀打字中，前排候选绝不包含荒谬的 TypoCorrection 容错词
    for (idx, c) in candidates.iter().take(3).enumerate() {
        assert_ne!(
            c.tier,
            CandidateTier::TypoCorrection,
            "第 {} 位候选不应为容错词: {:?}",
            idx + 1,
            c
        );
    }
}
