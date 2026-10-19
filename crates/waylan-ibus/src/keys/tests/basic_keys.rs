use super::super::core::IbusSessionCore;
use super::super::keyval::{ibus_keyval_to_evdev, IBUS_RELEASE_MASK};
use waylan_core::input::*;
use waylan_core::InputMode;
use waylan_engine::{AlgoConfig, AlgoEngine};

#[test]
fn test_ibus_keyval_mapping() {
    assert_eq!(ibus_keyval_to_evdev(b'a' as u32, 0), 30);
    assert_eq!(ibus_keyval_to_evdev(b'Z' as u32, 0), 44);
    assert_eq!(ibus_keyval_to_evdev(0xff08, 0), KEY_BACKSPACE);
    assert_eq!(ibus_keyval_to_evdev(0xffe1, 0), KEY_LEFTSHIFT);
    assert_eq!(ibus_keyval_to_evdev(0xffe5, 0), KEY_CAPSLOCK);
    assert_eq!(ibus_keyval_to_evdev(0xffe6, 0), KEY_CAPSLOCK);
}

#[test]
fn test_ibus_session_pinyin_and_shift_toggle() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 输入 'x', 'i', 'a', 'n'
    for ch in ['x', 'i', 'a', 'n'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
        assert!(out.update_ui);
    }
    assert_eq!(core.runtime.core.engine.raw_input(), "xian");

    // 按下并释放 Shift 切换至英文模式并自动上屏残留编码
    let _ = core.process_key_event(0xffe1, KEY_LEFTSHIFT, 0);
    let rel = core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_RELEASE_MASK);
    assert_eq!(rel.mode_changed, Some(InputMode::English));
    assert_eq!(rel.commit_text.as_deref(), Some("xian"));
}

#[test]
fn test_ibus_date_time_tab_expansion_and_selection() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 输入拼音 "riqi"
    for ch in ['r', 'i', 'q', 'i'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
        assert!(out.update_ui);
    }

    // 验证候选词：第 1 位为词库词“日期”，第 2 位为动态日期且包含年份
    let cands = core.current_candidates();
    assert!(cands.len() >= 2);
    assert_eq!(cands[0].text, "日期");
    assert!(cands[1].text.contains("年"));
    assert!(cands[1].is_expandable);
    assert_eq!(core.runtime.core.selected_index(), 0);

    // 2. 按 Tab 键触发二级展开
    let tab_out = core.process_key_event(0xff09, KEY_TAB, 0);
    assert!(tab_out.consumed);
    assert!(tab_out.update_ui);
    assert!(core.runtime.core.is_card_expanded());

    // 3. 展开后 current_candidates 应返回全部 7 种细分日期格式
    let expanded_cands = core.current_candidates();
    assert_eq!(expanded_cands.len(), 7);
    assert!(!expanded_cands[0].is_expandable);

    // 4. 按数字键 2 选取第 2 种格式上屏
    let num_out = core.process_key_event(b'2' as u32, KEY_2, 0);
    assert!(num_out.consumed);
    assert_eq!(num_out.commit_text, Some(expanded_cands[1].text.clone()));
    assert!(!core.runtime.core.is_card_expanded());
    assert!(core.runtime.core.engine.raw_input().is_empty());
}

#[test]
fn test_ibus_password_context_reset_and_recovery() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 模拟进入密码输入框
    core.is_password_context = true;
    let outcome = core.process_key_event(b'n' as u32, 49, 0);
    assert!(
        !outcome.consumed,
        "密码输入框中按键应直通放行，不被输入法消费"
    );
    assert!(core.runtime.core.engine.raw_input().is_empty());

    // 2. 模拟切换窗口或失焦触发重置
    core.reset_context();
    assert!(
        !core.is_password_context,
        "reset_context 必须彻底重置密码框上下文标志"
    );

    // 3. 再次在普通窗口输入，应恢复拼音消费与候选生成
    let outcome2 = core.process_key_event(b'n' as u32, 49, 0);
    assert!(
        outcome2.consumed,
        "退出密码框后输入法应完全恢复可用并消费按键"
    );
    assert_eq!(core.runtime.core.engine.raw_input(), "n");
}

#[test]
fn test_ibus_screen_lock_passthrough_and_recovery() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 模拟进入锁屏状态
    core.is_screen_locked = true;
    let outcome = core.process_key_event(b'a' as u32, 30, 0);
    assert!(
        !outcome.consumed,
        "系统锁屏状态下所有硬件按键必须完全直通放行，绝不消费"
    );
    assert!(
        core.runtime.core.engine.raw_input().is_empty(),
        "锁屏状态下严禁将按键录入算法引擎"
    );

    // 2. 模拟解锁屏幕回到桌面
    core.is_screen_locked = false;
    let outcome2 = core.process_key_event(b'a' as u32, 30, 0);
    assert!(
        outcome2.consumed,
        "解锁屏幕后应立即恢复拼音拦截与候选词处理"
    );
    assert_eq!(core.runtime.core.engine.raw_input(), "a");
}
