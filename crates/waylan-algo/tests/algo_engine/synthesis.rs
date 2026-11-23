//! 算法引擎智能盘古空格、前缀短词混排注入、跨词短语前缀合成与简拼合成集成测试

use waylan_algo::AlgoEngine;
use waylan_algo::types::AlgoConfig;

#[test]
fn test_pangu_spacing_helper_rules() {
    // 1. 中文汉字接英文单词或数字
    assert!(AlgoEngine::should_insert_pangu_space("测试", "Rust"));
    assert!(AlgoEngine::should_insert_pangu_space("第", "10"));

    // 2. 英文单词接中文汉字
    assert!(AlgoEngine::should_insert_pangu_space("Rust", "语言"));
    assert!(AlgoEngine::should_insert_pangu_space("Waylan", "输入法"));

    // 3. 数字接量词豁免 vs 数字接普通汉字
    assert!(!AlgoEngine::should_insert_pangu_space("100", "米"));
    assert!(!AlgoEngine::should_insert_pangu_space("20", "个"));
    assert!(!AlgoEngine::should_insert_pangu_space("5", "折"));
    assert!(!AlgoEngine::should_insert_pangu_space("2026", "年"));
    assert!(AlgoEngine::should_insert_pangu_space("100", "苹果"));
    assert!(AlgoEngine::should_insert_pangu_space("2026", "你好"));

    // 4. 同类别不插入空格
    assert!(!AlgoEngine::should_insert_pangu_space("北京", "时间"));
    assert!(!AlgoEngine::should_insert_pangu_space("hello", "world"));

    // 5. 标点符号与已有空格不重复插入
    assert!(!AlgoEngine::should_insert_pangu_space("你好", "，"));
    assert!(!AlgoEngine::should_insert_pangu_space("测试 ", "Rust"));
    assert!(!AlgoEngine::should_insert_pangu_space("测试", " Rust"));
}

#[test]
fn test_engine_auto_pangu_spacing_integration() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入目标词条与英文短语（带音节分词符）
    engine.table_mut().insert("ping'guo", "苹果", 10_000);
    engine.table_mut().insert("ge", "个", 10_000);
    engine
        .resources_mut()
        .custom_phrases
        .add_entry("rust".to_string(), "Rust".to_string());
    engine
        .resources_mut()
        .custom_phrases
        .add_entry("100".to_string(), "100".to_string());

    // 1. 默认情况下，auto_pangu_spacing 为 false，不自动追加空格
    assert!(!engine.config.auto_pangu_spacing);
    for ch in "women".chars() {
        engine.push_char(ch);
    }
    let (comm1, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
    assert_eq!(comm1, "我们");

    for ch in "rust".chars() {
        engine.push_char(ch);
    }
    let (comm2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中Rust");
    assert_eq!(comm2, "Rust", "未开启智能空格时不应包含前置空格");

    // 2. 开启智能空格配置
    engine.reset();
    engine.config.auto_pangu_spacing = true;

    // 步骤 A：中文接英文（我们 -> Rust）
    for ch in "women".chars() {
        engine.push_char(ch);
    }
    let (c1, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
    assert_eq!(c1, "我们");

    for ch in "rust".chars() {
        engine.push_char(ch);
    }
    let (c2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中Rust");
    assert_eq!(c2, " Rust", "中文接英文应自动补齐半角空格");
    assert_eq!(
        engine.last_commit_context(),
        Some("Rust"),
        "历史上下文应保存去除空格后的纯净词"
    );

    // 步骤 B：英文接中文（Rust -> 我们）
    for ch in "women".chars() {
        engine.push_char(ch);
    }
    let (c3, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
    assert_eq!(c3, " 我们", "英文接中文应自动补齐半角空格");

    // 步骤 C：数字接量词白名单豁免（100 -> 个）
    engine.reset();
    for ch in "100".chars() {
        engine.push_char(ch);
    }
    let (num_c, _) = engine.select_candidate(0, 1_700_000_000).expect("选中100");
    assert_eq!(num_c, "100");

    for ch in "ge".chars() {
        engine.push_char(ch);
    }
    let (clf_c, _) = engine
        .select_candidate(0, 1_700_000_000)
        .expect("选中量词个");
    assert_eq!(clf_c, "个", "数字接中文量词白名单应豁免，不加空格");

    // 步骤 D：数字接普通中文词（100 -> 苹果）
    engine.reset();
    for ch in "100".chars() {
        engine.push_char(ch);
    }
    let (num_c2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中100");
    assert_eq!(num_c2, "100");

    for ch in "pingguo".chars() {
        engine.push_char(ch);
    }
    let (noun_c, _) = engine.select_candidate(0, 1_700_000_000).expect("选中苹果");
    assert_eq!(noun_c, " 苹果", "数字接普通中文词应自动补齐半角空格");
}

#[test]
fn test_engine_best_path_only_no_garbage_sentence() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入词条
    engine
        .table_mut()
        .insert("wo'men'de'shi'jie", "我们的世界", 50_000);
    engine.table_mut().insert("wo'men", "我们", 20_000);
    engine.table_mut().insert("shi'jie", "世界", 20_000);
    engine.table_mut().insert("de", "的", 30_000);
    engine.table_mut().insert("me", "么", 10_000);
    engine.table_mut().insert("n", "嗯", 5_000);

    for ch in "womendeshijie".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty());
    // 验证绝不包含次级劣质碎切整句“我么嗯的世界”
    assert!(!candidates.iter().any(|c| c.text == "我么嗯的世界"));
    // 首选项必须为“我们的世界”
    assert_eq!(candidates[0].text, "我们的世界");
}

#[test]
fn test_engine_prefix_segment_candidate_injection_and_cascade() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入全词与前缀词
    engine
        .table_mut()
        .insert("wo'men'de'shi'jie", "我们的世界", 50_000);
    engine.table_mut().insert("wo'men'de", "我们的", 35_000);
    engine.table_mut().insert("wo'men", "我们", 40_000);
    engine.table_mut().insert("de'shi'jie", "的世界", 40_000);

    for ch in "womendeshijie".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(candidates.len() >= 3);
    // 1. 首位保持长句
    assert_eq!(candidates[0].text, "我们的世界");
    // 2. 候选列表中成功混排注入前缀短词“我们”与“我们的”
    assert!(candidates.iter().any(|c| c.text == "我们"));
    assert!(candidates.iter().any(|c| c.text == "我们的"));

    // 3. 找到“我们”的索引进行分段确认选择
    let women_idx = candidates
        .iter()
        .position(|c| c.text == "我们")
        .expect("应包含我们");
    let (committed, remaining_len) = engine
        .select_candidate(women_idx, 1_700_000_000)
        .expect("选中我们");

    assert_eq!(committed, "我们");
    assert!(remaining_len > 0);
    assert_eq!(engine.raw_input(), "deshijie");
    assert_eq!(engine.formatted_preedit(), "de'shi'jie");

    // 4. 接力验证：剩余拼音立刻刷新出针对 de'shi'jie 的候选
    let next_cands = engine.candidates();
    assert!(!next_cands.is_empty());
    assert_eq!(next_cands[0].text, "的世界");

    // 5. 选中后半段，完成整句录入
    let (next_committed, next_remaining) = engine
        .select_candidate(0, 1_700_000_000)
        .expect("选中的世界");
    assert_eq!(next_committed, "的世界");
    assert_eq!(next_remaining, 0);
    assert!(engine.raw_input().is_empty());
}

#[test]
fn test_engine_phrase_prefix_synthesis_womendeshij() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入目标词条：“时间”和“的”
    engine.table_mut().insert("shi'jian", "时间", 50_000);
    engine.table_mut().insert("de", "的", 80_000);

    // 模拟用户键入 womendeshij
    for ch in "womendeshij".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty(), "候选列表不应为空");

    // 验证成功合成跨词短语“我们的时间”
    let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
    assert!(
        texts.contains(&"我们的时间"),
        "候选列表中应成功包含跨词边界合成短语‘我们的时间’，当前候选: {:?}",
        texts
    );
}

#[test]
fn test_engine_mixed_full_pinyin_and_tail_acronym_synthesis() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入目标词条与简拼
    engine.table_mut().insert("gong'tong", "共同", 50_000);
    engine.table_mut().insert("de", "的", 80_000);
    engine.table_mut().insert("gan'jue", "感觉", 60_000);
    engine.table_mut().insert("guo'wu'yuan", "国务院", 55_000);

    // 1. 验证 k = 2（双声母）：womengongtongdegj -> 我们共同的感觉
    for ch in "womengongtongdegj".chars() {
        engine.push_char(ch);
    }
    let candidates = engine.candidates();
    assert!(!candidates.is_empty());
    assert_eq!(
        candidates[0].text, "我们共同的感觉",
        "首选词必须命中跨词简拼合成短语‘我们共同的感觉’"
    );

    // 2. 验证 k = 3（三声母）：womendegwy -> 我们的国务院
    engine.reset();
    for ch in "womendegwy".chars() {
        engine.push_char(ch);
    }
    let cands3 = engine.candidates();
    assert!(!cands3.is_empty());
    assert_eq!(
        cands3[0].text, "我们的国务院",
        "首选词必须命中跨词三声母简拼合成短语‘我们的国务院’"
    );
}

#[test]
fn test_engine_rgzn_multi_acronyms_and_prefix_injection() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine
        .table_mut()
        .insert("ren'gong'zhi'neng", "人工智能", 31_607);
    engine
        .table_mut()
        .insert("ren'guo'zhong'nian", "人过中年", 504);
    engine
        .table_mut()
        .insert("ruo'guan'zhi'nian", "弱冠之年", 78);
    engine.table_mut().insert("ru'guo", "如果", 25_000);
    engine.table_mut().insert("ru'gu", "入股", 12_000);
    engine.table_mut().insert("zhi'neng", "智能", 20_000);
    engine.table_mut().insert("ren", "人", 18_000);

    for ch in "rgzn".chars() {
        engine.push_char(ch);
    }

    let cands = engine.candidates();
    assert!(
        cands.len() >= 6,
        "rgzn 候选列表应包含全长词、二字前缀简拼与首声母单字: {:?}",
        cands
    );
    assert_eq!(cands[0].text, "人工智能");
    assert_eq!(cands[1].text, "人过中年");
    assert_eq!(cands[2].text, "弱冠之年");
    assert_eq!(cands[3].text, "如果");
    assert_eq!(cands[4].text, "入股");
    assert_eq!(cands[5].text, "人");

    // 验证选中第 4 项“如果”后，分段确认消费前 2 键 "rg"，剩余 "zn" 自动重算召回“智能”
    let (committed, rem_len) = engine.select_candidate(3, 1_700_000_000).expect("选中如果");
    assert_eq!(committed, "如果");
    assert_eq!(rem_len, 2);
    assert_eq!(engine.raw_input(), "zn");
    assert_eq!(engine.candidates()[0].text, "智能");
}
