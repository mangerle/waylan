use super::super::*;
use super::make_test_session;

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
