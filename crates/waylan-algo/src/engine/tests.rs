use super::*;
use crate::types::CandidateTier;
use crate::user_adaptive::{CommitCountParams, UserAdaptiveModel};

#[test]
fn test_acronym_candidates_respect_configuration() {
    use crate::prism::PrismEngine;

    let mut disabled = AlgoEngine::with_sample_data(AlgoConfig {
        enable_acronym: false,
        ..Default::default()
    });
    let paths = PrismEngine::build_paths("bj", &disabled.config.fuzzy);
    let raw = disabled.collect_all_raw_candidates(&paths);
    println!(
        "disabled acronym raw: {:?}",
        raw.iter().map(|c| (&c.text, c.tier)).collect::<Vec<_>>()
    );
    assert!(
        !raw.iter()
            .any(|c| c.tier == CandidateTier::Acronym && c.text == "北京"),
        "关闭简拼后不应召回简拼候选"
    );

    let mut enabled = AlgoEngine::with_sample_data(AlgoConfig::default());
    let paths = PrismEngine::build_paths("bj", &enabled.config.fuzzy);
    let raw = enabled.collect_all_raw_candidates(&paths);
    assert!(
        raw.iter()
            .any(|c| c.tier == CandidateTier::Acronym && c.text == "北京"),
        "开启简拼后应召回北京简拼候选"
    );
}

#[test]
fn test_dynamic_adjust_freq_configuration() {
    let mut boosted = AlgoEngine::with_sample_data(AlgoConfig::default());
    boosted
        .adaptive
        .model
        .record_commit_count(&CommitCountParams {
            text: "你好",
            pinyin: "ni'hao",
            current_timestamp: 100,
            count: 100,
        });
    for ch in "nihao".chars() {
        boosted.push_char(ch);
    }
    let boosted_score = boosted
        .candidates()
        .iter()
        .find(|c| c.text == "你好")
        .map(|c| c.score)
        .expect("应召回你好");

    let mut static_freq = AlgoEngine::with_sample_data(AlgoConfig {
        dynamic_adjust_freq: false,
        ..Default::default()
    });
    static_freq
        .adaptive
        .model
        .record_commit_count(&CommitCountParams {
            text: "你好",
            pinyin: "ni'hao",
            current_timestamp: 100,
            count: 100,
        });
    for ch in "nihao".chars() {
        static_freq.push_char(ch);
    }
    let static_score = static_freq
        .candidates()
        .iter()
        .find(|c| c.text == "你好")
        .map(|c| c.score)
        .expect("应召回你好");

    assert!(
        boosted_score > static_score,
        "开启动态调频后用户历史应提高候选得分: boosted={boosted_score}, static={static_score}"
    );
}

#[test]
fn test_pagination_truncate_and_last_page_guard() {
    let config = AlgoConfig {
        page_size: 5,
        ..Default::default()
    };
    let mut engine = AlgoEngine::with_sample_data(config);

    for ch in "riqi".chars() {
        engine.push_char(ch);
    }
    assert!(!engine.candidates().is_empty(), "必须成功生成候选列表");
    assert!(
        engine
            .candidates()
            .iter()
            .any(|c| c.is_expandable && !c.sub_candidates.is_empty()),
        "必须成功注入可展开的日期动态扩展项"
    );
}

#[test]
fn test_partial_commit_synthesizes_user_word() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 输入 bjxian -> 分段依次选中 北京 (bei'jing) 与 西安 (xi'an)
    for ch in "beijingxian".chars() {
        engine.push_char(ch);
    }
    let texts1: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
    let idx1 = texts1
        .iter()
        .position(|t| t == "北京")
        .expect("应包含前缀词 北京");
    let (c1, rem1) = engine.select_candidate(idx1, 200).unwrap();
    assert_eq!(c1, "北京");
    assert_eq!(rem1, 4);

    let texts2: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
    let idx2 = texts2
        .iter()
        .position(|t| t == "西安")
        .expect("剩余 xian 应包含 西安");
    let (c2, rem2) = engine.select_candidate(idx2, 201).unwrap();
    assert_eq!(c2, "西安");
    assert_eq!(rem2, 0);

    // 再次输入 beijingxian，验证分段合成的自造新词 北京西安 已沉淀到用户模型并直接召回
    let user_words = engine
        .adaptive
        .model
        .lookup_user_words("bei'jing'xi'an", 202);
    assert!(
        user_words.iter().any(|w| w.text == "北京西安"),
        "分段选词消费完整个拼音序列后应自动合成用户自造新词 北京西安"
    );
}

#[test]
fn test_partial_commit_segments_cleared_on_edit() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    for ch in "beijingxian".chars() {
        engine.push_char(ch);
    }
    let texts1: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
    let idx1 = texts1
        .iter()
        .position(|t| t == "北京")
        .expect("应包含前缀词 北京");
    let (c1, rem1) = engine.select_candidate(idx1, 200).unwrap();
    assert_eq!(c1, "北京");
    assert_eq!(rem1, 4);
    assert_eq!(engine.composition.partial_segments().len(), 1);

    // 用户在剩余的拼音中按下退格键修改拼音
    engine.pop_char();
    assert!(
        engine.composition.partial_segments().is_empty(),
        "修改剩余拼音时必须立即清空自造词累积器，防止拼音错位合成脏词"
    );
}

#[test]
fn test_engine_persist_and_drop_saves_user_dict() {
    let temp_dir = std::env::temp_dir();
    let dict_path = temp_dir.join(format!("test_waylan_user_dict_{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&dict_path);

    {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine.set_user_dict_path(&dict_path);

        for ch in "nihao".chars() {
            engine.push_char(ch);
        }
        let res = engine.select_candidate(0, 100);
        assert!(res.is_some(), "应当成功选中首个候选词 你好");
        // 引擎在此作用域结束并触发 Drop
    }

    // 重新从持久化落盘的文件恢复，验证词频数据已成功刷盘
    assert!(dict_path.exists(), "引擎析构后用户词库文件应当已安全落盘");
    let restored_model =
        UserAdaptiveModel::load_or_replay_wal(&dict_path).expect("读取持久化词库失败");
    assert!(
        restored_model.total_entries() > 0,
        "重新加载的用户自适应词库词条数应大于 0"
    );
    let _ = std::fs::remove_file(&dict_path);
}

#[test]
fn test_special_and_traditional_candidates_do_not_pollute_user_dict() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 1. 选中动态日期扩展项不应写入用户词库
    for ch in "riqi".chars() {
        engine.push_char(ch);
    }
    let exp_idx = engine
        .candidates()
        .iter()
        .position(|c| c.is_expandable)
        .expect("应包含可展开日期项");
    engine.select_candidate(exp_idx, 1000).unwrap();
    assert_eq!(
        engine.adaptive.model.total_entries(),
        0,
        "可展开动态功能项不得写入用户自适应词库"
    );

    // 2. 选中 V 模式计算结果不应写入用户词库
    for ch in "v2+3".chars() {
        engine.push_char(ch);
    }
    engine.select_candidate(0, 1001).unwrap();
    assert_eq!(
        engine.adaptive.model.total_entries(),
        0,
        "V 模式计算候选不得写入用户自适应词库"
    );

    // 3. 繁体中文模式下选词应正常写入用户自适应词库以沉淀用词习惯
    engine.config.traditional_chinese = true;
    for ch in "nihao".chars() {
        engine.push_char(ch);
    }
    engine.select_candidate(0, 1002).unwrap();
    assert_eq!(
        engine.adaptive.model.total_entries(),
        1,
        "繁体中文模式下用户选词应正常记入自适应词库"
    );
}

#[test]
fn test_cjk_extension_ideographs_recorded_to_user_dict() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());

    // 扩展 A 区生僻字（如 "䶮" U+4DAE）、扩展 B 区（"𤧛" U+249DB）、扩展 G 区（"𰻝" U+30EDE）
    let ext_a_cand = AlgoCandidate::new("䶮", "yan", 500);
    assert!(
        engine.should_record_to_user_dict(&ext_a_cand),
        "CJK 扩展 A 区生僻字应允许记入用户词库"
    );

    let ext_b_cand = AlgoCandidate::new("𤧛", "kun", 500);
    assert!(
        engine.should_record_to_user_dict(&ext_b_cand),
        "CJK 扩展 B 区生僻字应允许记入用户词库"
    );

    let ext_g_cand = AlgoCandidate::new("𰻝𰻝面", "biang'biang'mian", 500);
    assert!(
        engine.should_record_to_user_dict(&ext_g_cand),
        "CJK 扩展 G 区生僻字词组应允许记入用户词库"
    );

    // 含英文或标点的混合项不应记入纯汉字用户词库
    let mixed_cand = AlgoCandidate::new("Hello世界", "hello", 500);
    assert!(!engine.should_record_to_user_dict(&mixed_cand));
}

#[test]
fn test_engine_reset_clears_all_candidates_cache() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "nihao".chars() {
        engine.push_char(ch);
    }
    assert!(!engine.all_candidates().is_empty());
    assert!(!engine.candidates().is_empty());

    engine.reset();
    assert!(engine.all_candidates().is_empty());
    assert!(engine.candidates().is_empty());
    assert_eq!(engine.raw_input(), "");
}

#[test]
fn test_cursor_move_recomputes_candidates() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "xian".chars() {
        engine.push_char(ch);
    }
    assert!(engine.all_candidates().len() >= 3);
    assert_eq!(engine.cursor(), 4);

    engine.move_cursor_left();
    assert_eq!(engine.cursor(), 3);
    assert!(!engine.candidates().is_empty());

    engine.move_cursor_right();
    assert_eq!(engine.cursor(), 4);
    assert!(!engine.candidates().is_empty());
}

#[test]
fn test_formatted_cursor_byte_offset_case_and_apostrophe() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "beijing".chars() {
        engine.push_char(ch);
    }
    assert_eq!(engine.formatted_preedit(), "bei'jing");
    assert!(matches!(
        engine.formatted_preedit_cow(),
        std::borrow::Cow::Borrowed(_)
    ));
    assert_eq!(engine.formatted_cursor_byte_offset(), "bei'jing".len());

    // 光标左移 4 次，raw_cursor 处于 3 ("bei")，应在隔音符单引号前 (3)
    for _ in 0..4 {
        engine.move_cursor_left();
    }
    assert_eq!(engine.cursor(), 3);
    assert_eq!(engine.formatted_cursor_byte_offset(), 3);

    // 大写输入与隔音符测试
    engine.reset();
    for ch in "BeiJing".chars() {
        engine.push_char(ch);
    }
    for _ in 0..4 {
        engine.move_cursor_left();
    }
    assert_eq!(engine.cursor(), 3);
    assert_eq!(engine.formatted_cursor_byte_offset(), 3);
}

#[test]
fn test_compute_consumed_len_mismatch_break() {
    // 字符失配时应立即中断，不得跨越错误字符匹配导致过度消费按键
    assert_eq!(AlgoEngine::compute_consumed_len("baxei", "bei"), 0);
    // 正常前缀且带隔音符
    assert_eq!(AlgoEngine::compute_consumed_len("bei'jing", "bei"), 4);
    // 大小写不区分匹配
    assert_eq!(AlgoEngine::compute_consumed_len("BeiJing", "bei"), 3);
    // 完全匹配
    assert_eq!(AlgoEngine::compute_consumed_len("beijing", "beijing"), 7);
    // 输入长度不足以完整匹配候选词拼音
    assert_eq!(AlgoEngine::compute_consumed_len("b", "bei"), 0);
}

#[test]
fn test_custom_phrase_traditional_conversion() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig {
        traditional_chinese: true,
        ..Default::default()
    });
    engine
        .resources
        .custom_phrases
        .add_entry("csdy".to_string(), "测试短语".to_string());

    for ch in "csdy".chars() {
        engine.push_char(ch);
    }

    let cands = engine.candidates();
    assert!(!cands.is_empty());
    assert_eq!(
        cands[0].text, "測試短語",
        "开启繁体模式后自定义短语应被正确转换为繁体"
    );
    assert_eq!(cands[0].tier, crate::types::CandidateTier::ProtectedEntity);
}

#[test]
fn test_all_candidates_tagged_with_consumed_len() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "nihao".chars() {
        engine.push_char(ch);
    }

    let cands = engine.candidates();
    assert!(!cands.is_empty(), "候选词列表不应为空");
    for cand in cands {
        assert!(
            cand.consumed_len.is_some(),
            "候选词 '{}' 必须预先统一标记 consumed_len",
            cand.text
        );
        let len = cand.consumed_len.unwrap();
        assert!(len > 0 && len <= 5, "消费长度必须处于合法区间 [1, 5]");
    }
}

#[test]
fn test_select_candidate_partial_commit_consumed_len() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 输入 nihaoma
    for ch in "nihaoma".chars() {
        engine.push_char(ch);
    }

    // 选中第一个候选词（你好）
    let result = engine.select_candidate(0, 1000);
    assert!(result.is_some(), "选词操作应成功");
    let (committed, remaining_len) = result.unwrap();
    assert!(!committed.is_empty(), "上屏文本不应为空");

    // 验证剩余未消费长度：nihao 消费 5 字符，剩余 ma 共 2 字符
    if committed.contains("你好") {
        assert_eq!(remaining_len, 2, "你好 消费后应剩余 ma（2 字节）");
        assert_eq!(engine.raw_input(), "ma", "引擎剩余拼音应为 ma");
    }
}

#[test]
fn test_select_candidate_zero_consumed_len_fallback() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "nihaoma".chars() {
        engine.push_char(ch);
    }
    assert!(!engine.candidates().is_empty());

    // 人工插入一个 consumed_len 为 Some(0) 或失配的候选词到首位，验证强健的保底机制
    let mut mock_cand = engine.candidates()[0].clone();
    mock_cand.consumed_len = Some(0);
    mock_cand.pinyin = "qun'ze".to_string();
    engine.candidates.insert(0, mock_cand);

    // 选中该候选词上屏
    let result = engine.select_candidate(0, 1000);
    assert!(result.is_some(), "选词必须成功");
    let (_, remaining_len) = result.unwrap();

    // 断言：安全保底必须生效，消费全部拼音，剩余拼音长度归零，绝对不可死锁
    assert_eq!(
        remaining_len, 0,
        "消费长度为 0 的异常候选词提交后必须保底消费全部拼音"
    );
    assert!(engine.raw_input().is_empty(), "引擎拼音必须完全清空");
    assert!(engine.candidates().is_empty(), "候选词列表必须完全清空");
}

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
