//! IBus 键盘事件映射与输入法会话核心按键状态机处理
//!
//! # 设计原理
//! - **实现初衷**：IBus `ProcessKeyEvent(keyval, keycode, state)` 传入的是 X11/GDK `keyval` 符号与修饰位掩码，
//!   而微澜的核心按键决策层 (`waylan_wayland::input_core`) 基于 Linux evdev 扫描码工作。本模块负责将 IBus 按键事件
//!   无损归一化为标准扫描码，并驱动 `AlgoEngine`、`ShiftToggleStateMachine` 与 `PunctuationProcessor`。
//! - **核心优势**：优先基于与键盘物理布局解耦的 `keyval` 进行映射，自动兼容 QWERTY、Dvorak、Colemak 及 XWayland 客户端，
//!   100% 复用微澜现有的拼音组词、V 模式、简繁转换及智能标点逻辑。
//! - **代价与局限**：成对标点补全的光标回退需通过 `ForwardKeyEvent` 模拟左方向键完成。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use waylan_core::{
    Candidate, CandidateIndexStyle, CandidateOrientation, FullWidthCharShortcut,
    FullWidthPunctShortcut, InputMode, TextAction, TraditionalShortcut,
};
use waylan_wayland::input_constants::*;

/// 字母 F 的 evdev 扫描码 (33)
pub const KEY_F: u32 = 33;
/// 字母 T 的 evdev 扫描码 (20)
pub const KEY_T: u32 = 20;
use waylan_wayland::input_core::{
    handle_composition_key_with_params, keycode_to_ascii_digit_char, keycode_to_char,
    keycode_to_punctuation_with_shift, CompositionKeyParams, ConsumedKeyTracker, KeyProcessAction,
    PageKeyConfig,
};
use waylan_wayland::session_common::{sync_mode_from_shared_flag, update_session_modifiers};

/// IBus Shift 修饰位掩码 (`IBUS_SHIFT_MASK`)
pub const IBUS_SHIFT_MASK: u32 = 1 << 0;
/// IBus Lock/CapsLock 修饰位掩码 (`IBUS_LOCK_MASK`)
pub const IBUS_LOCK_MASK: u32 = 1 << 1;
/// IBus Ctrl 修饰位掩码 (`IBUS_CONTROL_MASK`)
pub const IBUS_CONTROL_MASK: u32 = 1 << 2;
/// IBus Alt 修饰位掩码 (`IBUS_MOD1_MASK`)
pub const IBUS_MOD1_MASK: u32 = 1 << 3;
/// IBus Super 修饰位掩码 (`IBUS_SUPER_MASK`)
pub const IBUS_SUPER_MASK: u32 = 1 << 26;
/// IBus Hyper 修饰位掩码 (`IBUS_HYPER_MASK`)
pub const IBUS_HYPER_MASK: u32 = 1 << 27;
/// IBus Meta 修饰位掩码 (`IBUS_META_MASK`)
pub const IBUS_META_MASK: u32 = 1 << 28;
/// IBus 按键释放标志位掩码 (`IBUS_RELEASE_MASK`)
pub const IBUS_RELEASE_MASK: u32 = 1 << 30;

/// GDK/IBus 左方向键 KeySym (`IBUS_KEY_Left`)
pub const IBUS_KEYVAL_LEFT: u32 = 0xff51;
/// GDK/IBus 右方向键 KeySym (`IBUS_KEY_Right`)
pub const IBUS_KEYVAL_RIGHT: u32 = 0xff53;

/// 单次 IBus 按键或交互事件的决策输出
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IbusKeyOutcome {
    /// 是否由输入法拦截消费该按键
    pub consumed: bool,
    /// 需要立即提交上屏的文本（若有）
    pub commit_text: Option<String>,
    /// 是否需要刷新预编辑文本与候选词表信号
    pub update_ui: bool,
    /// 若发生中英文状态切换，记录切换后的新模式
    pub mode_changed: Option<InputMode>,
    /// 成对标点补全后需模拟方向键移动光标的偏移量（负数表示向左）
    pub cursor_offset: i32,
}

/// 微澜 IBus 会话内部核心状态机（受互斥锁保护，在 D-Bus 方法调用中同步推进）
pub struct IbusSessionCore {
    pub engine: AlgoEngine,
    pub shift_sm: ShiftToggleStateMachine,
    pub punct_processor: PunctuationProcessor,
    pub page_keys: PageKeyConfig,
    pub mode: InputMode,
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    pub shared_is_caps_locked: Option<Arc<AtomicBool>>,
    pub shared_traditional_chinese: Option<Arc<AtomicBool>>,
    pub shared_full_width_punct: Option<Arc<AtomicBool>>,
    pub shared_full_width_char: Option<Arc<AtomicBool>>,
    pub mode_notify_tx: Option<SyncSender<()>>,
    pub is_caps_locked: bool,
    pub mods_depressed: u32,
    pub phys_mods: u8,
    pub consumed_keys: ConsumedKeyTracker,
    pub is_password_context: bool,
    pub is_screen_locked: bool,
    pub surrounding_text: Option<(String, u32, u32)>,
    pub candidate_index_style: CandidateIndexStyle,
    pub candidate_orientation: CandidateOrientation,
    pub traditional_shortcut: TraditionalShortcut,
    pub full_width_char_shortcut: FullWidthCharShortcut,
    pub full_width_punct_shortcut: FullWidthPunctShortcut,
}

impl IbusSessionCore {
    /// 使用预加载的算法引擎构造 IBus 会话状态核心
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_sm: ShiftToggleStateMachine::new(),
            punct_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            shared_is_caps_locked: None,
            shared_traditional_chinese: None,
            shared_full_width_punct: None,
            shared_full_width_char: None,
            mode_notify_tx: None,
            is_caps_locked: false,
            mods_depressed: 0,
            phys_mods: 0,
            consumed_keys: ConsumedKeyTracker::new(),
            is_password_context: false,
            is_screen_locked: false,
            surrounding_text: None,
            candidate_index_style: CandidateIndexStyle::default(),
            candidate_orientation: CandidateOrientation::default(),
            traditional_shortcut: TraditionalShortcut::default(),
            full_width_char_shortcut: FullWidthCharShortcut::default(),
            full_width_punct_shortcut: FullWidthPunctShortcut::default(),
        }
    }

    /// 同步来自系统托盘的共享中英文状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        sync_mode_from_shared_flag(self.shared_is_chinese.as_deref(), &mut self.mode)
    }

    /// 同步来自托盘与全局状态的简繁与标点/字符全半角配置
    pub fn sync_shared_settings(&mut self) {
        if let Some(ref trad) = self.shared_traditional_chinese {
            let val = trad.load(Ordering::Acquire);
            if self.engine.config.traditional_chinese != val {
                self.engine.config.traditional_chinese = val;
            }
        }
        if let Some(ref punct) = self.shared_full_width_punct {
            let val = punct.load(Ordering::Acquire);
            if self.punct_processor.full_width_punct != val {
                self.punct_processor.full_width_punct = val;
            }
        }
    }

    /// 判定当前是否开启了全角字符/字母模式
    pub fn is_full_width_char(&self) -> bool {
        self.shared_full_width_char
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Acquire))
    }

    /// 重置当前预编辑与修饰键跟踪状态（用于失焦 `FocusOut` 或 `Reset`）
    pub fn reset_context(&mut self) {
        self.engine.reset();
        self.shift_sm.reset();
        self.mods_depressed = 0;
        self.phys_mods = 0;
        self.consumed_keys.clear();
        self.is_password_context = false;
        self.surrounding_text = None;
    }

    /// 获取当前页的候选词列表（若处于二级展开状态，则返回对应展开子项列表）
    pub fn current_candidates(&self) -> Vec<Candidate> {
        if self.engine.is_expanded {
            if let Some(cand) = self.engine.candidates().get(self.engine.selected_index) {
                if cand.is_expandable && !cand.sub_candidates.is_empty() {
                    let mut out = Vec::with_capacity(cand.sub_candidates.len());
                    for sub in &cand.sub_candidates {
                        out.push(Candidate {
                            text: sub.clone(),
                            pinyin: cand.pinyin.clone(),
                            score: cand.score,
                            is_expandable: false,
                            sub_candidates: Vec::new(),
                        });
                    }
                    return out;
                }
            }
        }
        let cands = self.engine.candidates();
        let mut out = Vec::with_capacity(cands.len());
        for c in cands {
            out.push(Candidate {
                text: c.text.clone(),
                pinyin: c.pinyin.clone(),
                score: c.score,
                is_expandable: c.is_expandable,
                sub_candidates: c.sub_candidates.clone(),
            });
        }
        out
    }

    /// 同步 CapsLock 大写锁定状态至共享标志并唤醒托盘刷新
    fn update_caps_lock_state(&mut self, is_locked: bool) {
        if self.is_caps_locked != is_locked {
            self.is_caps_locked = is_locked;
            if let Some(ref flag) = self.shared_is_caps_locked {
                flag.store(is_locked, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
        }
    }

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        self.sync_shared_settings();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);
        update_session_modifiers(
            &mut self.mods_depressed,
            &mut self.phys_mods,
            evdev_key,
            is_press,
        );

        // 密码框或系统锁屏状态防线：当处于密码输入框或屏幕锁定时，强制所有硬件按键 100% 直通操作系统，
        // 绝对不拦截按键、不生成预编辑拼音，杜绝锁屏密码框跳出拼音候选框泄露或阻碍密码输入。
        if self.is_screen_locked || self.is_password_context {
            self.shift_sm.reset();
            if !is_press {
                let was_consumed = self.consumed_keys.take_release(evdev_key);
                return IbusKeyOutcome {
                    consumed: was_consumed,
                    ..Default::default()
                };
            }
            self.consumed_keys.record_press(evdev_key, false);
            return IbusKeyOutcome::default();
        }

        // CapsLock 键处理：若当前存在未完成拼音，直接提交英文字符上屏并重置；按键原生透传
        if evdev_key == KEY_CAPSLOCK {
            self.shift_sm.reset();
            return if is_press {
                let mut outcome = IbusKeyOutcome {
                    consumed: false,
                    ..Default::default()
                };
                if !self.engine.raw_input().is_empty() {
                    let raw = self.engine.raw_input().to_string();
                    self.engine.reset();
                    outcome.commit_text = Some(raw);
                    outcome.update_ui = true;
                }
                let next_locked = !self.is_caps_locked;
                self.update_caps_lock_state(next_locked);

                // 仅当原本处于中文模式时，开启/关闭大写才需要驱动对外的中英显示模式切换；
                // 若原本处于英文模式，则始终保持英文模式，不触发中英文切换信号以防止状态栏闪烁
                if self.mode != InputMode::English {
                    let effective_mode = if next_locked {
                        InputMode::English
                    } else {
                        self.mode
                    };
                    outcome.mode_changed = Some(effective_mode);
                }
                self.consumed_keys.record_press(evdev_key, false);
                outcome
            } else {
                let was_consumed = self.consumed_keys.take_release(evdev_key);
                // 在按键释放阶段仅记录消费状态，不根据滞后的硬件 state 掩码重新翻转锁定状态，
                // 彻底杜绝松开按键时被旧状态覆盖而导致“必须按两下才能退出大写”
                IbusKeyOutcome {
                    consumed: was_consumed,
                    ..Default::default()
                }
            };
        }

        // 硬件修饰符校准：
        // 若客户端传入的 state 明确带有 IBUS_LOCK_MASK，则确保大写锁定置为激活；
        // 若 state 没有携带 IBUS_LOCK_MASK，绝不能直接盲目推断为“大写已关闭”，
        // 因为众多 Wayland/XWayland 客户端并不会在每个字符按键上传递 Lock 掩码，
        // 唯有当 evdev_key == KEY_CAPSLOCK 时由按键事件本身权威翻转锁定状态。
        if (state & IBUS_LOCK_MASK) != 0 && !self.is_caps_locked {
            self.update_caps_lock_state(true);
        }

        // 大写锁定期间严格重置并旁路 Shift 单击中英文切换状态机，防止输入大写符号时误篡改基准中英模式
        let toggled_mode = if self.is_caps_locked {
            self.shift_sm.reset();
            None
        } else {
            self.shift_sm.on_key_event(evdev_key, is_press, self.mode)
        };
        if !is_press {
            return self.handle_key_release(evdev_key, toggled_mode);
        }

        let shift_active = (state & IBUS_SHIFT_MASK) != 0 || (self.mods_depressed & MOD_SHIFT) != 0;
        let ctrl_active = (state & IBUS_CONTROL_MASK) != 0 || (self.mods_depressed & MOD_CTRL) != 0;
        let alt_active = (state & IBUS_MOD1_MASK) != 0 || (self.mods_depressed & MOD_ALT) != 0;
        let ctrl_or_alt = ibus_state_has_ctrl_or_alt(state)
            || (self.mods_depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0;

        if let Some(shortcut_outcome) =
            self.handle_global_shortcuts(evdev_key, shift_active, ctrl_active, alt_active)
        {
            self.consumed_keys.record_press(evdev_key, true);
            return shortcut_outcome;
        }

        let mut outcome =
            self.handle_key_press(evdev_key, shift_active, ctrl_or_alt, self.is_caps_locked);
        if self.is_full_width_char() {
            if let Some(ref text) = outcome.commit_text {
                outcome.commit_text = Some(waylan_core::to_full_width_string(text));
            }
        }
        self.consumed_keys.record_press(evdev_key, outcome.consumed);
        outcome
    }

    /// 匹配并执行用户配置的全局模式切换快捷键（简繁切换、字符全半角、标点全半角）
    fn handle_global_shortcuts(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
    ) -> Option<IbusKeyOutcome> {
        // 1. 简繁切换快捷键匹配
        let trad_match = match self.traditional_shortcut {
            TraditionalShortcut::CtrlShiftF => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_F
            }
            TraditionalShortcut::CtrlAltF => {
                ctrl_active && alt_active && !shift_active && evdev_key == KEY_F
            }
            TraditionalShortcut::CtrlShiftT => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_T
            }
            TraditionalShortcut::Disabled => false,
        };
        if trad_match {
            let next_val = !self.engine.config.traditional_chinese;
            self.engine.config.traditional_chinese = next_val;
            if let Some(ref flag) = self.shared_traditional_chinese {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                update_ui: !self.engine.raw_input().is_empty(),
                ..Default::default()
            });
        }

        // 2. 字符全半角切换快捷键匹配
        let char_match = match self.full_width_char_shortcut {
            FullWidthCharShortcut::ShiftSpace => {
                shift_active && !ctrl_active && !alt_active && evdev_key == KEY_SPACE
            }
            FullWidthCharShortcut::CtrlShiftSpace => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_SPACE
            }
            FullWidthCharShortcut::Disabled => false,
        };
        if char_match {
            let next_val = !self.is_full_width_char();
            if let Some(ref flag) = self.shared_full_width_char {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }

        // 3. 标点全半角切换快捷键匹配
        let punct_match = match self.full_width_punct_shortcut {
            FullWidthPunctShortcut::CtrlPeriod => {
                ctrl_active && !shift_active && !alt_active && evdev_key == KEY_DOT
            }
            FullWidthPunctShortcut::AltPeriod => {
                alt_active && !ctrl_active && !shift_active && evdev_key == KEY_DOT
            }
            FullWidthPunctShortcut::Disabled => false,
        };
        if punct_match {
            let next_val = !self.punct_processor.full_width_punct;
            self.punct_processor.full_width_punct = next_val;
            if let Some(ref flag) = self.shared_full_width_punct {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }

        None
    }

    /// 处理按键释放阶段（含 Shift/Ctrl 防误触中英文切换判定）
    fn handle_key_release(
        &mut self,
        evdev_key: u32,
        toggled_mode: Option<InputMode>,
    ) -> IbusKeyOutcome {
        let was_press_consumed = self.consumed_keys.take_release(evdev_key);
        let mut outcome = IbusKeyOutcome {
            consumed: was_press_consumed,
            ..Default::default()
        };

        if let Some(new_mode) = toggled_mode {
            self.mode = new_mode;
            outcome.mode_changed = Some(new_mode);
            if let Some(ref zh_flag) = self.shared_is_chinese {
                zh_flag.store(
                    matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                    Ordering::Release,
                );
            }
            if let Some(ref notify_tx) = self.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            if new_mode == InputMode::English && !self.engine.raw_input().is_empty() {
                let raw = self.engine.raw_input().to_string();
                self.engine.reset();
                outcome.commit_text = Some(raw);
                outcome.update_ui = true;
            }
        }
        outcome
    }

    /// 处理按键按下阶段的分发逻辑
    fn handle_key_press(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_or_alt: bool,
        is_caps_locked: bool,
    ) -> IbusKeyOutcome {
        if ctrl_or_alt || is_caps_locked {
            return IbusKeyOutcome::default();
        }

        // 若处于英文模式但开启了全角字符，直通字母转换为全角字符输出
        if self.mode == InputMode::English {
            if self.is_full_width_char() && !shift_active {
                if let Some(ch) = keycode_to_char(evdev_key) {
                    let full = waylan_core::to_full_width_char(ch);
                    return IbusKeyOutcome {
                        consumed: true,
                        commit_text: Some(full.to_string()),
                        ..Default::default()
                    };
                }
            }
            return IbusKeyOutcome::default();
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if !self.engine.raw_input().is_empty() {
            if let Some(outcome) = self.handle_active_composition_key(evdev_key, shift_active, now)
            {
                return outcome;
            }
        } else if let Some(outcome) = self.handle_idle_punctuation_key(evdev_key, shift_active) {
            return outcome;
        }

        if !shift_active {
            if let Some(ch) = keycode_to_char(evdev_key) {
                self.engine.push_char(ch);
                return IbusKeyOutcome {
                    consumed: true,
                    update_ui: true,
                    ..Default::default()
                };
            }
        }

        IbusKeyOutcome::default()
    }

    /// 处理正在组词状态（`raw_input` 非空）下的按键拦截
    fn handle_active_composition_key(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        now: u64,
    ) -> Option<IbusKeyOutcome> {
        let params = CompositionKeyParams {
            key: evdev_key,
            shift_active,
            page_keys: self.page_keys,
            now,
        };
        let action = handle_composition_key_with_params(
            &mut self.engine,
            &mut self.punct_processor,
            params,
        )?;

        match action {
            KeyProcessAction::PassThrough => None,
            KeyProcessAction::Handled | KeyProcessAction::Reset | KeyProcessAction::ExpandGrid => {
                Some(IbusKeyOutcome {
                    consumed: true,
                    update_ui: true,
                    ..Default::default()
                })
            }
            KeyProcessAction::Commit(committed)
            | KeyProcessAction::CommitPartial { committed, .. } => Some(IbusKeyOutcome {
                consumed: true,
                commit_text: Some(committed),
                update_ui: true,
                ..Default::default()
            }),
        }
    }

    /// 处理空闲状态（`raw_input` 为空）下的中文标点转换与数字保护
    fn handle_idle_punctuation_key(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
    ) -> Option<IbusKeyOutcome> {
        if let Some(punct_char) = keycode_to_punctuation_with_shift(evdev_key, shift_active) {
            let actions_opt = self.punct_processor.process_punctuation(punct_char);
            if let Some(actions) = actions_opt {
                let mut committed_out = None;
                let mut cursor_offset = 0;
                for act in actions {
                    match act {
                        TextAction::Commit(text) => {
                            self.punct_processor.record_commit(&text);
                            committed_out = Some(text);
                        }
                        TextAction::MoveCursor(offset) => {
                            cursor_offset += offset;
                        }
                        _ => {}
                    }
                }
                return Some(IbusKeyOutcome {
                    consumed: true,
                    commit_text: committed_out,
                    cursor_offset,
                    ..Default::default()
                });
            }
        }

        if !shift_active {
            if let Some(digit_ch) = keycode_to_ascii_digit_char(evdev_key) {
                self.punct_processor.record_commit(&digit_ch.to_string());
            }
        }
        None
    }
}

/// 判断 IBus 修饰位掩码是否包含 Ctrl / Alt / Super / Meta 修饰键
#[inline]
pub fn ibus_state_has_ctrl_or_alt(state: u32) -> bool {
    (state
        & (IBUS_CONTROL_MASK | IBUS_MOD1_MASK | IBUS_SUPER_MASK | IBUS_HYPER_MASK | IBUS_META_MASK))
        != 0
}

/// 将 IBus/GDK `keyval` 归一化为 Linux evdev 扫描码（未命中时回退至 `fallback_keycode`）
pub fn ibus_keyval_to_evdev(keyval: u32, fallback_keycode: u32) -> u32 {
    if let Some(code) = map_alpha_or_digit_keyval(keyval) {
        return code;
    }
    if let Some(code) = map_punct_or_control_keyval(keyval) {
        return code;
    }
    fallback_keycode
}

/// 映射字母与数字相关的 `keyval` 到 evdev 扫描码
fn map_alpha_or_digit_keyval(keyval: u32) -> Option<u32> {
    let lower = if (0x41..=0x5a).contains(&keyval) {
        keyval + 32
    } else {
        keyval
    };
    let code = match lower {
        0x61 => 30, // a
        0x62 => 48, // b
        0x63 => 46, // c
        0x64 => 32, // d
        0x65 => 18, // e
        0x66 => 33, // f
        0x67 => 34, // g
        0x68 => 35, // h
        0x69 => 23, // i
        0x6a => 36, // j
        0x6b => 37, // k
        0x6c => 38, // l
        0x6d => 50, // m
        0x6e => 49, // n
        0x6f => 24, // o
        0x70 => 25, // p
        0x71 => 16, // q
        0x72 => 19, // r
        0x73 => 31, // s
        0x74 => 20, // t
        0x75 => 22, // u
        0x76 => 47, // v
        0x77 => 17, // w
        0x78 => 45, // x
        0x79 => 21, // y
        0x7a => 44, // z
        0x31 | 0x21 => KEY_1,
        0x32 | 0x40 => KEY_2,
        0x33 | 0x23 => KEY_3,
        0x34 | 0x24 => KEY_4,
        0x35 | 0x25 => KEY_5,
        0x36 | 0x5e => KEY_6,
        0x37 | 0x26 => KEY_7,
        0x38 | 0x2a => KEY_8,
        0x39 | 0x28 => KEY_9,
        0x30 | 0x29 => KEY_0,
        _ => return None,
    };
    Some(code)
}

/// 映射标点、导航、修饰键及小键盘的 `keyval` 到 evdev 扫描码
fn map_punct_or_control_keyval(keyval: u32) -> Option<u32> {
    let code = match keyval {
        0x0020 => KEY_SPACE,
        0x002d | 0x005f => KEY_MINUS,
        0x003d | 0x002b => KEY_EQUAL,
        0x005b | 0x007b => KEY_LEFTBRACE,
        0x005d | 0x007d => KEY_RIGHTBRACE,
        0x003b | 0x003a => KEY_SEMICOLON,
        0x0027 | 0x0022 => KEY_APOSTROPHE,
        0x0060 | 0x007e => KEY_GRAVE,
        0x002c | 0x003c => KEY_COMMA,
        0x002e | 0x003e => KEY_DOT,
        0x002f | 0x003f => KEY_SLASH,
        0xff08 => KEY_BACKSPACE,
        0xff09 | 0xfe20 => KEY_TAB,
        0xff0d => KEY_ENTER,
        0xff8d => KEY_KPENTER,
        0xff1b => KEY_ESC,
        0xffff => KEY_DELETE,
        0xff51 => KEY_LEFT,
        0xff52 => KEY_UP,
        0xff53 => KEY_RIGHT,
        0xff54 => KEY_DOWN,
        0xff55 => KEY_PAGEUP,
        0xff56 => KEY_PAGEDOWN,
        0xffe1 => KEY_LEFTSHIFT,
        0xffe2 => KEY_RIGHTSHIFT,
        0xffe3 => KEY_LEFTCTRL,
        0xffe4 => KEY_RIGHTCTRL,
        0xffe9 => KEY_LEFTALT,
        0xffea => KEY_RIGHTALT,
        0xffeb => KEY_LEFTMETA,
        0xffec => KEY_RIGHTMETA,
        0xffe5 | 0xffe6 => KEY_CAPSLOCK,
        0xffb0 => KEY_KP0,
        0xffb1 => KEY_KP1,
        0xffb2 => KEY_KP2,
        0xffb3 => KEY_KP3,
        0xffb4 => KEY_KP4,
        0xffb5 => KEY_KP5,
        0xffb6 => KEY_KP6,
        0xffb7 => KEY_KP7,
        0xffb8 => KEY_KP8,
        0xffb9 => KEY_KP9,
        0xffaa => KEY_KPASTERISK,
        0xffab => KEY_KPPLUS,
        0xffad => KEY_KPMINUS,
        0xffae => KEY_KPDOT,
        0xffaf => KEY_KPSLASH,
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_algo::types::AlgoConfig;

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
        assert_eq!(core.engine.raw_input(), "xian");

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
        assert_eq!(core.engine.selected_index, 0);

        // 2. 按 Tab 键触发二级展开
        let tab_out = core.process_key_event(0xff09, KEY_TAB, 0);
        assert!(tab_out.consumed);
        assert!(tab_out.update_ui);
        assert!(core.engine.is_expanded);

        // 3. 展开后 current_candidates 应返回全部 7 种细分日期格式
        let expanded_cands = core.current_candidates();
        assert_eq!(expanded_cands.len(), 7);
        assert!(!expanded_cands[0].is_expandable);

        // 4. 按数字键 2 选取第 2 种格式上屏
        let num_out = core.process_key_event(b'2' as u32, KEY_2, 0);
        assert!(num_out.consumed);
        assert_eq!(num_out.commit_text, Some(expanded_cands[1].text.clone()));
        assert!(!core.engine.is_expanded);
        assert!(core.engine.raw_input().is_empty());
    }

    #[test]
    fn test_ibus_caps_lock_passthrough() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);

        // 1. 中文状态输入 'a', 'i'
        for ch in ['a', 'i'] {
            let out = core.process_key_event(ch as u32, 0, 0);
            assert!(out.consumed);
        }
        assert_eq!(core.engine.raw_input(), "ai");

        // 2. 按下 CapsLock 键：未完成拼音立即上屏，按键本身不被消费（透传给应用），且通知模式变更为英文
        let caps_out = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
        assert!(!caps_out.consumed, "CapsLock 键本身不应被消费");
        assert_eq!(caps_out.commit_text.as_deref(), Some("ai"));
        assert_eq!(
            caps_out.mode_changed,
            Some(InputMode::English),
            "CapsLock 激活时模式应指示为英文"
        );
        assert!(core.engine.raw_input().is_empty());

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
        assert_eq!(core.engine.raw_input(), "n");
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
            core.mode,
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
        assert_eq!(core.engine.raw_input(), "h");
    }

    #[test]
    fn test_ibus_caps_lock_in_english_mode_no_flicker() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);
        core.mode = InputMode::English;

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
        assert_eq!(core.mode, InputMode::English, "始终保持纯英文模式");
    }

    #[test]
    fn test_ibus_caps_lock_tray_flags_and_subsequent_keys() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);
        let zh_flag = Arc::new(AtomicBool::new(true));
        let caps_flag = Arc::new(AtomicBool::new(false));
        core.shared_is_chinese = Some(zh_flag.clone());
        core.shared_is_caps_locked = Some(caps_flag.clone());

        // 1. 中文状态下按下 CapsLock：大写标志权威置为 true（托盘读取该标志直接展现 A 大写图标与大写菜单项）
        let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
        assert!(core.is_caps_locked);
        assert!(caps_flag.load(Ordering::Acquire));
        assert_eq!(core.mode, InputMode::Chinese, "基准中文模式状态不受篡改");

        // 2. 随后的普通字符按键（即使客户端 state 未附带 IBUS_LOCK_MASK），大写状态绝不可被误冲刷
        let letter = core.process_key_event(b'A' as u32, 0, 0);
        assert!(!letter.consumed, "大写锁定下普通字母按键应原生透传");
        assert!(
            core.is_caps_locked,
            "后续未带Lock掩码的按键绝不能冲刷掉大写状态"
        );
        assert!(caps_flag.load(Ordering::Acquire));

        // 3. 再次按下 CapsLock 退出大写：大写标志关闭，无缝回到中文模式
        let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
        assert!(!core.is_caps_locked);
        assert!(!caps_flag.load(Ordering::Acquire));
        assert_eq!(
            core.mode,
            InputMode::Chinese,
            "退出大写锁定后丝滑保持中文模式"
        );
        assert!(zh_flag.load(Ordering::Acquire));
    }

    #[test]
    fn test_ibus_shared_settings_sync_and_full_width() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);
        let trad_flag = Arc::new(AtomicBool::new(false));
        let punct_flag = Arc::new(AtomicBool::new(true));
        let char_flag = Arc::new(AtomicBool::new(false));

        core.shared_traditional_chinese = Some(trad_flag.clone());
        core.shared_full_width_punct = Some(punct_flag.clone());
        core.shared_full_width_char = Some(char_flag.clone());

        // 默认状态：简体、全角标点、半角字母
        assert!(!core.engine.config.traditional_chinese);
        assert!(core.punct_processor.full_width_punct);
        assert!(!core.is_full_width_char());

        // 1. 托盘触发切换为繁体
        trad_flag.store(true, Ordering::Release);
        let _ = core.process_key_event(0, 0, 0);
        assert!(
            core.engine.config.traditional_chinese,
            "引擎应自动同步繁体设置"
        );

        // 2. 托盘触发切换为半角标点
        punct_flag.store(false, Ordering::Release);
        let outcome = core.process_key_event(b',' as u32, 51, 0);
        assert_eq!(
            outcome.commit_text,
            Some(",".to_string()),
            "半角标点模式下应输出半角逗号"
        );

        // 3. 托盘触发切换为全角字符，并在英文模式下敲击字母
        char_flag.store(true, Ordering::Release);
        core.mode = InputMode::English;
        let letter_outcome = core.process_key_event(b'a' as u32, 30, 0);
        assert_eq!(
            letter_outcome.commit_text,
            Some("ａ".to_string()),
            "全角字符模式下英文应输出全角字母"
        );
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
        assert!(core.engine.raw_input().is_empty());

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
        assert_eq!(core.engine.raw_input(), "n");
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
            core.engine.raw_input().is_empty(),
            "锁屏状态下严禁将按键录入算法引擎"
        );

        // 2. 模拟解锁屏幕回到桌面
        core.is_screen_locked = false;
        let outcome2 = core.process_key_event(b'a' as u32, 30, 0);
        assert!(
            outcome2.consumed,
            "解锁屏幕后应立即恢复拼音拦截与候选词处理"
        );
        assert_eq!(core.engine.raw_input(), "a");
    }

    #[test]
    fn test_ibus_global_shortcuts_toggle() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);
        let trad_flag = Arc::new(AtomicBool::new(false));
        let punct_flag = Arc::new(AtomicBool::new(true));
        let char_flag = Arc::new(AtomicBool::new(false));

        core.shared_traditional_chinese = Some(trad_flag.clone());
        core.shared_full_width_punct = Some(punct_flag.clone());
        core.shared_full_width_char = Some(char_flag.clone());

        // 默认快捷键：Ctrl+Shift+F (简繁), Shift+Space (字符全半角), Ctrl+. (标点全半角)
        assert_eq!(core.traditional_shortcut, TraditionalShortcut::CtrlShiftF);
        assert_eq!(
            core.full_width_char_shortcut,
            FullWidthCharShortcut::ShiftSpace
        );
        assert_eq!(
            core.full_width_punct_shortcut,
            FullWidthPunctShortcut::CtrlPeriod
        );

        // 1. 测试简繁切换快捷键 (Ctrl + Shift + F)
        let state_ctrl_shift = IBUS_CONTROL_MASK | IBUS_SHIFT_MASK;
        let res_trad = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
        assert!(res_trad.consumed, "简繁快捷键必须被输入法消费");
        assert!(
            core.engine.config.traditional_chinese,
            "简繁状态应切换为繁体"
        );
        assert!(
            trad_flag.load(Ordering::Acquire),
            "共享托盘标志应同步为繁体"
        );

        // 再次按下切回简体
        let res_trad2 = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
        assert!(res_trad2.consumed);
        assert!(
            !core.engine.config.traditional_chinese,
            "简繁状态应切回简体"
        );
        assert!(!trad_flag.load(Ordering::Acquire));

        // 2. 测试字符全半角切换快捷键 (Shift + Space)
        let res_char = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
        assert!(res_char.consumed, "字符全半角快捷键必须被消费");
        assert!(core.is_full_width_char(), "字符全半角状态应切换为全角");
        assert!(char_flag.load(Ordering::Acquire));

        // 再次按下切回半角
        let res_char2 = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
        assert!(res_char2.consumed);
        assert!(!core.is_full_width_char(), "字符全半角状态应切回半角");
        assert!(!char_flag.load(Ordering::Acquire));

        // 3. 测试标点全半角切换快捷键 (Ctrl + .)
        let res_punct = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
        assert!(res_punct.consumed, "标点全半角快捷键必须被消费");
        assert!(
            !core.punct_processor.full_width_punct,
            "标点状态应由全角切换为半角"
        );
        assert!(!punct_flag.load(Ordering::Acquire));

        // 再次按下切回全角
        let res_punct2 = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
        assert!(res_punct2.consumed);
        assert!(
            core.punct_processor.full_width_punct,
            "标点状态应切回全角中文标点"
        );
        assert!(punct_flag.load(Ordering::Acquire));
    }
}
