use super::super::*;

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
