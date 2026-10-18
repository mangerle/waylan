//! IBus 按键事件核心分发与协议适配
//!
//! # 设计原理
//! - **实现初衷**：将 IBus 协议层传入的 (keyval, keycode, state) 转换为标准 evdev 扫描码，
//!   委托统一的 [`ImeCoreSession::process_key`] 决策流水线处理，并将统一响应映射为 IBus 上屏与 D-Bus 信号。
//! - **核心优势**：彻底消除 IBus 与 Wayland 端按键处理逻辑的不一致，使 IBus 端降级为纯粹的协议转接器；
//!   密码框保护、CapsLock 截断提交、中英模式切换与全局快捷键全部由核心状态机单一真实源驱动。
//! - **代价与局限**：需将 X11 修饰键状态位映射至统一修饰键位掩码。

use std::time::{SystemTime, UNIX_EPOCH};

use waylan_algo::{
    extract_surrounding_prefix, KeyProcessContext, KeyProcessResult, ShortcutToggleAction,
};
use waylan_core::input::modifiers::update_modifier_masks;
use waylan_core::input::*;
use waylan_core::InputMode;
use waylan_voice::VoiceKeyAction;

use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::keyval::{
    ibus_keyval_to_evdev, IBUS_CONTROL_MASK, IBUS_LOCK_MASK, IBUS_MOD1_MASK, IBUS_RELEASE_MASK,
    IBUS_SHIFT_MASK, IBUS_SUPER_MASK,
};

impl IbusSessionCore {
    /// 统一调用 VoiceInteractionManager 处理语音按键交互（启动、停止、断句、取消）
    pub(crate) fn handle_voice_key(
        &mut self,
        evdev_key: u32,
        is_press: bool,
        effective_mods: u32,
    ) -> Option<IbusKeyOutcome> {
        let action = self
            .voice_manager
            .handle_key_event(evdev_key, is_press, effective_mods);
        match action {
            VoiceKeyAction::PassThrough => None,
            VoiceKeyAction::Consumed => {
                if is_press {
                    self.core.consumed_keys.record_press(evdev_key, true);
                    if !self.core.engine.raw_input().is_empty() {
                        self.core.engine.reset();
                    }
                } else {
                    self.core.consumed_keys.take_release(evdev_key);
                }
                Some(IbusKeyOutcome {
                    consumed: true,
                    update_ui: true,
                    ..Default::default()
                })
            }
            VoiceKeyAction::Commit(text) => {
                if is_press {
                    self.core.consumed_keys.record_press(evdev_key, true);
                    if !self.core.engine.raw_input().is_empty() {
                        self.core.engine.reset();
                    }
                } else {
                    self.core.consumed_keys.take_release(evdev_key);
                }
                Some(IbusKeyOutcome {
                    consumed: true,
                    commit_text: Some(text),
                    update_ui: true,
                    ..Default::default()
                })
            }
        }
    }

    /// 同步并计算最终的修饰键位掩码与 CapsLock 状态
    fn sync_and_calculate_modifiers(&mut self, evdev_key: u32, is_press: bool, state: u32) -> u32 {
        update_modifier_masks(
            &mut self.core.modifiers.depressed,
            &mut self.phys_mods,
            evdev_key,
            is_press,
        );

        let mut event_mods = self.core.modifiers.depressed;
        if (state & IBUS_SHIFT_MASK) != 0 {
            event_mods |= MOD_SHIFT;
        }
        if (state & IBUS_CONTROL_MASK) != 0 {
            event_mods |= MOD_CTRL;
        }
        if (state & IBUS_MOD1_MASK) != 0 {
            event_mods |= MOD_ALT;
        }
        if (state & IBUS_SUPER_MASK) != 0 {
            event_mods |= MOD_SUPER;
        }

        if (state & IBUS_LOCK_MASK) != 0 && !self.is_caps_locked {
            self.update_caps_lock_state(true);
        }

        event_mods
    }

    /// 处理 CapsLock 键切换时的状态更新并计算生效的模式指示
    fn handle_caps_lock_toggle(&mut self, evdev_key: u32, is_press: bool) -> Option<InputMode> {
        if evdev_key != KEY_CAPSLOCK || !is_press {
            return None;
        }

        let next_locked = !self.is_caps_locked;
        self.update_caps_lock_state(next_locked);
        if self.core.mode != InputMode::English {
            let effective_mode = if next_locked {
                InputMode::English
            } else {
                self.core.mode
            };
            Some(effective_mode)
        } else {
            None
        }
    }

    /// 将算法引擎统一按键决策结果映射为 IBus 输出
    fn map_key_process_result(
        &mut self,
        result: KeyProcessResult,
        caps_mode_changed: Option<InputMode>,
    ) -> IbusKeyOutcome {
        match result {
            KeyProcessResult::PassThrough => IbusKeyOutcome {
                consumed: false,
                mode_changed: caps_mode_changed,
                update_ui: caps_mode_changed.is_some(),
                ..Default::default()
            },
            KeyProcessResult::Handled { update_ui } => IbusKeyOutcome {
                consumed: true,
                update_ui,
                ..Default::default()
            },
            KeyProcessResult::ExpandGrid => IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::Commit(text) => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(text),
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::CommitAndPassThrough(text) => IbusKeyOutcome {
                consumed: false,
                commit_text: Some(text),
                mode_changed: caps_mode_changed,
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::CommitPunctuation {
                text,
                cursor_offset,
            } => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(text),
                cursor_offset,
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::CommitPartial { committed, .. } => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(committed),
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::Reset => IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            },
            KeyProcessResult::ShortcutToggled(toggle) => {
                if let Some(ref flags) = self.shared_flags {
                    match toggle {
                        ShortcutToggleAction::Traditional(trad) => {
                            flags.set_traditional(trad);
                        }
                        ShortcutToggleAction::FullWidthChar(fw) => {
                            flags.set_full_width_char(fw);
                        }
                        ShortcutToggleAction::FullWidthPunct(fw) => {
                            flags.set_full_width_punct(fw);
                        }
                    }
                }
                if let Some(ref tx) = self.mode_notify_tx {
                    let _ = tx.try_send(());
                }
                IbusKeyOutcome {
                    consumed: true,
                    update_ui: !self.core.engine.raw_input().is_empty(),
                    ..Default::default()
                }
            }
            KeyProcessResult::ModeChanged {
                new_mode,
                committed,
                was_consumed,
            } => {
                if let Some(ref flags) = self.shared_flags {
                    flags.set_chinese(matches!(new_mode, InputMode::Chinese | InputMode::VMode));
                }
                if let Some(ref notify_tx) = self.mode_notify_tx {
                    let _ = notify_tx.try_send(());
                }
                IbusKeyOutcome {
                    consumed: was_consumed,
                    mode_changed: Some(new_mode),
                    commit_text: committed,
                    update_ui: true,
                    ..Default::default()
                }
            }
        }
    }

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        self.sync_shared_settings();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);

        let event_mods = self.sync_and_calculate_modifiers(evdev_key, is_press, state);

        // 语音听写优先拦截
        if let Some(voice_outcome) = self.handle_voice_key(evdev_key, is_press, event_mods) {
            return voice_outcome;
        }

        // 提取周围文本安全有效上下文切片
        let surrounding_ctx = extract_surrounding_prefix(self.surrounding_text.as_ref(), 64);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let saved_mods = self.core.modifiers.depressed;
        self.core.modifiers.depressed = event_mods;
        let result = self.core.process_key(
            KeyProcessContext {
                keycode: evdev_key,
                is_press,
                is_password_or_locked: self.is_screen_locked || self.is_password_context,
                surrounding_prefix: surrounding_ctx,
            },
            now,
        );
        self.core.modifiers.depressed = saved_mods;

        let caps_mode_changed = self.handle_caps_lock_toggle(evdev_key, is_press);
        self.map_key_process_result(result, caps_mode_changed)
    }
}
