use super::*;
use waylan_core::state::SharedInputFlags;
use waylan_core::{AppConfig, InputMode};

#[test]
fn test_apply_initial_config_sets_mode_and_decorations() {
    let mut cfg = AppConfig::default();
    cfg.pinyin.init_chinese = false;
    cfg.pinyin.traditional_chinese = true;
    cfg.pinyin.full_width_punct = false;
    cfg.pinyin.full_width_char = true;
    cfg.ui.page_size = 9;

    let mut runtime = ImeRuntime::default();
    runtime.apply_initial_config(&cfg);

    assert_eq!(runtime.core.mode, InputMode::English);
    assert!(runtime.core.is_traditional());
    assert!(!runtime.core.is_full_width_punct());
    assert!(!runtime.core.punctuation_processor.is_full_width_punct());
    assert!(runtime.core.is_full_width_char());
    assert_eq!(runtime.core.page_size(), 9);
}

#[test]
fn test_reload_config_updates_page_size_and_clamps_viewport() {
    let mut runtime = ImeRuntime::default();
    runtime.core.page_index = 3;
    runtime.core.selected_index = 4;

    let mut cfg = AppConfig::default();
    cfg.ui.page_size = 9;
    cfg.pinyin.full_width_char = true;
    runtime.reload_config(&cfg);

    assert_eq!(runtime.core.page_size(), 9);
    assert_eq!(runtime.core.page_index(), 0);
    assert_eq!(runtime.core.selected_index(), 0);
    assert!(runtime.core.is_full_width_char());
}

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

#[test]
fn test_effective_mode_projection_and_protected_context() {
    let flags = SharedInputFlags::new(true, false, true, false);
    let mut runtime = ImeRuntime::default().with_shared_flags(flags.clone());

    // 1. 初始状态为中文
    assert_eq!(runtime.effective_input_mode(), InputMode::Chinese);
    assert!(runtime.state_snapshot().is_chinese);
    assert!(flags.is_chinese());

    // 2. 进入密码框或锁屏等受保护上下文：有效模式立即投影为英文，且共享标志同步为英文
    runtime.set_protected_context(true);
    assert!(runtime.is_protected_context());
    assert_eq!(runtime.effective_input_mode(), InputMode::English);
    assert!(!runtime.state_snapshot().is_chinese);
    assert!(!flags.is_chinese());
    // 验证底层基础模式未被篡改
    assert_eq!(runtime.core.input_mode(), InputMode::Chinese);

    // 3. 离开密码框或解锁：自动恢复原中文状态
    runtime.set_protected_context(false);
    assert!(!runtime.is_protected_context());
    assert_eq!(runtime.effective_input_mode(), InputMode::Chinese);
    assert!(runtime.state_snapshot().is_chinese);
    assert!(flags.is_chinese());
}

#[test]
fn test_window_mode_tracker_runtime_integration() {
    let mut runtime = ImeRuntime::default();

    // 1. 获焦窗口 1（终端）：开启 terminal_default_english 时初始即为英文
    runtime.focus_window("terminal_1", Some("gnome-terminal-server"));
    assert_eq!(runtime.effective_input_mode(), InputMode::English);

    // 2. 获焦窗口 2（浏览器）：默认为中文
    runtime.focus_window("browser_1", Some("google-chrome"));
    assert_eq!(runtime.effective_input_mode(), InputMode::Chinese);

    // 3. 切回窗口 1：自动恢复英文
    runtime.focus_window("terminal_1", Some("gnome-terminal-server"));
    assert_eq!(runtime.effective_input_mode(), InputMode::English);

    // 4. 切回窗口 2：自动恢复中文
    runtime.focus_window("browser_1", Some("google-chrome"));
    assert_eq!(runtime.effective_input_mode(), InputMode::Chinese);

    // 5. 在窗口 2 期间进入密码框：投影为英文
    runtime.set_protected_context(true);
    assert_eq!(runtime.effective_input_mode(), InputMode::English);

    // 6. 退出密码框：自然恢复窗口 2 的中文
    runtime.set_protected_context(false);
    assert_eq!(runtime.effective_input_mode(), InputMode::Chinese);
}

#[derive(Default)]
struct MockFrontend {
    committed: Vec<String>,
}

impl crate::frontend::InputMethodFrontend for MockFrontend {
    fn commit_text(&mut self, text: &str) {
        self.committed.push(text.to_string());
    }
    fn update_preedit(&mut self, _text: &str, _cursor: usize) {}
    fn update_candidates(&mut self, _state: &waylan_core::CandidateBoxState) {}
    fn hide_candidates(&mut self) {}
    fn move_cursor(&mut self, _offset: i32) {}
    fn sync_input_mode(&mut self, _mode: InputMode) {}
}

#[test]
fn test_protected_context_forces_key_passthrough() {
    let mut runtime = ImeRuntime::default();
    let mut frontend = MockFrontend::default();

    // 默认中文模式下敲击字母 'a'（扫描码 30）：正常组词
    let (consumed, _) = runtime.process_key_event(
        &mut frontend,
        ProcessKeyEventParams {
            key: 30,
            is_press: true,
            is_password_or_locked: false,
            surrounding_text: None,
            inline_preedit: false,
        },
    );
    assert!(consumed);
    assert_eq!(runtime.core.engine.raw_input(), "a");

    // 进入受保护上下文（锁屏或密码框）：缓冲被自动清理
    runtime.set_protected_context(true);
    assert!(runtime.core.engine.raw_input().is_empty());

    // 在受保护上下文下，即使 params 中 is_password_or_locked 误传为 false，也强制全量物理直通
    let (consumed, commit) = runtime.process_key_event(
        &mut frontend,
        ProcessKeyEventParams {
            key: 30,
            is_press: true,
            is_password_or_locked: false,
            surrounding_text: None,
            inline_preedit: false,
        },
    );
    assert!(!consumed, "受保护上下文下物理按键必须全量直通");
    assert_eq!(commit, None);
    assert!(
        runtime.core.engine.raw_input().is_empty(),
        "受保护上下文下绝对不允许产生拼音组词"
    );
}
