use super::*;

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
    assert!(
        !engine.candidates().is_empty() && engine.candidates().len() <= 5,
        "注入日期动态扩展项后单页候选数不得超过 page_size"
    );

    // 连续向后翻页，验证处于末页时不会翻入空白页
    for _ in 0..5 {
        let page_cands = engine.next_page();
        assert!(
            !page_cands.is_empty(),
            "末页向后翻页应触发越界保护，始终保持非空候选列表"
        );
        assert!(page_cands.len() <= 5);
    }
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
    let user_words = engine.user_model.lookup_user_words("bei'jing'xi'an", 202);
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
    assert_eq!(engine.partial_commit_segments.len(), 1);

    // 用户在剩余的拼音中按下退格键修改拼音
    engine.pop_char();
    assert!(
        engine.partial_commit_segments.is_empty(),
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
