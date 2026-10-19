use super::*;
use waylan_core::state::SharedInputFlags;
use waylan_core::InputMode;

#[test]
fn test_sync_shared_mode_comprehensive() {
    let mut runtime = ImeRuntime::default();
    // 初始托盘 flags：中文模式、繁体关闭、半角标点、半角字符
    let flags = SharedInputFlags::new(true, false, false, false);
    runtime = runtime.with_shared_flags(flags.clone());

    // 初始状态验证：引擎默认中文全角标点，尚未同步
    assert_eq!(runtime.core.mode, InputMode::Chinese);
    assert!(!runtime.core.engine.config.traditional_chinese);
    assert!(runtime.core.punctuation_processor.is_full_width_punct());
    assert!(!runtime.core.is_full_width_char());

    // 首次同步：将托盘的半角标点同步至运行时
    let mode_changed = runtime.sync_shared_mode();
    assert_eq!(mode_changed, None);
    assert!(!runtime.core.punctuation_processor.is_full_width_punct());

    // 外部托盘切换为繁体、全角标点、全角字符、英文
    flags.set_traditional(true);
    flags.set_full_width_punct(true);
    flags.set_full_width_char(true);
    flags.set_chinese(false);

    let mode_changed = runtime.sync_shared_mode();
    assert_eq!(mode_changed, Some(InputMode::English));
    assert_eq!(runtime.core.mode, InputMode::English);
    assert!(runtime.core.engine.config.traditional_chinese);
    assert!(runtime.core.punctuation_processor.is_full_width_punct());
    assert!(runtime.core.is_full_width_char());

    // 再次同步，无状态变化
    let mode_changed = runtime.sync_shared_mode();
    assert_eq!(mode_changed, None);

    // 外部托盘切回简体和中文
    flags.set_traditional(false);
    flags.set_chinese(true);
    let mode_changed = runtime.sync_shared_mode();
    assert_eq!(mode_changed, Some(InputMode::Chinese));
    assert!(!runtime.core.engine.config.traditional_chinese);
}

#[test]
fn test_ime_runtime_single_source_of_truth() {
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let flags = SharedInputFlags::new(true, false, true, false);
    let mut runtime = ImeRuntime::default()
        .with_shared_flags(flags.clone())
        .with_mode_notify_tx(tx);

    // 验证初始状态一致性
    let snap = runtime.state_snapshot();
    assert!(snap.is_chinese);
    assert!(!snap.is_caps_locked);
    assert_eq!(flags.snapshot(), snap);

    // 切换中英文模式并广播
    let mode = runtime.toggle_input_mode();
    assert_eq!(mode, InputMode::English);
    assert!(!flags.is_chinese());
    assert_eq!(runtime.state_snapshot(), flags.snapshot());
    assert!(rx.try_recv().is_ok());

    // 切换简繁
    let trad = runtime.toggle_traditional();
    assert!(trad);
    assert!(flags.is_traditional());
    assert_eq!(runtime.state_snapshot(), flags.snapshot());
    assert!(rx.try_recv().is_ok());

    // 切换标点全半角
    let punct = runtime.toggle_full_width_punct();
    assert!(!punct);
    assert!(!flags.is_full_width_punct());
    assert_eq!(runtime.state_snapshot(), flags.snapshot());
    assert!(rx.try_recv().is_ok());

    // 切换字符全半角
    let char_full = runtime.toggle_full_width_char();
    assert!(char_full);
    assert!(flags.is_full_width_char());
    assert_eq!(runtime.state_snapshot(), flags.snapshot());
    assert!(rx.try_recv().is_ok());

    // 更新 CapsLock 大写锁定
    runtime.update_caps_lock_state(true);
    assert!(flags.is_caps_locked());
    assert_eq!(runtime.state_snapshot(), flags.snapshot());
    assert!(rx.try_recv().is_ok());
}
