use super::super::*;
use waylan_core::InputMode;
use waylan_engine::AlgoConfig;

#[test]
fn test_v2_session_initialization() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let session = WaylanV2Session::new(engine);

    assert!(!session.focus.is_active);
    assert_eq!(session.runtime.core.input_mode(), InputMode::Chinese);
    assert_eq!(session.protocol.done_serial, 0);
    assert!(session.protocol.keyboard_grab.is_none());
    assert!(session.protocol.input_method.is_none());

    let box_state = session.build_candidate_box_state();
    assert!(!box_state.visible);
    assert_eq!(box_state.mode_badge, "");
}

#[test]
fn test_v2_session_preedit_and_cancel() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.focus.is_active = true;

    session.runtime.core.engine.push_char('n');
    session.runtime.core.engine.push_char('i');

    let box_state = session.build_candidate_box_state();
    assert!(box_state.visible);
    assert!(!box_state.candidates.is_empty());
    assert_eq!(box_state.candidates[0].text, "你");

    session.cancel_input();
    let box_state_after = session.build_candidate_box_state();
    assert!(!box_state_after.visible);
    assert!(session.runtime.core.engine.raw_input().is_empty());
}

#[test]
fn test_formatted_cursor_byte_offset_sync() {
    let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    for ch in "women".chars() {
        engine.push_char(ch);
    }
    assert_eq!(engine.formatted_preedit(), "wo'men");
    // 光标在末尾："wo'men|" -> 6
    assert_eq!(engine.formatted_cursor_byte_offset(), 6);

    // 左移 3 次后位于 'o' 之后："wo|'men" -> 2
    engine.move_cursor_left();
    engine.move_cursor_left();
    engine.move_cursor_left();
    assert_eq!(engine.formatted_cursor_byte_offset(), 2);

    // 再右移 1 次位于 'm' 之后："wo'm|en" -> 4
    engine.move_cursor_right();
    assert_eq!(engine.formatted_cursor_byte_offset(), 4);
}

#[test]
fn test_watched_paths_mtime_triggers_reload() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let temp_file = dir.path().join("waylan_test_mtime_watch.toml");
    let _ = std::fs::write(&temp_file, "page_size = 7\n");

    session.watcher.watched_paths = vec![temp_file.clone()];
    session.watcher.reload_callback = Some(Box::new(|s| {
        s.runtime.core.engine.config.page_size = 9;
    }));

    // 首次同步仅记录基准时间戳，不触发重载
    session.sync_external_state();
    assert_eq!(session.runtime.core.engine.config.page_size, 7);

    // 模拟外部进程将历史签名退后，使得当前文件签名更新于基准签名
    session.watcher.last_watched_mtime = Some(waylan_sys::FileSignature::SingleFile(
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    ));
    session.sync_external_state();
    assert_eq!(
        session.runtime.core.engine.config.page_size, 9,
        "检测到被监控文件状态签名变迁后应自动触发 reload_callback"
    );
}

#[test]
fn test_calculate_delete_range_utf8_and_selection() {
    // 1. 无 surrounding_text 时的默认兜底行为
    assert_eq!(calculate_delete_backward_range(None), (1, 0));
    assert_eq!(calculate_delete_forward_range(None), (0, 1));

    // 2. 正常 ASCII 字符删除
    let ascii_env = ("hello".to_string(), 5, 5);
    assert_eq!(calculate_delete_backward_range(Some(&ascii_env)), (1, 0));
    let ascii_mid = ("hello".to_string(), 2, 2);
    assert_eq!(calculate_delete_forward_range(Some(&ascii_mid)), (0, 1));

    // 3. 中文多字节 UTF-8 字符（每个汉字 3 字节）
    // "你好世界"：每个字 3 字节，共 12 字节
    let zh_env = ("你好世界".to_string(), 6, 6); // 光标在 "好" 后面 (6 字节)
    assert_eq!(
        calculate_delete_backward_range(Some(&zh_env)),
        (3, 0),
        "向后删除中文字符应准确删除 3 字节"
    );
    assert_eq!(
        calculate_delete_forward_range(Some(&zh_env)),
        (0, 3),
        "向前删除中文字符应准确删除 3 字节"
    );

    // 4. 异常非 UTF-8 字符边界传入（如 cursor 落在多字节汉字中间第 4 字节）：绝对不应 Panic
    let invalid_boundary_env = ("你好世界".to_string(), 4, 4);
    let (del_b, _) = calculate_delete_backward_range(Some(&invalid_boundary_env));
    assert!(del_b > 0, "非字符边界应安全回退而不触发切片 panic");

    // 5. 选区整块删除：cursor > anchor (从左往右拖动选区，选中 "你好" 6 字节)
    let sel_right = ("你好世界".to_string(), 6, 0);
    assert_eq!(
        calculate_delete_backward_range(Some(&sel_right)),
        (6, 0),
        "选区存在时 Backspace 应删除整个选区 6 字节"
    );
    assert_eq!(
        calculate_delete_forward_range(Some(&sel_right)),
        (6, 0),
        "选区存在时 Delete 亦应删除整个选区 6 字节"
    );

    // 6. 选区整块删除：cursor < anchor (从右往左反向拖选，选中 "你好" 6 字节)
    let sel_left = ("你好世界".to_string(), 0, 6);
    assert_eq!(
        calculate_delete_backward_range(Some(&sel_left)),
        (0, 6),
        "反向选区存在时 Backspace 应删除光标后方的整个选区 6 字节"
    );
    assert_eq!(
        calculate_delete_forward_range(Some(&sel_left)),
        (0, 6),
        "反向选区存在时 Delete 应删除光标后方的整个选区 6 字节"
    );
}

#[test]
fn test_v2_double_buffered_done_atomic_commit() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 挂起 Activate 与 SurroundingText，但在 Done 之前不应提前修改生效状态
    session.stage_activate();
    session.stage_surrounding_text("测试文本".to_string(), 6, 6);
    assert!(!session.focus.is_active);
    assert!(session.focus.surrounding_text.is_none());
    assert_eq!(session.protocol.done_serial, 0);

    // 触发 Done 原子生效
    assert!(session.apply_done_state());
    assert!(session.focus.is_active);
    assert_eq!(session.protocol.done_serial, 1);
    assert_eq!(
        session.focus.surrounding_text,
        Some(("测试文本".to_string(), 6, 6))
    );

    // 模拟输入拼音与残留引号状态后，在同一批次内收到 Deactivate + Activate 焦点切换
    session.runtime.core.engine.push_char('n');
    session
        .runtime
        .core
        .punctuation_processor_mut()
        .double_quote_open = true;
    session.stage_deactivate();
    session.stage_activate();
    assert!(session.apply_done_state());
    assert!(session.focus.is_active);
    assert_eq!(session.protocol.done_serial, 2);
    assert!(session.runtime.core.engine.raw_input().is_empty());
    assert!(
        !session
            .runtime
            .core
            .punctuation_processor()
            .double_quote_open
    );
    assert!(session.focus.surrounding_text.is_none());
}
