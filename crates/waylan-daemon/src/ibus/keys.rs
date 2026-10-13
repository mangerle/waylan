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
use waylan_core::{Candidate, InputMode, TextAction};
use waylan_wayland::input_constants::*;
use waylan_wayland::input_core::{
    handle_composition_key_with_params, keycode_to_ascii_digit_char, keycode_to_char,
    keycode_to_punctuation_with_shift, CompositionKeyParams, ConsumedKeyTracker, KeyProcessAction,
    PageKeyConfig,
};
use waylan_wayland::session_common::{sync_mode_from_shared_flag, update_session_modifiers};

/// IBus Shift 修饰位掩码 (`IBUS_SHIFT_MASK`)
pub const IBUS_SHIFT_MASK: u32 = 1 << 0;
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
    pub mode_notify_tx: Option<SyncSender<()>>,
    pub mods_depressed: u32,
    pub phys_mods: u8,
    pub consumed_keys: ConsumedKeyTracker,
    pub is_password_context: bool,
    pub surrounding_text: Option<(String, u32, u32)>,
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
            mode_notify_tx: None,
            mods_depressed: 0,
            phys_mods: 0,
            consumed_keys: ConsumedKeyTracker::new(),
            is_password_context: false,
            surrounding_text: None,
        }
    }

    /// 同步来自系统托盘的共享中英文状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        sync_mode_from_shared_flag(self.shared_is_chinese.as_deref(), &mut self.mode)
    }

    /// 重置当前预编辑与修饰键跟踪状态（用于失焦 `FocusOut` 或 `Reset`）
    pub fn reset_context(&mut self) {
        self.engine.reset();
        self.shift_sm.reset();
        self.mods_depressed = 0;
        self.phys_mods = 0;
        self.consumed_keys.clear();
    }

    /// 获取当前页的候选词列表（转换为 `waylan_core::Candidate`）
    pub fn current_candidates(&self) -> Vec<Candidate> {
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

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);
        update_session_modifiers(
            &mut self.mods_depressed,
            &mut self.phys_mods,
            evdev_key,
            is_press,
        );

        if self.is_password_context {
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

        let toggled_mode = self.shift_sm.on_key_event(evdev_key, is_press, self.mode);
        if !is_press {
            return self.handle_key_release(evdev_key, toggled_mode);
        }

        let shift_active = (state & IBUS_SHIFT_MASK) != 0 || (self.mods_depressed & MOD_SHIFT) != 0;
        let ctrl_or_alt = ibus_state_has_ctrl_or_alt(state)
            || (self.mods_depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0;

        let outcome = self.handle_key_press(evdev_key, shift_active, ctrl_or_alt);
        self.consumed_keys.record_press(evdev_key, outcome.consumed);
        outcome
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
    ) -> IbusKeyOutcome {
        if ctrl_or_alt || self.mode == InputMode::English {
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
            KeyProcessAction::Handled | KeyProcessAction::Reset => Some(IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            }),
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
}
