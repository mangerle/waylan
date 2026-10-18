use super::super::core::IbusSessionCore;
use super::super::keyval::{
    IBUS_CONTROL_MASK, IBUS_LOCK_MASK, IBUS_RELEASE_MASK, IBUS_SHIFT_MASK, KEY_F,
};
use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;
use waylan_core::input::*;
use waylan_core::state::SharedInputFlags;
use waylan_core::{FullWidthCharShortcut, FullWidthPunctShortcut, InputMode, TraditionalShortcut};

#[test]
fn test_ibus_caps_lock_passthrough() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 中文状态输入 'a', 'i'
    for ch in ['a', 'i'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
    }
    assert_eq!(core.core.engine.raw_input(), "ai");

    // 2. 按下 CapsLock 键：未完成拼音立即上屏，按键本身不被消费（透传给应用），且通知模式变更为英文
    let caps_out = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert!(!caps_out.consumed, "CapsLock 键本身不应被消费");
    assert_eq!(caps_out.commit_text.as_deref(), Some("ai"));
    assert_eq!(
        caps_out.mode_changed,
        Some(InputMode::English),
        "CapsLock 激活时模式应指示为英文"
    );
    assert!(core.core.engine.raw_input().is_empty());

    // 3. 在 CapsLock 大写锁定状态下（state 带有 IBUS_LOCK_MASK），输入字母应原生透传
    let a_out = core.process_key_event(b'A' as u32, 0, IBUS_LOCK_MASK);
    assert!(!a_out.consumed, "大写锁定状态下字母键应原生透传给应用");

    // 4. 再次按下并释放 CapsLock 解除锁定，模式恢复为中文拼音
    let caps_unlock = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_RELEASE_MASK);
    assert!(!core.is_caps_locked, "解除 CapsLock 后应脱离大写锁定状态");
    assert_eq!(
        caps_unlock.mode_changed,
        Some(InputMode::Chinese),
        "解除锁定后应恢复中文模式"
    );

    // 5. 验证此时按字母恢复中文拼音输入
    let ni_out = core.process_key_event(b'n' as u32, 0, 0);
    assert!(ni_out.consumed, "解除大写锁定后应恢复中文拼音输入");
    assert_eq!(core.core.engine.raw_input(), "n");
}

#[test]
fn test_ibus_caps_lock_with_shift_interaction() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 中文模式下开启 CapsLock
    let caps_on = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert_eq!(caps_on.mode_changed, Some(InputMode::English));
    assert!(core.is_caps_locked);

    // 2. 大写锁定期间，敲击 Shift 键（单按或误触），状态机不应破坏基准中文模式
    let shift_down = core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_LOCK_MASK);
    assert_eq!(shift_down.mode_changed, None);
    let shift_up =
        core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_LOCK_MASK | IBUS_RELEASE_MASK);
    assert_eq!(shift_up.mode_changed, None);
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "大写锁定时敲击 Shift 不得篡改基准中文模式"
    );

    // 3. 敲击 CapsLock 解除锁定：必须一次性成功回到中文模式，无需按两下
    let caps_off_press = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert_eq!(
        caps_off_press.mode_changed,
        Some(InputMode::Chinese),
        "仅按一次 CapsLock 必须立即切回中文"
    );
    let caps_off_rel = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_RELEASE_MASK);
    assert_eq!(caps_off_rel.mode_changed, None);
    assert!(!core.is_caps_locked);

    // 4. 后续输入正常进入拼音输入
    let letter = core.process_key_event(b'h' as u32, 0, 0);
    assert!(letter.consumed);
    assert_eq!(core.core.engine.raw_input(), "h");
}

#[test]
fn test_ibus_caps_lock_in_english_mode_no_flicker() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.core.mode = InputMode::English;

    // 1. 在纯英文模式下开启 CapsLock：不触发任何 mode_changed 信号
    let caps_on = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert_eq!(
        caps_on.mode_changed, None,
        "英文模式下开启 CapsLock 不应派发任何模式变更信号"
    );
    assert!(core.is_caps_locked);

    // 2. 在纯英文模式下关闭 CapsLock：不触发任何 mode_changed 信号，防止状态栏闪烁中文
    let caps_off = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert_eq!(
        caps_off.mode_changed, None,
        "英文模式下关闭 CapsLock 绝不应闪现中文模式"
    );
    assert!(!core.is_caps_locked);
    assert_eq!(core.core.mode, InputMode::English, "始终保持纯英文模式");
}

#[test]
fn test_ibus_caps_lock_tray_flags_and_subsequent_keys() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 1. 中文状态下按下 CapsLock：大写标志权威置为 true（托盘读取该标志直接展现 A 大写图标与大写菜单项）
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert!(core.is_caps_locked);
    assert!(flags.is_caps_locked());
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "基准中文模式状态不受篡改"
    );

    // 2. 随后的普通字符按键（即使客户端 state 未附带 IBUS_LOCK_MASK），大写状态绝不可被误冲刷
    let letter = core.process_key_event(b'A' as u32, 0, 0);
    assert!(!letter.consumed, "大写锁定下普通字母按键应原生透传");
    assert!(
        core.is_caps_locked,
        "后续未带Lock掩码的按键绝不能冲刷掉大写状态"
    );
    assert!(flags.is_caps_locked());

    // 3. 再次按下 CapsLock 退出大写：大写标志关闭，无缝回到中文模式
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert!(!core.is_caps_locked);
    assert!(!flags.is_caps_locked());
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "退出大写锁定后丝滑保持中文模式"
    );
    assert!(flags.is_chinese());
}

#[test]
fn test_ibus_shared_settings_sync_and_full_width() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 默认状态：简体、全角标点、半角字母
    assert!(!core.core.engine.config.traditional_chinese);
    assert!(core.core.punctuation_processor.full_width_punct);
    assert!(!core.is_full_width_char());

    // 1. 托盘触发切换为繁体
    flags.set_traditional(true);
    let _ = core.process_key_event(0, 0, 0);
    assert!(
        core.core.engine.config.traditional_chinese,
        "引擎应自动同步繁体设置"
    );

    // 2. 托盘触发切换为半角标点
    flags.set_full_width_punct(false);
    let outcome = core.process_key_event(b',' as u32, 51, 0);
    assert_eq!(
        outcome.commit_text,
        Some(",".to_string()),
        "半角标点模式下应输出半角逗号"
    );

    // 3. 托盘触发切换为全角字符，并在英文模式下敲击字母
    flags.set_full_width_char(true);
    flags.set_chinese(false);
    let letter_outcome = core.process_key_event(b'a' as u32, 30, 0);
    assert_eq!(
        letter_outcome.commit_text,
        Some("ａ".to_string()),
        "全角字符模式下英文应输出全角字母"
    );
}

#[test]
fn test_ibus_global_shortcuts_toggle() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 默认快捷键：Ctrl+Shift+F (简繁), Shift+Space (字符全半角), Ctrl+. (标点全半角)
    assert_eq!(
        core.core.traditional_shortcut,
        TraditionalShortcut::CtrlShiftF
    );
    assert_eq!(
        core.core.full_width_char_shortcut,
        FullWidthCharShortcut::ShiftSpace
    );
    assert_eq!(
        core.core.full_width_punct_shortcut,
        FullWidthPunctShortcut::CtrlPeriod
    );

    // 1. 测试简繁切换快捷键 (Ctrl + Shift + F)
    let state_ctrl_shift = IBUS_CONTROL_MASK | IBUS_SHIFT_MASK;
    let res_trad = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
    assert!(res_trad.consumed, "简繁快捷键必须被输入法消费");
    assert!(
        core.core.engine.config.traditional_chinese,
        "简繁状态应切换为繁体"
    );
    assert!(flags.is_traditional(), "共享托盘标志应同步为繁体");

    // 再次按下切回简体
    let res_trad2 = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
    assert!(res_trad2.consumed);
    assert!(
        !core.core.engine.config.traditional_chinese,
        "简繁状态应切回简体"
    );
    assert!(!flags.is_traditional());

    // 2. 测试字符全半角切换快捷键 (Shift + Space)
    let res_char = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
    assert!(res_char.consumed, "字符全半角快捷键必须被消费");
    assert!(core.is_full_width_char(), "字符全半角状态应切换为全角");
    assert!(flags.is_full_width_char());

    // 再次按下切回半角
    let res_char2 = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
    assert!(res_char2.consumed);
    assert!(!core.is_full_width_char(), "字符全半角状态应切回半角");
    assert!(!flags.is_full_width_char());

    // 3. 测试标点全半角切换快捷键 (Ctrl + .)
    let res_punct = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
    assert!(res_punct.consumed, "标点全半角快捷键必须被消费");
    assert!(
        !core.core.punctuation_processor.full_width_punct,
        "标点状态应由全角切换为半角"
    );
    assert!(!flags.is_full_width_punct());

    // 再次按下切回全角
    let res_punct2 = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
    assert!(res_punct2.consumed);
    assert!(
        core.core.punctuation_processor.full_width_punct,
        "标点状态应切回全角中文标点"
    );
    assert!(flags.is_full_width_punct());
}
