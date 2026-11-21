//! 算法引擎上下文联想、空间相邻键位容错与拼写纠错集成测试

use waylan_algo::types::{AlgoConfig, CandidateTier};
use waylan_algo::AlgoEngine;

#[test]
fn test_engine_commit_context_association() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 插入具有同音竞争的词条："shi'jian" => "事件"（静态分 14,000）与 "时间"（静态分 10,000）
    engine.table_mut().insert("shi'jian", "事件", 14_000);
    engine.table_mut().insert("shi'jian", "时间", 10_000);

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
    engine.table_mut().insert("cheng", "成", 10_000);

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
    engine.table_mut().insert("ying'xiang", "影响", 50_000);
    engine.table_mut().insert("ying'xiang", "影像", 40_000);
    engine.table_mut().insert("ying", "应", 30_000);
    engine.table_mut().insert("xian", "先", 30_000);
    engine.table_mut().insert("ai", "爱", 30_000);
    engine.table_mut().insert("neng'gou", "能够", 35_000);
    engine.table_mut().insert("na'ge", "那个", 34_000);

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
    engine.table_mut().insert("shi'jian", "时间", 50_000);
    engine.table_mut().insert("de", "的", 80_000);

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

#[test]
fn test_engine_habitual_alias_en_and_adaptive_boost() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 1. 首次冷启动输入 en：标准全拼“恩”排第 1，“嗯”作为习惯音排第 2
    for ch in "en".chars() {
        engine.push_char(ch);
    }
    let candidates = engine.candidates();
    assert!(candidates.len() >= 2);
    assert_eq!(candidates[0].text, "恩");
    assert_eq!(candidates[1].text, "嗯");
    assert_eq!(candidates[1].comment.as_deref(), Some("[习惯音]"));

    // 2. 用户选择“嗯”上屏（索引 1）
    let (committed, remaining) = engine.select_candidate(1, 1_700_000_000).expect("选中‘嗯’");
    assert_eq!(committed, "嗯");
    assert_eq!(remaining, 0);
    assert!(engine.raw_input().is_empty());

    // 3. 用户再次输入 en：在用户自适应模型加权驱动下，“嗯”成功跃升置顶至第 1 位
    for ch in "en".chars() {
        engine.push_char(ch);
    }
    let candidates_after = engine.candidates();
    assert!(!candidates_after.is_empty());
    assert_eq!(
        candidates_after[0].text, "嗯",
        "经过一次选择后，用户自适应模型应将‘嗯’置顶至首选第 1 位"
    );
}

#[test]
fn test_engine_habitual_alias_compound_words_enen_and_enhao() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 1. 测试连打词组 enen => “嗯嗯”
    for ch in "enen".chars() {
        engine.push_char(ch);
    }
    let candidates_enen = engine.candidates();
    assert!(!candidates_enen.is_empty());
    assert_eq!(candidates_enen[0].text, "嗯嗯");
    let (committed, rem) = engine
        .select_candidate(0, 1_700_000_000)
        .expect("选中‘嗯嗯’");
    assert_eq!(committed, "嗯嗯");
    assert_eq!(rem, 0);

    // 2. 测试复合词组 enhao => “嗯好”
    for ch in "enhao".chars() {
        engine.push_char(ch);
    }
    let candidates_enhao = engine.candidates();
    assert!(!candidates_enhao.is_empty());
    assert_eq!(candidates_enhao[0].text, "嗯好");
    let (committed2, rem2) = engine
        .select_candidate(0, 1_700_000_000)
        .expect("选中‘嗯好’");
    assert_eq!(committed2, "嗯好");
    assert_eq!(rem2, 0);

    // 3. 测试其他叹词音别名 me => 召回“么”与“呣”
    for ch in "me".chars() {
        engine.push_char(ch);
    }
    let cands_me = engine.candidates();
    let texts_me: Vec<&str> = cands_me.iter().map(|c| c.text.as_str()).collect();
    assert!(texts_me.contains(&"么"));
    assert!(texts_me.contains(&"呣"));
}

#[test]
fn test_engine_long_typo_and_english_fallback_connectivity() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine.table_mut().insert("wo", "我", 80_000);
    engine.table_mut().insert("de", "的", 90_000);
    engine.table_mut().insert("da", "大", 60_000);
    engine.table_mut().insert("fa", "发", 50_000);
    engine.table_mut().insert("sha", "傻", 45_000);
    engine.table_mut().insert("du", "读", 40_000);
    engine.table_mut().insert("wan", "完", 45_000);
    engine.table_mut().insert("shi", "是", 75_000);
    engine.table_mut().insert("sa", "撒", 30_000);

    let input = "wodedasdefasdhadwduwdsdwdadwdsdasdadsad";
    for ch in input.chars() {
        engine.push_char(ch);
    }

    let preedit = engine.formatted_preedit();
    assert!(
        preedit.contains("s'de") && preedit.contains("du'w"),
        "preedit 应将非法音节拆分为合法前缀与简拼，实际: {preedit}"
    );

    let cands = engine.candidates();
    assert!(!cands.is_empty());
    assert_eq!(
        cands[0].consumed_len,
        Some(input.len()),
        "第 1 候选必须完整覆盖全长输入（整句置顶），实际: {:?}",
        cands[0]
    );
    assert!(
        cands[0].text.starts_with("我的") && cands[0].text.ends_with("sad"),
        "第 1 候选应保留前段中文解码并在末尾兜底英文 sad，实际: {}",
        cands[0].text
    );
}
