use super::*;
use crate::user_adaptive::UserDictAppendParams;

#[test]
fn test_record_and_boost() {
    let mut model = UserAdaptiveModel::new();
    let now = 1_700_000_000;
    model.record_commit("先", "xian", now);
    model.record_commit("先", "xian", now + 10);

    let mut candidate = AlgoCandidate::new("先", "xian", 7_200);
    model.apply_boost(&mut candidate, now + 100);

    assert!(candidate.score > 100_000);
    assert_eq!(
        candidate.comment, None,
        "用户高频自适应提权时严禁附加冗余的‘常用习惯’文本"
    );
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

    // 对 UserHistory 层级调用 apply_boost 不应重复加分
    model.apply_boost(&mut user_cand, now + 10);
    assert_eq!(user_cand.score, original_score);
    assert_eq!(user_cand.comment.as_deref(), None);
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
    let mut wal = UserAdaptiveWal::new();
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "微澜",
            pinyin: "wei'lan",
            timestamp: 1_700_000_000,
        },
    )
    .expect("写入第1条WAL");
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "微澜",
            pinyin: "wei'lan",
            timestamp: 1_700_000_100,
        },
    )
    .expect("写入第2条WAL");
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "输入法",
            pinyin: "shu'ru'fa",
            timestamp: 1_700_000_200,
        },
    )
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
    let mut wal = UserAdaptiveWal::new();
    // 设置较小阈值为 5，用于测试自动触发
    wal.set_compact_threshold(5);

    for i in 0..4 {
        wal.append(
            &mut model,
            &UserDictAppendParams {
                path: &wal_path,
                text: "测试",
                pinyin: "ce'shi",
                timestamp: 1_700_000_000 + i,
            },
        )
        .expect("追加写入");
    }
    assert_eq!(wal.wal_append_counter(), 4);

    // 第 5 次追加达到阈值，但不会在追加写热路径中同步触发阻塞式压缩，计数器保持递增
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "测试",
            pinyin: "ce'shi",
            timestamp: 1_700_000_010,
        },
    )
    .expect("追加写入");
    assert_eq!(wal.wal_append_counter(), 5);

    // 在外部安全窗口（如会话失焦）调用 maybe_compact 成功执行条件压缩并清零计数器
    assert!(wal.maybe_compact(&model, &wal_path).expect("条件压缩"));
    assert_eq!(wal.wal_append_counter(), 0);

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
    let mut wal = UserAdaptiveWal::new();
    wal.set_compact_threshold(100_000); // 运行期不触发

    // 写入 60 条同一个词的流水（高冗余率：60 流水 vs 1 个词）
    for i in 0..60 {
        wal.append(
            &mut model,
            &UserDictAppendParams {
                path: &wal_path,
                text: "高频词",
                pinyin: "gao'pin'ci",
                timestamp: 1_700_000_000 + i,
            },
        )
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
    let mut wal = UserAdaptiveWal::new();
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "完整词",
            pinyin: "wan'zheng",
            timestamp: 1_700_000_000,
        },
    )
    .expect("写入正常第一条");
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "被截断词",
            pinyin: "jie'duan",
            timestamp: 1_700_000_001,
        },
    )
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
    assert!(
        restored
            .lookup_user_words("jie'duan", 1_700_000_001)
            .is_empty()
    );

    let _ = std::fs::remove_file(wal_path);
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

#[test]
fn test_wal_append_reopens_on_inode_change() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join(format!("test_wal_inode_{}.wal", nanos));
    let temp_new_path = temp_dir.join(format!("test_wal_inode_new_{}.wal", nanos));

    let mut model = UserAdaptiveModel::new();
    let mut wal = UserAdaptiveWal::new();
    // 第一次写入，建立常驻句柄
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "中国",
            pinyin: "zhong'guo",
            timestamp: 1_700_000_000,
        },
    )
    .expect("首次追加写入成功");

    let mut new_model = UserAdaptiveModel::new();
    let mut new_wal = UserAdaptiveWal::new();
    new_wal
        .append(
            &mut new_model,
            &UserDictAppendParams {
                path: &temp_new_path,
                text: "北京",
                pinyin: "bei'jing",
                timestamp: 1_700_000_010,
            },
        )
        .expect("向临时文件写入成功");
    std::fs::rename(&temp_new_path, &wal_path).expect("原子替换原文件");

    // 原 wal 再次写入，应当感知到 Inode 改变并重新打开新文件，而不是继续写向旧文件的孤儿句柄
    wal.append(
        &mut model,
        &UserDictAppendParams {
            path: &wal_path,
            text: "上海",
            pinyin: "shang'hai",
            timestamp: 1_700_000_020,
        },
    )
    .expect("替换后追加写入成功");

    // 重放磁盘文件，应同时包含“北京”与“上海”
    let replayed = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放新文件成功");
    assert_eq!(replayed.total_entries(), 2);
    let words = replayed.lookup_user_words("shang'hai", 1_700_000_030);
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "上海");
    let _ = std::fs::remove_file(wal_path);
}

#[test]
fn test_user_pairs_decay_wal_replay_and_forget() {
    let mut wal_path = std::env::temp_dir();
    wal_path.push("waylan_test_user_pairs_wal.bin");
    let _ = std::fs::remove_file(&wal_path);

    let mut model = UserAdaptiveModel::new();
    let mut wal = UserAdaptiveWal::new();
    let t0 = 1_700_000_000u64;

    wal.append_pair(&mut model, &wal_path, "开发", "高效", t0)
        .expect("追加词对 1 成功");
    wal.append_pair(&mut model, &wal_path, "开发", "高效", t0 + 1)
        .expect("追加词对 2 成功");

    let boost_now = model.pair_boost_score("开发", "高效", t0 + 1);
    assert!(boost_now > 0, "刚记录的词对应当产生正加分");

    // 30 天（2_592_000 秒）半衰期后分值应明显衰减但仍大于 0
    let boost_30d = model.pair_boost_score("开发", "高效", t0 + 1 + 2_592_000);
    assert!(boost_30d > 0 && boost_30d < boost_now);

    // 忘词后应立即清除该词对加分并持久化到 WAL
    wal.append_forget(&mut model, &wal_path, "高效", None, t0 + 10)
        .expect("追加忘词记录成功");
    assert_eq!(model.pair_boost_score("开发", "高效", t0 + 10), 0);

    // 再次写入一个词对并重放 WAL 验证持久化一致
    wal.append_pair(&mut model, &wal_path, "编译", "通过", t0 + 20)
        .expect("追加词对 3 成功");
    let replayed = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放 WAL 成功");
    assert_eq!(replayed.pair_boost_score("开发", "高效", t0 + 30), 0);
    assert!(replayed.pair_boost_score("编译", "通过", t0 + 30) > 0);

    let _ = std::fs::remove_file(&wal_path);
}
