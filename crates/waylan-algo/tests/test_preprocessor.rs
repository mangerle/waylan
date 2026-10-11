use waylan_algo::preprocessor::*;
use waylan_core::{InputMode, TextAction};

#[test]
fn test_shift_toggle() {
    let mut sm = ShiftToggleStateMachine::new();
    // 单击 Shift 切换模式
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            false,
            InputMode::Chinese
        ),
        Some(InputMode::English)
    );

    // Shift + A 不切换
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(sm.on_key_event(30, true, InputMode::Chinese), None); // A
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            false,
            InputMode::Chinese
        ),
        None
    );

    // 其它键在 Shift 期间释放也不切换
    sm.reset();
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(sm.on_key_event(30, false, InputMode::Chinese), None); // Release A
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            false,
            InputMode::Chinese
        ),
        None
    );

    // 左右 Shift 交叠按下不切换
    sm.reset();
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_RIGHTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            false,
            InputMode::Chinese
        ),
        None
    );
}

#[test]
fn test_punctuation_conversion() {
    let mut pp = PunctuationProcessor::new();
    // 斜杠替换顿号
    let res = pp.process_punctuation('/').expect("应替换顿号");
    assert_eq!(res, vec![TextAction::Commit("、".to_string())]);

    // 数字后冒号保护
    pp.record_commit("12");
    let res_colon = pp.process_punctuation(':').expect("应半角冒号");
    assert_eq!(res_colon, vec![TextAction::Commit(":".to_string())]);

    // 自动成对括号
    let res_bracket = pp.process_punctuation('(').expect("成对括号");
    assert_eq!(
        res_bracket,
        vec![
            TextAction::Commit("（）".to_string()),
            TextAction::MoveCursor(-1),
        ]
    );

    // 常规逗号句号映射
    assert_eq!(
        pp.process_punctuation(','),
        Some(vec![TextAction::Commit("，".to_string())])
    );
    assert_eq!(
        pp.process_punctuation('.'),
        Some(vec![TextAction::Commit("。".to_string())])
    );

    // 成对书名号《》
    let res_book = pp.process_punctuation('<').expect("成对书名号");
    assert_eq!(
        res_book,
        vec![
            TextAction::Commit("《》".to_string()),
            TextAction::MoveCursor(-1),
        ]
    );

    // 成对双引号“”
    let res_quote = pp.process_punctuation('"').expect("成对双引号");
    assert_eq!(
        res_quote,
        vec![
            TextAction::Commit("“”".to_string()),
            TextAction::MoveCursor(-1),
        ]
    );

    // 数字后小数点保护 3.14
    pp.record_commit("3");
    assert_eq!(
        pp.process_punctuation('.'),
        Some(vec![TextAction::Commit(".".to_string())])
    );

    // 数字后斜杠保护 2026/10/10
    pp.record_commit("2026");
    assert_eq!(
        pp.process_punctuation('/'),
        Some(vec![TextAction::Commit("/".to_string())])
    );

    // 网址模式标点直通
    assert_eq!(
        pp.process_punctuation_with_context('.', "www.baidu"),
        Some(vec![TextAction::Commit(".".to_string())])
    );
    assert_eq!(
        pp.process_punctuation_with_context('/', "https://github"),
        Some(vec![TextAction::Commit("/".to_string())])
    );
    assert_eq!(
        pp.process_punctuation_with_context('.', "user@example"),
        Some(vec![TextAction::Commit(".".to_string())])
    );

    // 历史前序 URL 不应污染后续中文标点
    assert_eq!(
        pp.process_punctuation_with_context('.', "访问了 https://github 之后，我们继续"),
        Some(vec![TextAction::Commit("。".to_string())])
    );
}

#[test]
fn test_ctrl_toggle_and_punctuation_options() {
    let mut sm = ShiftToggleStateMachine::with_toggle_key(ToggleKeyMode::Ctrl);
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTSHIFT,
            false,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTCTRL,
            true,
            InputMode::Chinese
        ),
        None
    );
    assert_eq!(
        sm.on_key_event(
            ShiftToggleStateMachine::KEY_LEFTCTRL,
            false,
            InputMode::Chinese
        ),
        Some(InputMode::English)
    );

    let mut pp = PunctuationProcessor::with_options(false, false, false);
    assert_eq!(
        pp.process_punctuation('/'),
        Some(vec![TextAction::Commit("/".to_string())])
    );
    pp.record_commit("12");
    assert_eq!(
        pp.process_punctuation(':'),
        Some(vec![TextAction::Commit("：".to_string())])
    );
    assert_eq!(
        pp.process_punctuation('('),
        Some(vec![TextAction::Commit("（".to_string())])
    );

    // 验证非自动成对补全模式下，双引号交替输出 “ 与 ”
    assert_eq!(
        pp.process_punctuation('"'),
        Some(vec![TextAction::Commit("“".to_string())])
    );
    assert_eq!(
        pp.process_punctuation('"'),
        Some(vec![TextAction::Commit("”".to_string())])
    );
    assert_eq!(
        pp.process_punctuation('"'),
        Some(vec![TextAction::Commit("“".to_string())])
    );

    // 验证非自动成对补全模式下，单引号交替输出 ‘ 与 ’
    assert_eq!(
        pp.process_punctuation('\''),
        Some(vec![TextAction::Commit("‘".to_string())])
    );
    assert_eq!(
        pp.process_punctuation('\''),
        Some(vec![TextAction::Commit("’".to_string())])
    );
}

#[test]
fn test_url_and_email_context_punctuation() {
    let mut pp = PunctuationProcessor::new();

    // 当用户输入 https 并提交后，后续冒号与斜杠应保持半角直通
    pp.record_commit("https");
    assert_eq!(
        pp.process_punctuation(':'),
        Some(vec![TextAction::Commit(":".to_string())])
    );
    pp.record_commit(":");
    assert_eq!(
        pp.process_punctuation('/'),
        Some(vec![TextAction::Commit("/".to_string())])
    );
    pp.record_commit("/");
    assert_eq!(
        pp.process_punctuation('/'),
        Some(vec![TextAction::Commit("/".to_string())])
    );

    // 当用户输入 www 后，后续句点应保持半角直通
    pp.reset();
    pp.record_commit("www");
    assert_eq!(
        pp.process_punctuation('.'),
        Some(vec![TextAction::Commit(".".to_string())])
    );

    // 当用户输入邮箱前缀后，后续句点应保持半角直通
    pp.reset();
    pp.record_commit("developer@gmail");
    assert_eq!(
        pp.process_punctuation('.'),
        Some(vec![TextAction::Commit(".".to_string())])
    );
}
