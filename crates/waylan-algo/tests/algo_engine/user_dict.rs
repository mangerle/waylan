//! 算法引擎用户词库 WAL 持久化与拼音借音切分校准集成测试

use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;

#[test]
fn test_engine_user_dict_wal_persistence_across_restart() {
    let wal_path = std::env::temp_dir().join("test_engine_user_wal_persistence.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    // 1. 第一个引擎实例：绑定 WAL 路径，选择次位同音候选词触发自动追加落盘
    {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine.table_mut().insert("ji'shu", "技术", 14_000);
        engine.table_mut().insert("ji'shu", "基数", 10_000);
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
        engine2.table_mut().insert("ji'shu", "技术", 14_000);
        engine2.table_mut().insert("ji'shu", "基数", 10_000);
        engine2.set_user_dict_path(&wal_path);
        assert_eq!(engine2.user_adaptive_total_entries(), 1);

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

#[test]
fn test_engine_tijiao_and_tian_suboptimal_borrowing_split_calibration() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    // 1. 复现 tijiao 场景：2音节最优路径 ti'jiao vs 3音节零声母借音碎切路径 ti'ji'ao
    engine.table_mut().insert("ti'jiao", "提交", 27_723);
    engine.table_mut().insert("ti'jiao", "体教", 144);
    engine.table_mut().insert("ti'jiao", "踢脚", 115);
    engine.table_mut().insert("ti'jiao", "啼叫", 92);
    engine.table_mut().insert("ti'ji'ao", "替吉奥", 342);
    engine.table_mut().insert("ti", "提", 10_000);
    engine.table_mut().insert("ti", "体", 9_000);

    for ch in "tijiao".chars() {
        engine.push_char(ch);
    }

    assert_eq!(engine.formatted_preedit(), "ti'jiao");
    let cands = engine.candidates();
    assert!(cands.len() >= 5);
    assert_eq!(
        cands[0].text, "提交",
        "tijiao 首选必须为 2 字高频词‘提交’，不得被借音碎切的 3 字词‘替吉奥’反超"
    );
    assert_eq!(cands[1].text, "体教");
    assert_eq!(cands[2].text, "踢脚");
    assert_eq!(cands[3].text, "啼叫");
    assert_eq!(cands[4].text, "替吉奥");

    // 2. 验证单音节 tian 场景：1音节最优路径 tian vs 2音节借音碎切路径 ti'an
    engine.reset();
    engine.table_mut().insert("tian", "天", 50_000);
    engine.table_mut().insert("ti'an", "提案", 3_485);

    for ch in "tian".chars() {
        engine.push_char(ch);
    }

    assert_eq!(engine.formatted_preedit(), "tian");
    let tian_cands = engine.candidates();
    assert!(!tian_cands.is_empty());
    assert_eq!(
        tian_cands[0].text, "天",
        "单音节 tian 首选必须为单字‘天’，不得被借音拆分的‘提案’反超"
    );
}
