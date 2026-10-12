use super::*;

#[test]
fn test_record_and_boost() {
    let mut model = UserAdaptiveModel::new();
    let now = 1_700_000_000;
    model.record_commit("先", "xian", now);
    model.record_commit("先", "xian", now + 10);

    let mut candidate = AlgoCandidate::new("先", "xian", 7_200);
    model.apply_boost(&mut candidate, now + 100);

    assert!(candidate.score > 100_000);
    assert_eq!(candidate.comment.as_deref(), Some("常用习惯"));
}

#[test]
fn test_no_double_boost_on_user_history() {
    let mut model = UserAdaptiveModel::new();
    let now = 1_700_000_000;
    model.record_commit("自造词条", "zi'zao'ci'tiao", now);

    let candidates = model.lookup_user_words("zi'zao'ci'tiao", now + 10);
    assert_eq!(candidates.len(), 1);
    let mut user_cand = candidates[0].clone();
    let original_score = user_cand.score;

    // 对 UserHistory 层级调用 apply_boost 不应重复加分或篡改自造词注释
    model.apply_boost(&mut user_cand, now + 10);
    assert_eq!(user_cand.score, original_score);
    assert_eq!(user_cand.comment.as_deref(), Some("自造词"));
}

#[test]
fn test_half_life_decay() {
    let entry = UserEntry {
        text: "测试".to_string(),
        pinyin: "ce'shi".to_string(),
        commit_count: 10,
        last_used_timestamp: 1_000_000,
    };
    let score_immediate = UserAdaptiveModel::calculate_decay_score(&entry, 1_000_000);
    // 30 天后 (30 * 86400 = 2,592,000)
    let score_30_days = UserAdaptiveModel::calculate_decay_score(&entry, 1_000_000 + 2_592_000);

    assert!((score_30_days as f64 - (score_immediate as f64 * 0.5)).abs() < 1000.0);
}

#[test]
fn test_total_entries_and_clear() {
    let mut model = UserAdaptiveModel::new();
    assert_eq!(model.total_entries(), 0);
    model.record_commit("测试", "ce'shi", 100);
    model.record_commit("模型", "mo'xing", 100);
    assert_eq!(model.total_entries(), 2);
    model.clear();
    assert_eq!(model.total_entries(), 0);
}

#[test]
fn test_wal_append_replay_and_compaction() {
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join("test_user_history.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    // 1. 流式追加记录
    let mut model = UserAdaptiveModel::new();
    model
        .append_to_wal(&wal_path, "微澜", "wei'lan", 1_700_000_000)
        .expect("写入第1条WAL");
    model
        .append_to_wal(&wal_path, "微澜", "wei'lan", 1_700_000_100)
        .expect("写入第2条WAL");
    model
        .append_to_wal(&wal_path, "输入法", "shu'ru'fa", 1_700_000_200)
        .expect("写入第3条WAL");

    // 2. 从 WAL 文件重放
    let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("WAL重放");
    assert_eq!(restored.total_entries(), 2);
    let wl_words = restored.lookup_user_words("wei'lan", 1_700_000_200);
    assert_eq!(wl_words[0].text, "微澜");
    let wl_entry = restored
        .entries
        .get("wei'lan")
        .unwrap()
        .iter()
        .find(|e| e.text == "微澜")
        .unwrap();
    assert_eq!(wl_entry.commit_count, 2);

    // 3. 压缩归并
    restored.compact_wal(&wal_path).expect("WAL归并成功");
    let compacted = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放归并后的WAL");
    assert_eq!(compacted.total_entries(), 2);
    let compacted_entry = compacted
        .entries
        .get("wei'lan")
        .unwrap()
        .iter()
        .find(|e| e.text == "微澜")
        .unwrap();
    assert_eq!(compacted_entry.commit_count, 2, "压缩后词频必须忠实保留");

    let _ = std::fs::remove_file(wal_path);
}

#[test]
fn test_wal_runtime_auto_compaction() {
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join("test_runtime_auto_compact.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    let mut model = UserAdaptiveModel::new();
    // 设置较小阈值为 5，用于测试自动触发
    model.set_compact_threshold(5);

    for i in 0..4 {
        model
            .append_to_wal(&wal_path, "测试", "ce'shi", 1_700_000_000 + i)
            .expect("追加写入");
    }
    assert_eq!(model.wal_append_counter(), 4);

    // 第 5 次追加达到阈值，但不会在追加写热路径中同步触发阻塞式压缩，计数器保持递增
    model
        .append_to_wal(&wal_path, "测试", "ce'shi", 1_700_000_010)
        .expect("追加写入");
    assert_eq!(model.wal_append_counter(), 5);

    // 在外部安全窗口（如会话失焦）调用 maybe_compact_wal 成功执行条件压缩并清零计数器
    assert!(model.maybe_compact_wal(&wal_path).expect("条件压缩"));
    assert_eq!(model.wal_append_counter(), 0);

    // 重新加载验证数据完整无损
    let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放验证");
    assert_eq!(restored.total_entries(), 1);

    let _ = std::fs::remove_file(wal_path);
}

#[test]
fn test_wal_startup_auto_compaction() {
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join("test_startup_auto_compact.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    let mut model = UserAdaptiveModel::new();
    model.set_compact_threshold(100_000); // 运行期不触发

    // 写入 60 条同一个词的流水（高冗余率：60 流水 vs 1 个词）
    for i in 0..60 {
        model
            .append_to_wal(&wal_path, "高频词", "gao'pin'ci", 1_700_000_000 + i)
            .expect("追加");
    }

    let size_before = std::fs::metadata(&wal_path).unwrap().len();

    // 启动重放检测到流水数 >= 50 且 > 词条数*3，触发自动压缩
    let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放触发启动压缩");
    assert_eq!(restored.total_entries(), 1);

    let size_after = std::fs::metadata(&wal_path).unwrap().len();
    assert!(
        size_after < size_before,
        "自动压缩后文件体积应明显缩小 ({} -> {})",
        size_before,
        size_after
    );

    let _ = std::fs::remove_file(wal_path);
}

#[test]
fn test_wal_truncation_and_corrupt_resilience() {
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join("test_wal_corrupt.wal");
    if wal_path.exists() {
        let _ = std::fs::remove_file(&wal_path);
    }

    let mut model = UserAdaptiveModel::new();
    model
        .append_to_wal(&wal_path, "完整词", "wan'zheng", 1_700_000_000)
        .expect("写入正常第一条");
    model
        .append_to_wal(&wal_path, "被截断词", "jie'duan", 1_700_000_001)
        .expect("写入将被截断的第二条");

    // 模拟断电或进程崩溃：向文件末尾随机截断 5 个字节
    let mut data = std::fs::read(&wal_path).expect("读取WAL文件");
    let truncated_len = data.len().saturating_sub(5);
    data.truncate(truncated_len);
    std::fs::write(&wal_path, &data).expect("写回截断数据");

    // 重放应该安全跳过末尾损坏记录，且不 panic，完整恢复第一条
    let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("容错重放成功");
    assert_eq!(restored.total_entries(), 1);
    let cands = restored.lookup_user_words("wan'zheng", 1_700_000_000);
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].text, "完整词");
    assert!(restored
        .lookup_user_words("jie'duan", 1_700_000_001)
        .is_empty());

    let _ = std::fs::remove_file(wal_path);
}

#[test]
fn test_save_and_load_bincode_with_limit() {
    let temp_dir = std::env::temp_dir();
    let bin_path = temp_dir.join("test_user_adaptive_bincode_limit.bin");

    let mut model = UserAdaptiveModel::new();
    model.record_commit("微澜输入法", "wei'lan'shu'ru'fa", 1_700_000_500);
    model.save_to_file(&bin_path).expect("保存用户词库成功");

    let loaded = UserAdaptiveModel::load_from_file(&bin_path).expect("带上限反序列化加载成功");
    assert_eq!(loaded.total_entries(), 1);
    let words = loaded.lookup_user_words("wei'lan'shu'ru'fa", 1_700_000_600);
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "微澜输入法");

    // 伪造声明包含 u64::MAX 个元素的损坏 bincode 头部，验证不会触发 OOM 且安全返回错误
    std::fs::write(&bin_path, u64::MAX.to_le_bytes()).expect("写入损坏头部");
    assert!(UserAdaptiveModel::load_from_file(&bin_path).is_err());

    let _ = std::fs::remove_file(bin_path);
}

#[test]
fn test_cached_max_timestamp_o1() {
    let mut model = UserAdaptiveModel::new();
    assert_eq!(model.max_timestamp(), 0);
    assert_eq!(model.latest_timestamp(), 1_700_000_000);

    model.record_commit("词条一", "ci'tiao'yi", 1_700_000_100);
    assert_eq!(model.max_timestamp(), 1_700_000_100);
    assert_eq!(model.latest_timestamp(), 1_700_000_100);

    // 更早的时间戳不会冲淡最大时间戳
    model.record_commit("词条二", "ci'tiao'er", 1_700_000_050);
    assert_eq!(model.max_timestamp(), 1_700_000_100);

    // 更晚的时间戳会更新最大时间戳
    model.record_commit("词条三", "ci'tiao'san", 1_700_000_300);
    assert_eq!(model.max_timestamp(), 1_700_000_300);
    assert_eq!(model.latest_timestamp(), 1_700_000_300);

    model.clear();
    assert_eq!(model.max_timestamp(), 0);
    assert_eq!(model.latest_timestamp(), 1_700_000_000);
}
