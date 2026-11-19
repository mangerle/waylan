use super::super::*;
use super::make_test_session;

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
