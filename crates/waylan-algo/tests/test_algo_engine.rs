use waylan_algo::types::{AlgoConfig, CandidateTier};
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
    assert_eq!(candidates[1].text, "日期");
    assert!(candidates[1].is_expandable);
    assert_eq!(candidates[1].sub_candidates.len(), 7);

    // 验证展开控制
    assert!(engine.toggle_expand(1));
    assert!(engine.is_expanded);

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
    assert_eq!(dm_cands[0].text, "deepmind");
    assert_eq!(dm_cands[0].comment.as_deref(), Some("专名"));
}

#[test]
fn test_engine_commit_context_association() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 插入具有同音竞争的词条："shi'jian" => "事件"（静态分 14,000）与 "时间"（静态分 10,000）
    engine.table.insert("shi'jian", "事件", 14_000);
    engine.table.insert("shi'jian", "时间", 10_000);

    // 1. 在无上屏历史上下文时，输入 shijian 优先命中静态分更高的“事件”
    for ch in "shijian".chars() {
        engine.push_char(ch);
    }
    assert_eq!(engine.candidates()[0].text, "事件");
    engine.reset();
    assert!(engine.last_commit_context.is_none());

    // 2. 用户先输入 beijing 并选择“北京”上屏
    for ch in "beijing".chars() {
        engine.push_char(ch);
    }
    let (committed, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选中北京");
    assert_eq!(committed, "北京");
    assert_eq!(remaining, 0);
    assert_eq!(engine.last_commit_context.as_deref(), Some("北京"));

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
    assert!(engine.last_commit_context.is_none());
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

    // 验证 2：预编辑格式化文本绝不被改写为 nihao
    assert_ne!(engine.formatted_preedit(), "ni'hao");

    let candidates = engine.candidates();
    assert!(
        candidates.len() >= 2,
        "应至少包含保真降级候选与次位容错备选"
    );

    // 验证 3：第 2 位（次位）提供带有 [纠错] 标识的邻键候选词“你好”
    let typo_cand = &candidates[1];
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

    // 验证 4：选中次位容错候选上屏，完全消费掉全部 5 个误触字符并清空输入
    let (committed, remaining) = engine.select_candidate(1, 1_700_000_000).expect("选容错词");
    assert_eq!(committed, "你好");
    assert_eq!(remaining, 0, "选中纠错候选应完全清空输入缓冲区");
    assert_eq!(engine.raw_input(), "");
}

#[test]
fn test_engine_typo_adjacent_transpose_cehng() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 词库准备目标词条："cheng" => "成"
    engine.table.insert("cheng", "成", 10_000);

    // 模拟用户键入 cehng（eh 为相邻字母颠倒误敲，原本意图为 cheng）
    for ch in "cehng".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty(), "候选列表不应为空");
    // 验证包含 cheng 纠错的候选
    assert!(candidates.iter().any(|c| {
        c.tier == CandidateTier::TypoCorrection
            && c.comment
                .as_ref()
                .is_some_and(|comm| comm.contains("[纠错: cheng]"))
    }));
}

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
    engine.table.insert("ping'guo", "苹果", 10_000);
    engine.table.insert("ge", "个", 10_000);
    engine
        .custom_phrases
        .add_entry("rust".to_string(), "Rust".to_string());
    engine
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
        engine.last_commit_context.as_deref(),
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
        .table
        .insert("wo'men'de'shi'jie", "我们的世界", 50_000);
    engine.table.insert("wo'men", "我们", 20_000);
    engine.table.insert("shi'jie", "世界", 20_000);
    engine.table.insert("de", "的", 30_000);
    engine.table.insert("me", "么", 10_000);
    engine.table.insert("n", "嗯", 5_000);

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
        .table
        .insert("wo'men'de'shi'jie", "我们的世界", 50_000);
    engine.table.insert("wo'men'de", "我们的", 35_000);
    engine.table.insert("wo'men", "我们", 40_000);
    engine.table.insert("de'shi'jie", "的世界", 40_000);

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
    engine.table.insert("shi'jian", "时间", 50_000);
    engine.table.insert("de", "的", 80_000);

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
fn test_engine_typo_guard_suppression_on_valid_prefix() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    engine.table.insert("shi'jian", "时间", 50_000);
    engine.table.insert("de", "的", 80_000);

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
fn test_engine_mixed_full_pinyin_and_tail_acronym_synthesis() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 注入目标词条与简拼
    engine.table.insert("gong'tong", "共同", 50_000);
    engine.table.insert("de", "的", 80_000);
    engine.table.insert("gan'jue", "感觉", 60_000);
    engine.table.insert("guo'wu'yuan", "国务院", 55_000);

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
    engine.table.insert("ren'gong'zhi'neng", "人工智能", 31_607);
    engine.table.insert("ren'guo'zhong'nian", "人过中年", 504);
    engine.table.insert("ruo'guan'zhi'nian", "弱冠之年", 78);
    engine.table.insert("ru'guo", "如果", 25_000);
    engine.table.insert("ru'gu", "入股", 12_000);
    engine.table.insert("zhi'neng", "智能", 20_000);
    engine.table.insert("ren", "人", 18_000);

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

#[test]
fn test_engine_user_dict_wal_persistence_across_restart() {
    let wal_path = std::env::temp_dir().join("test_engine_user_wal_persistence.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    // 1. 第一个引擎实例：绑定 WAL 路径，选择次位同音候选词触发自动追加落盘
    {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine.table.insert("ji'shu", "技术", 14_000);
        engine.table.insert("ji'shu", "基数", 10_000);
        engine.set_user_dict_path(&wal_path);

        for ch in "jishu".chars() {
            engine.push_char(ch);
        }
        assert_eq!(engine.candidates()[0].text, "技术");
        let idx = engine
            .candidates()
            .iter()
            .position(|c| c.text == "基数")
            .expect("找到基数");

        // 选择“基数”上屏，自动写入 WAL
        let (committed, _) = engine
            .select_candidate(idx, 1_750_000_000)
            .expect("选中基数");
        assert_eq!(committed, "基数");
    }

    // 2. 模拟重启：新建第二个引擎实例并绑定同一 WAL 路径，验证历史词频已恢复且“基数”升至首位
    {
        let mut engine2 = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine2.table.insert("ji'shu", "技术", 14_000);
        engine2.table.insert("ji'shu", "基数", 10_000);
        engine2.set_user_dict_path(&wal_path);
        assert_eq!(engine2.user_model.total_entries(), 1);

        for ch in "jishu".chars() {
            engine2.push_char(ch);
        }
        assert_eq!(
            engine2.candidates()[0].text,
            "基数",
            "重启后从 WAL 恢复的用户词频应将‘基数’提权至首位"
        );
    }

    let _ = std::fs::remove_file(&wal_path);
}
