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
    let mut cfg = PageKeyConfig::default();
    assert!(cfg.is_prev_page(KEY_MINUS));
    assert!(cfg.is_next_page(KEY_EQUAL));
    assert!(cfg.is_prev_page(KEY_PAGEUP));
    assert!(cfg.is_next_page(KEY_PAGEDOWN));
    assert!(!cfg.is_prev_page(KEY_COMMA));
    assert!(!cfg.is_prev_page(KEY_LEFTBRACE));

    // 开启逗号句号与括号多选
    cfg.comma_period = true;
    cfg.bracket = true;
    assert!(cfg.is_prev_page(KEY_MINUS));
    assert!(cfg.is_prev_page(KEY_COMMA));
    assert!(cfg.is_next_page(KEY_DOT));
    assert!(cfg.is_prev_page(KEY_LEFTBRACE));
    assert!(cfg.is_next_page(KEY_RIGHTBRACE));
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

    // 模拟真实合成器下发的完整 xkb_keymap（包含前置 xkb_keycodes 段与后置 xkb_symbols 段）
    let mut full_layout = KeymapLayout::qwerty();
    let real_xkb = r#"
        xkb_keymap {
            xkb_keycodes "evdev+aliases(qwerty)" {
                minimum = 8;
                maximum = 255;
                <AC01> = 38;
                <AC02> = 39;
                <AC03> = 40;
                <AC04> = 41;
                <AC05> = 42;
                <AC06> = 43;
                <AC07> = 44;
                <AC08> = 45;
                <AC09> = 46;
                <AC10> = 47;
            };
            xkb_symbols "pc+us(dvorak)" {
                key <AC01> { [ a, A ] };
                key <AC02> { [ o, O ] };
                key <AC03> { [ e, E ] };
                key <AC04> { [ u, U ] };
                key <AC05> { [ i, I ] };
                key <AC06> { [ d, D ] };
                key <AC07> { [ h, H ] };
                key <AC08> { [ t, T ] };
                key <AC09> { [ n, N ] };
                key <AC10> { [ s, S ] };
            };
        };
    "#;
    full_layout.update_from_xkb_text(real_xkb);
    assert_eq!(full_layout.get_char(31), Some('o'));
    assert_eq!(full_layout.get_char(32), Some('e'));
    assert_eq!(full_layout.get_char(33), Some('u'));
}

#[test]
fn test_handle_composition_key() {
    let mut engine =
        waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
    let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
    let page_keys = PageKeyConfig::default();

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

#[test]
fn test_v_mode_composition_keys() {
    let mut engine =
        waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
    let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
    let page_keys = PageKeyConfig::default();

    // 1. 输入 'v' 进入 V 模式
    engine.push_char('v');
    assert_eq!(engine.raw_input(), "v");

    // 2. 按主键盘数字 2：必须作为字符推入表达式，绝不可触发选词
    let act = handle_composition_key(&mut engine, &mut punct, KEY_2, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.raw_input(), "v2");

    // 3. 按主键盘数字 0
    let act = handle_composition_key(&mut engine, &mut punct, KEY_0, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.raw_input(), "v20");

    // 4. 按 Shift + 8（乘号 '*'）
    let act = handle_composition_key(&mut engine, &mut punct, KEY_8, true, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.raw_input(), "v20*");

    // 5. 按小键盘数字 5
    let act = handle_composition_key(&mut engine, &mut punct, KEY_KP5, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.raw_input(), "v20*5");

    // 6. 验证计算器召回的首位候选为计算结果 "100"
    let cands = engine.candidates();
    assert!(!cands.is_empty());
    assert_eq!(cands[0].text, "100");

    // 7. 按空格键选词确认：首选计算结果直接上屏并全量清空预编辑缓冲区
    let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Commit("100".to_string())));
    assert!(engine.raw_input().is_empty());
}

#[test]
fn test_smart_navigation_selection_and_overflow_to_cursor_edit() {
    let mut engine =
        waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
    let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
    let page_keys = PageKeyConfig::default();

    // 1. 输入拥有多个候选词的 "xian"（包含“西安”、“先”、“现”、“显”等）
    for ch in "xian".chars() {
        engine.push_char(ch);
    }
    assert_eq!(engine.cursor(), 4);
    assert_eq!(engine.selected_index, 0);
    assert!(engine.candidates().len() >= 2);

    // 2. 选词模式下按 Right：移动候选词高亮
    let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.selected_index, 1);
    assert_eq!(engine.cursor(), 4);

    // 3. 按 Left：退回高亮至第 1 候选词
    let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.selected_index, 0);
    assert_eq!(engine.cursor(), 4);

    // 4. 在第 1 页第 1 候选词再次按 Left：边缘溢出切入拼音光标编辑模式
    let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.cursor(), 3);

    // 5. 拼音光标编辑模式下按 Left：光标继续左移
    let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.cursor(), 2);

    // 6. 拼音光标编辑模式下按 Right：光标向右移动
    let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.cursor(), 3);

    // 7. 拼音光标编辑模式下按空格：光标移回末尾并恢复选词模式
    let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::Handled));
    assert_eq!(engine.cursor(), 4);
    assert_eq!(engine.selected_index, 0);

    // 8. 选词模式下再次按空格：确认候选词上屏
    let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
    assert!(matches!(act, Some(KeyProcessAction::Commit(_))));
    assert!(engine.raw_input().is_empty());
}

#[test]
fn test_smart_navigation_down_page_and_expand_grid() {
    let mut engine =
        waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
    let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
    let page_keys = PageKeyConfig::default();

    engine.push_char('n');
    engine.push_char('i');

    // 连续按 Down 翻页，直到最后一页触发 ExpandGrid
    while engine.can_next_page() {
        let act = handle_composition_key(&mut engine, &mut punct, KEY_DOWN, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Handled));
    }

    // 已经处于末页，再次按 Down 唤出大网格候选面板
    let act = handle_composition_key(&mut engine, &mut punct, KEY_DOWN, false, page_keys, 0);
    assert_eq!(act, Some(KeyProcessAction::ExpandGrid));

    // 按 Up 翻回上一页
    if engine.can_prev_page() {
        let act = handle_composition_key(&mut engine, &mut punct, KEY_UP, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Handled));
    }
}
