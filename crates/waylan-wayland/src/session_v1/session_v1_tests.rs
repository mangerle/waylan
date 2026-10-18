use super::*;
use waylan_engine::{AlgoConfig, AlgoEngine};

#[test]
fn test_v1_session_initialization() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let session = WaylanV1Session::with_engine(engine);

    assert_eq!(session.runtime.core.mode, InputMode::Chinese);
    assert_eq!(session.protocol.commit_serial, 0);
    assert_eq!(session.protocol.current_serial, 0);
    assert!(session.protocol.active_context.is_none());
    assert!(session.protocol.grabbed_keyboard.is_none());
    assert_eq!(session.runtime.core.page_keys, PageKeyConfig::default());
}

#[test]
fn test_v1_modifier_and_state_tracking() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV1Session::with_engine(engine);

    assert!(!session.runtime.core.is_shift_active());
    assert!(!session.runtime.core.has_ctrl_or_alt());

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTSHIFT, true);
    assert!(session.runtime.core.is_shift_active());

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTCTRL, true);
    assert!(session.runtime.core.has_ctrl_or_alt());

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTSHIFT, false);
    assert!(!session.runtime.core.is_shift_active());
    assert!(session.runtime.core.has_ctrl_or_alt());

    session
        .runtime
        .core
        .update_modifier_by_key(KEY_LEFTCTRL, false);
    assert!(!session.runtime.core.has_ctrl_or_alt());
}

#[test]
fn test_v1_sync_atomic_state_and_mode_toggle() {
    use waylan_core::state::SharedInputFlags;

    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV1Session::with_engine(engine);

    let flags = SharedInputFlags::new(true, false, false, false);
    session.runtime.shared_flags = Some(flags.clone());

    // 外部修改为英文
    flags.set_chinese(false);
    session.sync_atomic_state();
    assert_eq!(session.runtime.core.mode, InputMode::English);

    // 外部修改为中文
    flags.set_chinese(true);
    session.sync_atomic_state();
    assert_eq!(session.runtime.core.mode, InputMode::Chinese);
}

#[test]
fn test_v1_keymap_layout_dynamic_mapping() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut session = WaylanV1Session::with_engine(engine);

    // 默认 QWERTY 布局下扫描码 16 (Q) 应映射为 'q'
    assert_eq!(session.runtime.core.keymap_layout.get_char(16), Some('q'));
    assert_eq!(
        session.runtime.core.keymap_layout.get_char(KEY_APOSTROPHE),
        Some('\'')
    );

    // 自定义动态布局：将扫描码 16 映射为 'x' (如自定义或 Dvorak 布局)
    let mut custom_map = std::collections::HashMap::new();
    custom_map.insert(16, 'x');
    session.runtime.core.keymap_layout = KeymapLayout::new(custom_map);

    assert_eq!(session.runtime.core.keymap_layout.get_char(16), Some('x'));
}
