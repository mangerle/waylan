use super::*;
use crate::keymap::KeymapLayout;

#[test]
fn test_keycode_conversions() {
    assert_eq!(keycode_to_char(30), Some('a'));
    assert_eq!(keycode_to_char(44), Some('z'));
    assert_eq!(keycode_to_char(999), None);

    assert_eq!(keycode_to_digit(KEY_1), Some(1));
    assert_eq!(keycode_to_digit(KEY_9), Some(9));
    assert_eq!(keycode_to_digit(KEY_0), Some(10));
    assert_eq!(keycode_to_digit(KEY_ESC), None);

    // 小键盘数字键码映射
    assert_eq!(keycode_to_digit(KEY_KP1), Some(1));
    assert_eq!(keycode_to_digit(KEY_KP5), Some(5));
    assert_eq!(keycode_to_digit(KEY_KP9), Some(9));
    assert_eq!(keycode_to_digit(KEY_KP0), Some(10));

    assert_eq!(keycode_to_ascii_digit_char(KEY_0), Some('0'));
    assert_eq!(keycode_to_ascii_digit_char(KEY_KP0), Some('0'));
    assert_eq!(keycode_to_ascii_digit_char(KEY_KP7), Some('7'));
    assert_eq!(keycode_to_punctuation(KEY_COMMA), Some(','));
    assert_eq!(keycode_to_punctuation_with_shift(KEY_1, true), Some('!'));
}

#[test]
fn test_page_key_config() {
    let cfg = PageKeyConfig::MinusEqual;
    assert!(cfg.is_prev_page(KEY_MINUS));
    assert!(cfg.is_next_page(KEY_EQUAL));
    assert!(cfg.is_prev_page(KEY_PAGEUP));
    assert!(cfg.is_next_page(KEY_PAGEDOWN));
}

#[test]
fn test_modifier_state() {
    let mut mods = ModifierState::default();
    assert!(!mods.is_shift_active());
    assert!(!mods.has_ctrl_or_alt());

    // 同时按下左 Shift 与右 Shift，松开左 Shift 后仍应保持 Shift 激活
    mods.update(KEY_LEFTSHIFT, true);
    mods.update(KEY_RIGHTSHIFT, true);
    assert!(mods.is_shift_active());
    mods.update(KEY_LEFTSHIFT, false);
    assert!(
        mods.is_shift_active(),
        "松开单侧 Shift 时若另一侧仍按下，不应清除 MOD_SHIFT"
    );
    mods.update(KEY_RIGHTSHIFT, false);
    assert!(!mods.is_shift_active());

    // 测试 Super/Meta 键跟踪
    mods.update(KEY_LEFTMETA, true);
    assert!(mods.has_ctrl_or_alt());
    mods.update(KEY_LEFTMETA, false);
    assert!(!mods.has_ctrl_or_alt());

    mods.update(KEY_LEFTCTRL, true);
    assert!(mods.has_ctrl_or_alt());
    mods.reset();
    assert!(!mods.has_ctrl_or_alt());
}

#[test]
fn test_consumed_key_tracker_symmetry() {
    let mut tracker = ConsumedKeyTracker::new();
    tracker.record_press(KEY_SPACE, true);
    tracker.record_press(KEY_LEFTSHIFT, false);

    assert!(tracker.take_release(KEY_SPACE));
    assert!(!tracker.take_release(KEY_SPACE));
    assert!(!tracker.take_release(KEY_LEFTSHIFT));
}

#[test]
fn test_keymap_layout_dynamic_update() {
    let mut layout = KeymapLayout::qwerty();
    // 初始 QWERTY: 扫描码 30 映射到 'a'
    assert_eq!(layout.get_char(30), Some('a'));

    // 模拟一段 XKB symbols 文本（例如将 AC01 映射为 'o'）
    let mock_xkb = r#"
        key <AC01> { [ o, O ] };
        key <AC02> { [ a, A ] };
        key <AC03> { [ e, E ] };
        key <AC04> { [ u, U ] };
        key <AC05> { [ i, I ] };
        key <AC06> { [ d, D ] };
        key <AC07> { [ h, H ] };
        key <AC08> { [ t, T ] };
        key <AC09> { [ n, N ] };
        key <AC10> { [ s, S ] };
    "#;
    layout.update_from_xkb_text(mock_xkb);
    assert_eq!(layout.get_char(30), Some('o'));
    assert_eq!(layout.get_char(31), Some('a'));
    // 未在文本中重定义的按键保持 QWERTY 兜底
    assert_eq!(layout.get_char(48), Some('b'));
}

#[test]
fn test_handle_composition_key() {
    let mut engine =
        waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
    let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
    let page_keys = PageKeyConfig::MinusEqual;

    // 模拟输入 "ni"
    engine.push_char('n');
    engine.push_char('i');
    assert_eq!(engine.raw_input(), "ni");

    // 测试光标移动
    let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));

    let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));

    // 测试回车字母上屏
    let act = handle_composition_key(&mut engine, &mut punct, KEY_ENTER, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Commit("ni".to_string())));
    assert!(engine.raw_input().is_empty());

    // 测试单引号隔音符输入
    engine.push_char('x');
    engine.push_char('i');
    let act = handle_composition_key(&mut engine, &mut punct, KEY_APOSTROPHE, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.raw_input(), "xi'");
    engine.push_char('a');
    engine.push_char('n');
    assert_eq!(engine.raw_input(), "xi'an");

    // 测试 ESC 清空
    let act = handle_composition_key(&mut engine, &mut punct, KEY_ESC, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Reset));
    assert!(engine.raw_input().is_empty());

    // 测试小键盘回车字母上屏
    engine.push_char('h');
    engine.push_char('a');
    let act = handle_composition_key(&mut engine, &mut punct, KEY_KPENTER, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Commit("ha".to_string())));
    assert!(engine.raw_input().is_empty());

    // 测试小键盘数字 1 选词上屏
    engine.push_char('n');
    engine.push_char('i');
    let act = handle_composition_key(&mut engine, &mut punct, KEY_KP1, false, page_keys, 0);
    assert!(matches!(act, Some(KeyProcessAction::Commit(_))));
    assert!(engine.raw_input().is_empty());
}
