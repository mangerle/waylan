use super::*;

#[test]
fn test_should_auto_passthrough_english_contexts() {
    // URL 与邮箱
    assert!(should_auto_passthrough_english("打开 https://github.com/"));
    assert!(should_auto_passthrough_english("www.rust-lang."));
    assert!(should_auto_passthrough_english("admin@example."));

    // 路径与代码符号
    assert!(should_auto_passthrough_english("cd /home/wsl/coding/"));
    assert!(should_auto_passthrough_english("./src/main"));
    assert!(should_auto_passthrough_english("std::collections::"));
    assert!(should_auto_passthrough_english("ptr->"));

    // 蛇形变量、成员访问与驼峰标识符
    assert!(should_auto_passthrough_english("let user_name"));
    assert!(should_auto_passthrough_english("self.config."));
    assert!(should_auto_passthrough_english("HashMap"));
    assert!(should_auto_passthrough_english("myVar"));

    // 正常中文、空白结尾与空上下文绝不误触发
    assert!(!should_auto_passthrough_english(""));
    assert!(!should_auto_passthrough_english("你好世界"));
    assert!(!should_auto_passthrough_english("https://github.com/ "));
    assert!(!should_auto_passthrough_english("HashMap "));
    assert!(!should_auto_passthrough_english("纯小写abc"));
}

#[test]
fn test_undo_last_commit_restores_pinyin() {
    let mut session = ImeCoreSession::default();
    // 输入 "wo" (evdev: w=17, o=24) + 空格上屏
    for key in [17u32, 24u32] {
        let act = session.process_key(
            KeyProcessContext {
                keycode: key,
                is_press: true,
                is_password_or_locked: false,
                surrounding_prefix: None,
            },
            1000,
        );
        assert!(matches!(act, ImeOutputAction::Handled { update_ui: true }));
    }
    let commit_act = session.process_key(
        KeyProcessContext {
            keycode: KEY_SPACE,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1001,
    );
    let committed = match commit_act {
        ImeOutputAction::Commit(t) => t,
        other => panic!("期望 Commit，实际: {:?}", other),
    };
    assert!(session.engine.raw_input().is_empty());

    // 紧接着按 Ctrl+Backspace 触发“选错即撤回”
    session.process_key(
        KeyProcessContext {
            keycode: KEY_LEFTCTRL,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1002,
    );
    let undo_act = session.process_key(
        KeyProcessContext {
            keycode: KEY_BACKSPACE,
            is_press: true,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        1002,
    );
    assert_eq!(
        undo_act,
        ImeOutputAction::UndoCommitAndRestore {
            delete_bytes: committed.len() as u32
        }
    );
    assert_eq!(session.engine.raw_input(), "wo");
    assert!(!session.engine.candidates().is_empty());
}
