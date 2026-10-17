//! IBus 按键事件核心分发与状态流转
//!
//! # 设计原理
//! - **实现初衷**：将归一化后的 evdev 扫描码分发给密码模式过滤、CapsLock、快捷键、组词中输入与标点处理器。
//! - **核心优势**：职责高度分离，每个按键处理分支单一清晰，遵循“微秒级纯内存处理”准则。
//! - **代价与局限**：对多修饰键组合需统一通过 update_modifier_masks 状态跟踪。

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use waylan_algo::{extract_surrounding_prefix, ImeAction, ShortcutToggleAction};
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
    /// 检查并处理系统锁屏或密码输入框状态下的按键直通逻辑
    pub(crate) fn handle_lock_or_password_state(
        &mut self,
        evdev_key: u32,
        is_press: bool,
    ) -> Option<IbusKeyOutcome> {
        if !self.is_screen_locked && !self.is_password_context {
            return None;
        }
        self.core.shift_sm.reset();
        if !is_press {
            let was_consumed = self.core.consumed_keys.take_release(evdev_key);
            return Some(IbusKeyOutcome {
                consumed: was_consumed,
                ..Default::default()
            });
        }
        self.core.consumed_keys.record_press(evdev_key, false);
        Some(IbusKeyOutcome::default())
    }

    /// 处理 CapsLock 键按下与释放状态变迁
    pub(crate) fn handle_capslock_event(
        &mut self,
        _evdev_key: u32,
        is_press: bool,
    ) -> IbusKeyOutcome {
        self.core.shift_sm.reset();
        if is_press {
            let (was_consumed, committed) = self.core.handle_capslock(true);
            let next_locked = !self.is_caps_locked;
            self.update_caps_lock_state(next_locked);

            let mut outcome = IbusKeyOutcome {
                consumed: was_consumed,
                commit_text: committed,
                update_ui: true,
                ..Default::default()
            };

            if self.core.mode != InputMode::English {
                let effective_mode = if next_locked {
                    InputMode::English
                } else {
                    self.core.mode
                };
                outcome.mode_changed = Some(effective_mode);
            }
            outcome
        } else {
            let (was_consumed, _) = self.core.handle_capslock(false);
            IbusKeyOutcome {
                consumed: was_consumed,
                ..Default::default()
            }
        }
    }

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

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        self.sync_shared_settings();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);
        update_modifier_masks(
            &mut self.core.modifiers.depressed,
            &mut self.phys_mods,
            evdev_key,
            is_press,
        );

        if let Some(outcome) = self.handle_lock_or_password_state(evdev_key, is_press) {
            return outcome;
        }

        if evdev_key == KEY_CAPSLOCK {
            return self.handle_capslock_event(evdev_key, is_press);
        }

        if (state & IBUS_LOCK_MASK) != 0 && !self.is_caps_locked {
            self.update_caps_lock_state(true);
        }

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

        if !is_press {
            if let Some(outcome) = self.handle_voice_key(evdev_key, false, event_mods) {
                return outcome;
            }
            return self.handle_key_release(evdev_key);
        }

        if self.is_caps_locked {
            self.core.shift_sm.reset();
        } else {
            self.core
                .shift_sm
                .on_key_event(evdev_key, true, self.core.mode);
        }

        if let Some(voice_outcome) = self.handle_voice_key(evdev_key, true, event_mods) {
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
        let action = self.core.handle_key_press(evdev_key, now, surrounding_ctx);
        self.core.modifiers.depressed = saved_mods;

        let outcome = match action {
            ImeAction::PassThrough => IbusKeyOutcome {
                consumed: false,
                ..Default::default()
            },
            ImeAction::Handled => IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            },
            ImeAction::ExpandGrid => IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            },
            ImeAction::Commit(text) => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(text),
                update_ui: true,
                ..Default::default()
            },
            ImeAction::CommitPunctuation {
                text,
                cursor_offset,
            } => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(text),
                cursor_offset,
                update_ui: true,
                ..Default::default()
            },
            ImeAction::CommitPartial { committed, .. } => IbusKeyOutcome {
                consumed: true,
                commit_text: Some(committed),
                update_ui: true,
                ..Default::default()
            },
            ImeAction::Reset => IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            },
            ImeAction::ShortcutToggled(toggle) => {
                match toggle {
                    ShortcutToggleAction::Traditional(trad) => {
                        if let Some(ref flag) = self.shared_traditional_chinese {
                            flag.store(trad, Ordering::Release);
                        }
                    }
                    ShortcutToggleAction::FullWidthChar(fw) => {
                        if let Some(ref flag) = self.shared_full_width_char {
                            flag.store(fw, Ordering::Release);
                        }
                    }
                    ShortcutToggleAction::FullWidthPunct(fw) => {
                        if let Some(ref flag) = self.shared_full_width_punct {
                            flag.store(fw, Ordering::Release);
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
        };

        self.core
            .consumed_keys
            .record_press(evdev_key, outcome.consumed);
        outcome
    }

    /// 处理按键释放阶段（含 Shift/Ctrl 防误触中英文切换判定）
    pub(crate) fn handle_key_release(&mut self, evdev_key: u32) -> IbusKeyOutcome {
        if self.is_caps_locked {
            self.core.shift_sm.reset();
            let was_press_consumed = self.core.consumed_keys.take_release(evdev_key);
            return IbusKeyOutcome {
                consumed: was_press_consumed,
                ..Default::default()
            };
        }

        let (was_press_consumed, toggled_mode, committed) = self.core.handle_key_release(evdev_key);
        let mut outcome = IbusKeyOutcome {
            consumed: was_press_consumed,
            ..Default::default()
        };

        if let Some(new_mode) = toggled_mode {
            self.core.mode = new_mode;
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
            if let Some(raw) = committed {
                outcome.commit_text = Some(raw);
                outcome.update_ui = true;
            }
        }
        outcome
    }
}
