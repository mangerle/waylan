use super::*;
use std::sync::Arc;
use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;

#[test]
fn test_v1_session_initialization() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let session = WaylanV1Session::with_engine(engine);

    assert_eq!(session.mode, InputMode::Chinese);
    assert_eq!(session.commit_serial, 0);
    assert_eq!(session.current_serial, 0);
    assert!(session.active_context.is_none());
    assert!(session.grabbed_keyboard.is_none());
    assert_eq!(session.page_keys, PageKeyConfig::MinusEqual);
}

#[test]
fn test_v1_modifier_and_state_tracking() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV1Session::with_engine(engine);

    assert!(!session.is_shift_active());
    assert!(!session.has_ctrl_or_alt());

    session.update_modifier_by_key(KEY_LEFTSHIFT, true);
    assert!(session.is_shift_active());

    session.update_modifier_by_key(KEY_LEFTCTRL, true);
    assert!(session.has_ctrl_or_alt());

    session.update_modifier_by_key(KEY_LEFTSHIFT, false);
    assert!(!session.is_shift_active());
    assert!(session.has_ctrl_or_alt());

    session.update_modifier_by_key(KEY_LEFTCTRL, false);
    assert!(!session.has_ctrl_or_alt());
}

#[test]
fn test_v1_sync_atomic_state_and_mode_toggle() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV1Session::with_engine(engine);

    let zh_flag = Arc::new(AtomicBool::new(true));
    session.shared_is_chinese = Some(zh_flag.clone());

    // 外部修改为英文
    zh_flag.store(false, Ordering::Release);
    session.sync_atomic_state();
    assert_eq!(session.mode, InputMode::English);

    // 外部修改为中文
    zh_flag.store(true, Ordering::Release);
    session.sync_atomic_state();
    assert_eq!(session.mode, InputMode::Chinese);
}
