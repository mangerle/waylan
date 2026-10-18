use super::*;
use crate::session_v1::{KEY_1, KEY_4, KEY_9, KEY_APOSTROPHE, KEY_SLASH};
use crate::session_v2::keys::process_key_event;
use waylan_engine::AlgoConfig;

#[test]
fn test_v2_session_initialization() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let session = WaylanV2Session::new(engine);

    assert!(!session.focus.is_active);
    assert_eq!(session.runtime.core.mode, InputMode::Chinese);
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

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTSHIFT, true);
    assert!(session.runtime.core.is_shift_active());
    assert!(!session.runtime.core.has_ctrl_or_alt());
    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTSHIFT, false);
    assert!(!session.runtime.core.is_shift_active());

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTCTRL, true);
    assert!(session.runtime.core.has_ctrl_or_alt());
    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTCTRL, false);
    assert!(!session.runtime.core.has_ctrl_or_alt());
}

#[test]
fn test_candidate_selection_navigation() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.focus.is_active = true;

    for ch in "xian".chars() {
        session.runtime.core.engine.push_char(ch);
    }
    assert!(session.runtime.core.engine.candidates().len() >= 2);

    assert_eq!(session.build_candidate_box_state().selected_index, 0);
    session.runtime.core.engine.next_candidate();
    assert_eq!(session.build_candidate_box_state().selected_index, 1);
    session.runtime.core.engine.prev_candidate();
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
fn test_v2_english_mode_and_empty_buffer_passthrough() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 1. 中文模式空缓冲区：回车键、Tab 键、空格键、退格键与数字键应原生透传 (返回 (false, None))
    let (consumed_1, _) = session.process_key_event(KEY_1, true);
    assert!(!consumed_1, "空闲状态数字键应保持透传");
    let (consumed_space, _) = session.process_key_event(KEY_SPACE, true);
    assert!(!consumed_space, "空闲状态空格键应保持透传");
    let (consumed_enter, _) = session.process_key_event(KEY_ENTER, true);
    assert!(!consumed_enter, "空闲状态回车键应保持透传");
    let (consumed_tab, _) = session.process_key_event(KEY_TAB, true);
    assert!(!consumed_tab, "空闲状态 Tab 键应保持透传");
    let (consumed_bs, _) = session.process_key_event(KEY_BACKSPACE, true);
    assert!(!consumed_bs, "空闲状态退格键应保持透传");

    // 中文标点（逗号）在空闲状态下应被正确拦截并提交中文标点
    let (consumed_comma, commit_comma) = session.process_key_event(KEY_COMMA, true);
    assert!(consumed_comma, "中文标点逗号应被输入法消费");
    assert_eq!(commit_comma, Some("，".to_string()));

    // 2. 切换至英文模式：普通字母、回车、Tab、退格等全部原生透传给席位
    session.runtime.core.mode = InputMode::English;
    let (consumed_a, _) = session.process_key_event(30, true);
    assert!(!consumed_a, "英文模式普通字母应原生透传");
    let (consumed_en_enter, _) = session.process_key_event(KEY_ENTER, true);
    assert!(!consumed_en_enter, "英文模式回车键应原生透传");
    let (consumed_en_bs, _) = session.process_key_event(KEY_BACKSPACE, true);
    assert!(!consumed_en_bs, "英文模式退格键应原生透传");
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
    session.watcher.last_watched_mtime = Some(waylan_core::FileSignature::SingleFile(
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    ));
    session.sync_external_state();
    assert_eq!(
        session.runtime.core.engine.config.page_size, 9,
        "检测到被监控文件状态签名变迁后应自动触发 reload_callback"
    );
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
    assert_eq!(session.runtime.core.engine.raw_input(), "ni");

    // 单击 Shift 键切换中英文
    session.handle_key_event(KEY_LEFTSHIFT, true);
    let commit = session.handle_key_event(KEY_LEFTSHIFT, false);

    assert_eq!(session.runtime.core.mode, InputMode::English);
    assert_eq!(commit, Some("ni".to_string()));
    assert!(session.runtime.core.engine.raw_input().is_empty());
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
    assert!(session.runtime.core.has_ctrl_or_alt());

    // 按下 C 键 (keycode 46)，由于带有 Ctrl 修饰键，输入法不拦截消费
    let (consumed_c, text_c) = session.process_key_event(46, true);
    assert!(!consumed_c, "Ctrl+C 应透传给应用程序而非被输入法拦截吞掉");
    assert_eq!(text_c, None);

    // 释放 C 键与释放 Ctrl 键
    let (consumed_c_up, _) = session.process_key_event(46, false);
    assert!(!consumed_c_up);
    session.process_key_event(KEY_LEFTCTRL, false);
    assert!(!session.runtime.core.has_ctrl_or_alt());

    // 3. 正常输入拼音字符时，应被输入法消费
    let (consumed_n, text_n) = session.process_key_event(49, true); // 'n'
    assert!(consumed_n);
    assert_eq!(text_n, None);
    assert_eq!(session.runtime.core.engine.raw_input(), "n");
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
    session.runtime.core.punctuation_processor.double_quote_open = true;
    session.stage_deactivate();
    session.stage_activate();
    assert!(session.apply_done_state());
    assert!(session.focus.is_active);
    assert_eq!(session.protocol.done_serial, 2);
    assert!(session.runtime.core.engine.raw_input().is_empty());
    assert!(!session.runtime.core.punctuation_processor.double_quote_open);
    assert!(session.focus.surrounding_text.is_none());
}

#[test]
fn test_key_press_release_symmetry() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);

    // 1. 输入 "ni" 并按空格上屏清空预编辑，随后释放空格键：空格键释放也必须被消费而不泄漏给客户端
    assert!(session.process_key_event(49, true).0); // 'n' press
    assert!(session.process_key_event(49, false).0); // 'n' release
    assert!(session.process_key_event(23, true).0); // 'i' press
    assert!(session.process_key_event(23, false).0); // 'i' release

    let (space_press_consumed, committed) = session.process_key_event(KEY_SPACE, true);
    assert!(space_press_consumed);
    assert_eq!(committed, Some("你".to_string()));
    assert!(session.runtime.core.engine.raw_input().is_empty());

    let (space_release_consumed, _) = session.process_key_event(KEY_SPACE, false);
    assert!(
        space_release_consumed,
        "空格按下完成选词清空预编辑后，其对应的松开事件仍应被消费而非透传给客户端"
    );

    // 2. Shift 键按下时未消费（透传给席位），松开触发中英切换时也应返回未消费以释放席位 Shift 状态
    let (shift_press_consumed, _) = session.process_key_event(KEY_LEFTSHIFT, true);
    assert!(!shift_press_consumed);
    let (shift_release_consumed, _) = session.process_key_event(KEY_LEFTSHIFT, false);
    assert!(
        !shift_release_consumed,
        "Shift 按下透传给客户端后，松开事件也必须透传以防客户端 Shift 卡死"
    );
    assert_eq!(session.runtime.core.mode, InputMode::English);
}

#[test]
fn test_v2_grid_expansion_state() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.focus.is_active = true;

    session.runtime.core.engine.push_char('n');
    session.runtime.core.engine.push_char('i');

    let initial_box = session.build_candidate_box_state();
    assert!(!initial_box.is_grid_expanded());
    assert!(initial_box.grid_candidates.is_empty());

    // 展开 5 行大网格大候选词面板
    session.runtime.core.expand_grid();
    let expanded_box = session.build_candidate_box_state();
    assert!(expanded_box.is_grid_expanded());
    assert!(!expanded_box.grid_candidates.is_empty());
    assert!(expanded_box.grid_candidates.len() <= 30);

    // 取消输入时自动收回展开态
    session.cancel_input();
    assert!(!session.is_grid_expanded());
}

#[test]
fn test_v2_grid_expanded_key_navigation_and_selection() {
    let mut table = waylan_engine::algo::table::LexiconTable::with_sample_data();
    for i in 0..20 {
        table.insert("ni", format!("字{}", i), 10_000 - i * 10);
    }
    let mut session = WaylanV2Session::new(AlgoEngine::new(table, AlgoConfig::default()));
    session.runtime.core.engine.push_char('n');
    session.runtime.core.engine.push_char('i');

    // 展开 5 行网格面板
    session.runtime.core.expand_grid();
    session.runtime.core.set_grid_selected_index(0);

    // 1. 按 Down 键：向下移动一行（从第 1 行跳至第 2 行第 1 个词，索引 6）
    let (consumed, committed) = process_key_event(&mut session, KEY_DOWN, true);
    assert!(consumed);
    assert!(committed.is_none());
    assert_eq!(session.grid_selected_index(), 6);

    // 2. 按 Right 键：向右移动一个词（索引 7）
    let (consumed, _) = process_key_event(&mut session, KEY_RIGHT, true);
    assert!(consumed);
    assert_eq!(session.grid_selected_index(), 7);

    // 3. 当前处于第 2 行，按数字 2 键：选取第 2 行第 2 个词（索引 1 * 6 + (2 - 1) = 7）
    let all_cands = session.runtime.core.engine.all_candidates();
    let expected_text = all_cands[7].text.clone();
    let (consumed, committed) = process_key_event(&mut session, KEY_2, true);
    assert!(consumed);
    assert_eq!(committed, Some(expected_text));
    assert!(!session.is_grid_expanded(), "选词后应自动折叠大网格");
    assert_eq!(session.grid_selected_index(), 0);

    // 4. 测试空格选词
    session.runtime.core.engine.push_char('n');
    session.runtime.core.engine.push_char('i');
    session.runtime.core.expand_grid();
    session.runtime.core.set_grid_selected_index(12); // 第 3 行第 1 个词
    let expected_text_3 = session.runtime.core.engine.all_candidates()[12]
        .text
        .clone();
    let (consumed, committed) = process_key_event(&mut session, KEY_SPACE, true);
    assert!(consumed);
    assert_eq!(committed, Some(expected_text_3));
    assert!(!session.is_grid_expanded());

    // 5. 测试 ESC 折叠
    session.runtime.core.engine.push_char('n');
    session.runtime.core.engine.push_char('i');
    session.runtime.core.expand_grid();
    let (consumed, committed) = process_key_event(&mut session, KEY_ESC, true);
    assert!(consumed);
    assert!(committed.is_none());
    assert!(!session.is_grid_expanded(), "ESC 应折叠大网格");
    assert!(
        !session.runtime.core.engine.raw_input().is_empty(),
        "单次 ESC 仅折叠大网格，不清除输入"
    );
}

#[test]
fn test_caps_lock_toggle_and_passthrough() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV2Session::new(engine);
    session.focus.is_active = true;
    assert_eq!(session.runtime.core.mode, InputMode::Chinese);
    assert!(!session.runtime.core.is_caps_locked());

    // 1. 中文状态下输入拼音字母
    let (c1, _) = session.process_key_event(49, true); // 'n'
    assert!(c1);
    let (c2, _) = session.process_key_event(23, true); // 'i'
    assert!(c2);
    assert_eq!(session.runtime.core.engine.raw_input(), "ni");

    // 2. 拼音输入中途按下 CapsLock：应立即提交拼音原始字母上屏，CapsLock 本身不被消费（透传给合成器）
    let (caps_consumed, committed) = session.process_key_event(KEY_CAPSLOCK, true);
    assert!(!caps_consumed, "CapsLock 键本身必须透传给合成器更新锁定位");
    assert_eq!(committed, Some("ni".to_string()), "未完成拼音应直接上屏");
    assert!(
        session.runtime.core.engine.raw_input().is_empty(),
        "引擎拼音输入应被清空"
    );

    let (caps_up_consumed, _) = session.process_key_event(KEY_CAPSLOCK, false);
    assert!(!caps_up_consumed);

    // 3. 模拟合成器下发修饰键锁定状态（mods_locked 包含 MOD_CAPS）
    session.runtime.core.modifiers.locked = MOD_CAPS;
    assert!(session.runtime.core.is_caps_locked());

    // 大写锁定期间，敲击字母键（如 A 键，扫描码 30），输入法不拦截消费，直通焦点应用
    let (a_consumed, _) = session.process_key_event(30, true);
    assert!(!a_consumed, "CapsLock 激活期间字母键应原生透传以输出大写");
    assert!(session.runtime.core.engine.raw_input().is_empty());

    // 4. 解除大写锁定（合成器更新 mods_locked = 0）
    session.runtime.core.modifiers.locked = 0;
    assert!(!session.runtime.core.is_caps_locked());
    assert_eq!(
        session.runtime.core.mode,
        InputMode::Chinese,
        "解除大写锁定后应恢复中文模式"
    );

    // 恢复中文模式后，再次敲击字母键应正常进入拼音引擎
    let (n_consumed, _) = session.process_key_event(49, true);
    assert!(n_consumed);
    assert_eq!(session.runtime.core.engine.raw_input(), "n");

    // 5. 若原来是英文模式：解除大写锁定后应保持英文模式
    session.runtime.core.mode = InputMode::English;
    session.runtime.core.modifiers.locked = MOD_CAPS;
    assert!(session.runtime.core.is_caps_locked());
    session.runtime.core.modifiers.locked = 0;
    assert!(!session.runtime.core.is_caps_locked());
    assert_eq!(
        session.runtime.core.mode,
        InputMode::English,
        "原本是英文模式时解除大写锁定仍应为英文模式"
    );
}
