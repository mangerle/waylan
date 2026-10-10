use super::*;
use crate::session_v1::{KEY_1, KEY_4, KEY_9, KEY_APOSTROPHE, KEY_SLASH};
use waylan_algo::types::AlgoConfig;

#[test]
fn test_v2_session_initialization() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let session = WaylanV2Session::new(engine);

    assert!(!session.is_active);
    assert_eq!(session.mode, InputMode::Chinese);
    assert_eq!(session.done_serial, 0);
    assert!(session.keyboard_grab.is_none());
    assert!(session.input_method.is_none());

    let box_state = session.build_candidate_box_state();
    assert!(!box_state.visible);
    assert_eq!(box_state.mode_badge, "中");
}

#[test]
fn test_v2_session_preedit_and_cancel() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.is_active = true;

    session.engine.push_char('n');
    session.engine.push_char('i');

    let box_state = session.build_candidate_box_state();
    assert!(box_state.visible);
    assert!(!box_state.candidates.is_empty());
    assert_eq!(box_state.candidates[0].text, "你");

    session.cancel_input();
    let box_state_after = session.build_candidate_box_state();
    assert!(!box_state_after.visible);
    assert!(session.engine.raw_input().is_empty());
}

#[test]
fn test_shift_punctuation_and_modifier_tracking() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    assert_eq!(keycode_to_punctuation_with_shift(KEY_1, true), Some('!'));
    assert_eq!(keycode_to_punctuation_with_shift(KEY_4, true), Some('$'));
    assert_eq!(keycode_to_punctuation_with_shift(KEY_9, true), Some('('));
    assert_eq!(
        keycode_to_punctuation_with_shift(KEY_APOSTROPHE, true),
        Some('"')
    );
    assert_eq!(
        keycode_to_punctuation_with_shift(KEY_SLASH, true),
        Some('?')
    );
    assert_eq!(
        keycode_to_punctuation_with_shift(KEY_SLASH, false),
        Some('/')
    );

    session.update_modifier_by_key(KEY_LEFTSHIFT, true);
    assert!(session.is_shift_active());
    assert!(!session.has_ctrl_or_alt());
    session.update_modifier_by_key(KEY_LEFTSHIFT, false);
    assert!(!session.is_shift_active());

    session.update_modifier_by_key(KEY_LEFTCTRL, true);
    assert!(session.has_ctrl_or_alt());
    session.update_modifier_by_key(KEY_LEFTCTRL, false);
    assert!(!session.has_ctrl_or_alt());
}

#[test]
fn test_candidate_selection_navigation() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.is_active = true;

    for ch in "xian".chars() {
        session.engine.push_char(ch);
    }
    assert!(session.engine.candidates().len() >= 2);

    assert_eq!(session.build_candidate_box_state().selected_index, 0);
    session.engine.next_candidate();
    assert_eq!(session.build_candidate_box_state().selected_index, 1);
    session.engine.prev_candidate();
    assert_eq!(session.build_candidate_box_state().selected_index, 0);
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
fn test_v2_english_mode_and_empty_buffer_commits() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 1. 中文模式空缓冲区：数字键 1、空格键、回车键与 Tab 键应直接提交上屏
    assert_eq!(session.handle_key_event(KEY_1, true), Some("1".to_string()));
    session.handle_key_event(KEY_1, false);
    assert_eq!(
        session.handle_key_event(KEY_SPACE, true),
        Some(" ".to_string())
    );
    session.handle_key_event(KEY_SPACE, false);
    assert_eq!(
        session.handle_key_event(KEY_ENTER, true),
        Some("\n".to_string())
    );
    session.handle_key_event(KEY_ENTER, false);
    assert_eq!(
        session.handle_key_event(KEY_TAB, true),
        Some("\t".to_string())
    );
    session.handle_key_event(KEY_TAB, false);

    // 空缓冲区下退格键触发 delete_backward（返回 None，不向预编辑输入）
    assert_eq!(session.handle_key_event(KEY_BACKSPACE, true), None);
    session.handle_key_event(KEY_BACKSPACE, false);

    // 2. 切换至英文模式：普通字母键 a (keycode 30)、Shift+a、数字、标点、回车与 Tab 直接以英文上屏
    session.mode = InputMode::English;
    assert_eq!(session.handle_key_event(30, true), Some("a".to_string()));
    session.handle_key_event(30, false);
    assert_eq!(
        session.handle_key_event(KEY_ENTER, true),
        Some("\n".to_string())
    );
    session.handle_key_event(KEY_ENTER, false);
    assert_eq!(
        session.handle_key_event(KEY_TAB, true),
        Some("\t".to_string())
    );
    session.handle_key_event(KEY_TAB, false);
    assert_eq!(session.handle_key_event(KEY_BACKSPACE, true), None);
    session.handle_key_event(KEY_BACKSPACE, false);

    session.handle_key_event(KEY_LEFTSHIFT, true);
    assert_eq!(session.handle_key_event(30, true), Some("A".to_string()));
    session.handle_key_event(30, false);
    assert_eq!(session.handle_key_event(KEY_1, true), Some("!".to_string()));
    session.handle_key_event(KEY_1, false);
    session.handle_key_event(KEY_LEFTSHIFT, false);
}

#[test]
fn test_watched_paths_mtime_triggers_reload() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    let temp_file = std::env::temp_dir().join("waylan_test_mtime_watch.toml");
    let _ = std::fs::write(&temp_file, "page_size = 7\n");

    session.watched_paths = vec![temp_file.clone()];
    session.reload_callback = Some(Box::new(|s| {
        s.engine.config.page_size = 9;
    }));

    // 首次同步仅记录基准时间戳，不触发重载
    session.sync_external_state();
    assert_eq!(session.engine.config.page_size, 7);

    // 模拟外部进程将历史时间戳退后，使得当前文件时间戳更新于基准时间戳
    session.last_watched_mtime =
        Some(std::time::SystemTime::now() - std::time::Duration::from_secs(60));
    session.sync_external_state();
    assert_eq!(
        session.engine.config.page_size, 9,
        "检测到被监控文件修改时间更新后应自动触发 reload_callback"
    );

    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn test_shift_toggle_commits_raw_preedit() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 输入拼音 "ni"
    session.handle_key_event(49, true); // 'n'
    session.handle_key_event(49, false);
    session.handle_key_event(23, true); // 'i'
    session.handle_key_event(23, false);
    assert_eq!(session.engine.raw_input(), "ni");

    // 单击 Shift 键切换中英文
    session.handle_key_event(KEY_LEFTSHIFT, true);
    let commit = session.handle_key_event(KEY_LEFTSHIFT, false);

    assert_eq!(session.mode, InputMode::English);
    assert_eq!(commit, Some("ni".to_string()));
    assert!(session.engine.raw_input().is_empty());
    assert!(!session.build_candidate_box_state().visible);
}

#[test]
fn test_v2_unconsumed_key_and_shortcuts() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 1. 中文模式空缓冲区下，普通方向键（如 KEY_LEFT）未被输入法消费，应透传席位
    let (consumed, text) = session.process_key_event(KEY_LEFT, true);
    assert!(!consumed);
    assert_eq!(text, None);
    let (consumed_up, _) = session.process_key_event(KEY_LEFT, false);
    assert!(!consumed_up);

    // 2. 模拟按下 Ctrl+C 组合快捷键
    session.process_key_event(KEY_LEFTCTRL, true);
    assert!(session.has_ctrl_or_alt());

    // 按下 C 键 (keycode 46)，由于带有 Ctrl 修饰键，输入法不拦截消费
    let (consumed_c, text_c) = session.process_key_event(46, true);
    assert!(!consumed_c, "Ctrl+C 应透传给应用程序而非被输入法拦截吞掉");
    assert_eq!(text_c, None);

    // 释放 C 键与释放 Ctrl 键
    let (consumed_c_up, _) = session.process_key_event(46, false);
    assert!(!consumed_c_up);
    session.process_key_event(KEY_LEFTCTRL, false);
    assert!(!session.has_ctrl_or_alt());

    // 3. 正常输入拼音字符时，应被输入法消费
    let (consumed_n, text_n) = session.process_key_event(49, true); // 'n'
    assert!(consumed_n);
    assert_eq!(text_n, None);
    assert_eq!(session.engine.raw_input(), "n");
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
