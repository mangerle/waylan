//! 核心会话按键事件分发流水线单元测试

use super::*;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;
use waylan_core::CandidateExpansion;

fn make_test_session() -> ImeCoreSession {
    let engine = AlgoEngine::new(LexiconTable::with_sample_data(), AlgoConfig::default());
    ImeCoreSession::new(engine)
}

#[test]
fn test_ime_core_session_input_and_commit() {
    let mut session = make_test_session();
    // 键入 "ni" (扫描码 49, 23)
    let act1 = session.handle_key_press(49, 1000, None);
    assert_eq!(act1, ImeOutputAction::Handled { update_ui: true });
    let act2 = session.handle_key_press(23, 1001, None);
    assert_eq!(act2, ImeOutputAction::Handled { update_ui: true });
    assert_eq!(session.engine.raw_input(), "ni");

    // 空格确认首选词 "你"
    let act_space = session.handle_key_press(KEY_SPACE, 1002, None);
    assert_eq!(act_space, ImeOutputAction::Commit("你".to_string()));
    assert!(session.engine.raw_input().is_empty());
}

#[test]
fn test_ime_core_session_capslock_commit_raw() {
    let mut session = make_test_session();
    session.handle_key_press(49, 1000, None);
    session.handle_key_press(23, 1001, None);
    assert_eq!(session.engine.raw_input(), "ni");

    // 按下 CapsLock，应立即提交原拼音 "ni"
    let (consumed, committed) = session.handle_capslock(true);
    assert!(!consumed);
    assert_eq!(committed.as_deref(), Some("ni"));
    assert!(session.engine.raw_input().is_empty());
}

#[test]
fn test_ime_core_session_idle_punctuation() {
    let mut session = make_test_session();
    // 空闲状态下按逗号 (KEY_COMMA: 51)
    let act = session.handle_key_press(51, 1000, None);
    assert_eq!(act, ImeOutputAction::Commit("，".to_string()));
}

#[test]
fn test_ime_core_session_shortcuts() {
    let mut session = make_test_session();
    // 模拟按下 Ctrl + Shift
    session.set_modifiers(MOD_CTRL | MOD_SHIFT, 0);

    // 按 F 键触发简繁切换 (默认配置为 CtrlShiftF)
    let act = session.handle_key_press(KEY_F, 1000, None);
    assert_eq!(
        act,
        ImeOutputAction::ShortcutToggled(ShortcutToggleAction::Traditional(true))
    );
    assert!(session.engine.config.traditional_chinese);
    assert!(session.is_traditional());
    assert!(session.decoration_flags().traditional_chinese);

    // 再次按下反转回简体
    let act2 = session.handle_key_press(KEY_F, 1001, None);
    assert_eq!(
        act2,
        ImeOutputAction::ShortcutToggled(ShortcutToggleAction::Traditional(false))
    );
    assert!(!session.engine.config.traditional_chinese);
    assert!(!session.is_traditional());
    assert!(!session.decoration_flags().traditional_chinese);

    // 模拟单独按住 Ctrl (无 Shift)
    session.set_modifiers(MOD_CTRL, 0);
    // 按句号 . 触发标点全半角切换 (默认 CtrlPeriod)
    let act_punct = session.handle_key_press(KEY_DOT, 1002, None);
    assert_eq!(
        act_punct,
        ImeOutputAction::ShortcutToggled(ShortcutToggleAction::FullWidthPunct(false))
    );
    assert!(!session.punctuation_processor.is_full_width_punct());
    assert!(!session.is_full_width_punct());
    assert!(!session.decoration_flags().full_width_punct);
}

#[test]
fn test_ime_core_session_full_width_char() {
    let mut session = make_test_session();
    session.set_full_width_char(true);
    assert!(session.is_full_width_char());
    assert!(session.decoration_flags().full_width_char);
    session.mode = InputMode::English;

    // 英文模式下键入 'a' (扫描码 30)
    let act = session.handle_key_press(30, 1000, None);
    assert_eq!(act, ImeOutputAction::Commit("ａ".to_string()));
}

#[test]
fn test_ime_core_session_decoration_flags_batch_sync() {
    let mut session = make_test_session();
    let flags = InputDecorationFlags::new(true, false, true);
    session.set_decoration_flags(flags);

    assert_eq!(session.decoration_flags(), flags);
    assert!(session.is_traditional());
    assert!(session.engine.config.traditional_chinese);
    assert!(!session.is_full_width_punct());
    assert!(!session.punctuation_processor.is_full_width_punct());
    assert!(session.is_full_width_char());
}

#[test]
fn test_extract_surrounding_prefix_safety() {
    let text = "你好，世界！abc".to_string();
    let surrounding = (text, 15, 15); // "你好，世界！" 占用 18 字节，15 字节在 "界" 的中间或边界
    let prefix = extract_surrounding_prefix(Some(&surrounding), 64);
    assert!(prefix.is_some());
}

#[test]
fn test_process_key_password_and_lock_bypass() {
    let mut session = make_test_session();
    // 模拟密码框中的按键输入 'a' (按下)
    let res = session.process_key(
        KeyProcessContext {
            keycode: 30,
            is_press: true,
            is_password_or_locked: true,
            surrounding_prefix: None,
        },
        1000,
    );
    assert_eq!(res, ImeOutputAction::PassThrough);
    assert!(session.engine.raw_input().is_empty());

    // 模拟密码框中的按键释放 'a'
    let res_rel = session.process_key(
        KeyProcessContext {
            keycode: 30,
            is_press: false,
            is_password_or_locked: true,
            surrounding_prefix: None,
        },
        1001,
    );
    assert_eq!(res_rel, ImeOutputAction::PassThrough);
}

#[test]
fn test_process_key_capslock_commit_and_passthrough() {
    let mut session = make_test_session();
    // 输入拼音 "ni"
    session.process_key(
        KeyProcessContext {
            keycode: 49,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1000,
    );
    session.process_key(
        KeyProcessContext {
            keycode: 23,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1001,
    );
    assert_eq!(session.engine.raw_input(), "ni");

    // 按下 CapsLock，应返回 CommitAndPassThrough("ni")
    let res_caps = session.process_key(
        KeyProcessContext {
            keycode: KEY_CAPSLOCK,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1002,
    );
    assert_eq!(
        res_caps,
        ImeOutputAction::ModeChanged {
            new_mode: InputMode::English,
            committed: Some("ni".to_string()),
            was_consumed: false,
        }
    );
    assert!(session.engine.raw_input().is_empty());
}

#[test]
fn test_process_key_shift_mode_toggle() {
    let mut session = make_test_session();
    assert_eq!(session.mode, InputMode::Chinese);

    // 按下 LeftShift
    session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1000,
    );

    // 释放 LeftShift (短按未按其他键)，应触发中英切换为英文
    let res = session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: false,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1050,
    );
    assert_eq!(
        res,
        ImeOutputAction::ModeChanged {
            new_mode: InputMode::English,
            committed: None,
            was_consumed: false,
        }
    );
    assert_eq!(session.mode, InputMode::English);
}

#[test]
fn test_ime_core_session_grid_navigation_and_selection() {
    let mut table = LexiconTable::with_sample_data();
    for i in 0..20 {
        table.insert("ni", format!("字{}", i), 10_000 - i * 10);
    }
    let mut session = ImeCoreSession::new(AlgoEngine::new(table, AlgoConfig::default()));
    session.handle_key_press(49, 1000, None); // 'n'
    session.handle_key_press(23, 1001, None); // 'i'

    // 展开大网格面板
    session.expand_grid();
    assert!(session.is_grid_expanded());
    assert_eq!(session.grid_selected_index(), 0);

    // 1. 按 Down 键：向下移动一行 (第 2 行第 1 个词，索引 6)
    let act_down = session.handle_key_press(KEY_DOWN, 1002, None);
    assert_eq!(act_down, ImeOutputAction::Handled { update_ui: true });
    assert_eq!(session.grid_selected_index(), 6);

    // 2. 按 Right 键：向右移动一个词 (索引 7)
    let act_right = session.handle_key_press(KEY_RIGHT, 1003, None);
    assert_eq!(act_right, ImeOutputAction::Handled { update_ui: true });
    assert_eq!(session.grid_selected_index(), 7);

    // 3. 当前处于第 2 行，按数字 2 键：选取第 2 行第 2 个词 (索引 1 * 6 + 1 = 7)
    let all_cands = session.engine.all_candidates();
    let expected_text = all_cands[7].text.clone();
    let act_num = session.handle_key_press(KEY_2, 1004, None);
    assert_eq!(act_num, ImeOutputAction::Commit(expected_text));
    assert!(!session.is_grid_expanded(), "选词后应自动折叠大网格");
    assert_eq!(session.grid_selected_index(), 0);

    // 4. 测试 ESC 折叠大网格
    session.handle_key_press(49, 1005, None);
    session.handle_key_press(23, 1006, None);
    session.expand_grid();
    let act_esc = session.handle_key_press(KEY_ESC, 1007, None);
    assert_eq!(act_esc, ImeOutputAction::Handled { update_ui: true });
    assert!(!session.is_grid_expanded());
    assert!(!session.engine.raw_input().is_empty());
}

#[test]
fn test_ime_core_session_expanded_card_and_reset_context() {
    let mut session = ImeCoreSession::default();
    assert!(!session.is_card_expanded());
    assert!(!session.is_grid_expanded());

    session.expansion = CandidateExpansion::Card { selected_index: 2 };
    assert!(session.is_card_expanded());

    session.expand_grid();
    assert!(session.is_grid_expanded());
    assert!(!session.is_card_expanded());

    session.reset_context();
    assert!(!session.is_card_expanded());
    assert!(!session.is_grid_expanded());
    assert!(session.engine.raw_input().is_empty());
}

#[test]
fn test_shift_digit_selection_toggle() {
    let mut session = make_test_session();
    for ch in "nihao".chars() {
        session.engine.push_char(ch);
    }

    let disabled = handle_composition_key_with_params(
        &mut session,
        CompositionKeyParams {
            key: KEY_1,
            shift_active: true,
            ctrl_active: false,
            page_keys: PageKeyConfig::default(),
            now: 100,
        },
    );
    assert!(
        disabled.is_none(),
        "未开启 Shift 选词时 Shift+数字应保持透传"
    );
    assert_eq!(session.engine.raw_input(), "nihao");

    session.shift_select_second = true;
    let enabled = handle_composition_key_with_params(
        &mut session,
        CompositionKeyParams {
            key: KEY_1,
            shift_active: true,
            ctrl_active: false,
            page_keys: PageKeyConfig::default(),
            now: 101,
        },
    );
    assert!(
        matches!(enabled, Some(ImeOutputAction::Commit(_))),
        "开启 Shift 选词后 Shift+数字应选择对应候选"
    );
}

#[test]
fn test_caps_lock_mode_toggle_and_shift_ignored_during_caps() {
    let mut session = make_test_session();
    assert_eq!(session.input_mode(), InputMode::Chinese);
    assert!(!session.is_caps_locked());

    // 1. 中文状态下敲击 CapsLock：派发 ModeChanged(English)，物理按键透传，内部置为大写锁定
    let caps_press = session.process_key(
        KeyProcessContext {
            keycode: KEY_CAPSLOCK,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1000,
    );
    assert_eq!(
        caps_press,
        ImeOutputAction::ModeChanged {
            new_mode: InputMode::English,
            committed: None,
            was_consumed: false,
        }
    );
    assert!(session.is_caps_locked());

    // 2. 大写锁定下单独敲击 Shift 键：彻底静默吞掉（Handled，不透传给系统），不破坏大写锁定
    let shift_press = session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1001,
    );
    assert_eq!(
        shift_press,
        ImeOutputAction::Handled { update_ui: false },
        "大写锁定下按下 Shift 必须被彻底静默吞掉"
    );

    let shift_rel = session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: false,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1002,
    );
    assert_eq!(
        shift_rel,
        ImeOutputAction::Handled { update_ui: false },
        "大写锁定下松开 Shift 必须被彻底静默吞掉"
    );
    assert!(
        session.is_caps_locked(),
        "大写锁定下敲击 Shift 保持大写锁定不变"
    );
    assert_eq!(session.input_mode(), InputMode::Chinese);

    // 3. 再次敲击 CapsLock 物理键：退出大写锁定并恢复中文模式
    let caps_exit = session.process_key(
        KeyProcessContext {
            keycode: KEY_CAPSLOCK,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1003,
    );
    assert_eq!(
        caps_exit,
        ImeOutputAction::ModeChanged {
            new_mode: InputMode::Chinese,
            committed: None,
            was_consumed: false,
        }
    );
    assert!(!session.is_caps_locked(), "再次敲击 CapsLock 解除大写锁定");
    assert_eq!(session.input_mode(), InputMode::Chinese);
}

#[test]
fn test_caps_lock_shift_chord_preserves_caps_lock() {
    let mut session = make_test_session();
    session.set_caps_lock(true);
    assert!(session.is_caps_locked());

    // 按下 Shift：静默吞没
    let shift_press = session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1000,
    );
    assert_eq!(shift_press, ImeOutputAction::Handled { update_ui: false });

    // 在 Shift 保持按下期间敲击字母 'a'（扫描码 30）：无论按不按 Shift 均严格输出大写 'A'
    let act_a = session.process_key(
        KeyProcessContext {
            keycode: 30,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1001,
    );
    assert_eq!(
        act_a,
        ImeOutputAction::Commit("A".to_string()),
        "大写锁定下无论按不按 Shift 均必须输出大写字母"
    );

    // 松开字母 'a'：已消费
    let rel_a = session.process_key(
        KeyProcessContext {
            keycode: 30,
            is_press: false,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1002,
    );
    assert_eq!(rel_a, ImeOutputAction::Handled { update_ui: false });

    // 松开 Shift：静默吞没
    let shift_rel = session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTSHIFT,
            is_press: false,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1003,
    );
    assert_eq!(
        shift_rel,
        ImeOutputAction::Handled { update_ui: false },
        "松开 Shift 静默吞没"
    );
    assert!(
        session.is_caps_locked(),
        "Shift 组合键操作绝不破坏大写锁定状态"
    );
}

#[test]
fn test_ctrl_delete_forgets_candidate() {
    let mut session = make_test_session();
    // 键入 "nihao" 并提交候选词，触发用户自适应记录
    for ch in "nihao".chars() {
        session.engine.push_char(ch);
    }
    let committed = session.engine.select_candidate(0, 100);
    assert!(committed.is_some(), "测试夹具应能提交候选词");

    // 再次键入 "nihao"，确认候选非空且定位到第 0 项
    for ch in "nihao".chars() {
        session.engine.push_char(ch);
    }
    assert!(!session.engine.candidates().is_empty());

    // 按下 LeftCtrl + Delete 触发精准忘词
    session.update_modifier_by_key(KEY_LEFTCTRL, true);
    session.selected_index = 0;

    let action = session.process_key(
        KeyProcessContext {
            keycode: KEY_DELETE,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        200,
    );
    assert_eq!(
        action,
        ImeOutputAction::Handled { update_ui: true },
        "组词态下 Ctrl+Delete 应触发忘词并刷新界面"
    );
    assert_eq!(
        session.engine.raw_input(),
        "nihao",
        "Ctrl+Delete 忘词不应误删拼音编码"
    );
}
