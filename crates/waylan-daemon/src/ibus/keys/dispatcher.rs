//! IBus 按键事件核心分发与状态流转
//!
//! # 设计原理
//! - **实现初衷**：将归一化后的 evdev 扫描码分发给密码模式过滤、CapsLock、快捷键、组词中输入与标点处理器。
//! - **核心优势**：职责高度分离，每个按键处理分支单一清晰，遵循“微秒级纯内存处理”准则。
//! - **代价与局限**：对多修饰键组合需统一通过 update_session_modifiers 状态跟踪。

use log::info;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use waylan_core::{to_full_width_char, to_full_width_string, InputMode, TextAction};
use waylan_wayland::input_constants::*;
use waylan_wayland::input_core::{
    handle_composition_key_with_params, keycode_to_ascii_digit_char, keycode_to_char,
    keycode_to_punctuation_with_shift, CompositionKeyParams, KeyProcessAction,
};
use waylan_wayland::session_common::update_session_modifiers;

use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::keyval::{
    ibus_keyval_to_evdev, ibus_state_has_ctrl_or_alt, IBUS_CONTROL_MASK, IBUS_LOCK_MASK,
    IBUS_MOD1_MASK, IBUS_RELEASE_MASK, IBUS_SHIFT_MASK, IBUS_SUPER_MASK,
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
        self.shift_sm.reset();
        if !is_press {
            let was_consumed = self.consumed_keys.take_release(evdev_key);
            return Some(IbusKeyOutcome {
                consumed: was_consumed,
                ..Default::default()
            });
        }
        self.consumed_keys.record_press(evdev_key, false);
        Some(IbusKeyOutcome::default())
    }

    /// 处理 CapsLock 键按下与释放状态变迁
    pub(crate) fn handle_capslock_event(
        &mut self,
        evdev_key: u32,
        is_press: bool,
    ) -> IbusKeyOutcome {
        self.shift_sm.reset();
        if is_press {
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
            IbusKeyOutcome {
                consumed: was_consumed,
                ..Default::default()
            }
        }
    }

    /// 处理对讲机长按模式松开事件
    pub(crate) fn handle_push_to_talk_release(&mut self, evdev_key: u32) -> Option<IbusKeyOutcome> {
        if !self.is_push_to_talk_active || !self.voice_controller.is_recording() {
            return None;
        }
        let is_main_key_release = self.push_to_talk_key == Some(evdev_key);
        let is_modifier_release = matches!(
            evdev_key,
            KEY_LEFTCTRL
                | KEY_RIGHTCTRL
                | KEY_LEFTALT
                | KEY_RIGHTALT
                | KEY_LEFTSHIFT
                | KEY_RIGHTSHIFT
                | KEY_LEFTMETA
                | KEY_RIGHTMETA
        );
        if is_main_key_release || is_modifier_release {
            info!("[微澜 IBus 语音] 对讲机模式快捷键松开，自动断句上屏提交");
            let commit_text = self.stop_voice_recording_and_commit();
            let was_consumed = self.consumed_keys.take_release(evdev_key);
            return Some(IbusKeyOutcome {
                consumed: was_consumed || is_main_key_release,
                commit_text,
                update_ui: true,
                ..Default::default()
            });
        }
        None
    }

    /// 录音活跃状态下的按键拦截处理（Esc 取消，Enter 提交，其余静默消费）
    pub(crate) fn handle_voice_recording_key(&mut self, evdev_key: u32) -> Option<IbusKeyOutcome> {
        if !self.voice_controller.is_recording() {
            return None;
        }
        if evdev_key == KEY_ESC {
            self.voice_controller.cancel();
            self.clear_voice_state();
            self.consumed_keys.record_press(evdev_key, true);
            return Some(IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            });
        }
        if evdev_key == KEY_ENTER {
            info!("[微澜 IBus 语音] Enter 键触发停止录音并请求提交");
            let commit_text = self.stop_voice_recording_and_commit();
            self.consumed_keys.record_press(evdev_key, true);
            return Some(IbusKeyOutcome {
                consumed: true,
                commit_text,
                update_ui: true,
                ..Default::default()
            });
        }
        self.consumed_keys.record_press(evdev_key, true);
        Some(IbusKeyOutcome {
            consumed: true,
            ..Default::default()
        })
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

        if let Some(outcome) = self.handle_lock_or_password_state(evdev_key, is_press) {
            return outcome;
        }

        if evdev_key == KEY_CAPSLOCK {
            return self.handle_capslock_event(evdev_key, is_press);
        }

        if (state & IBUS_LOCK_MASK) != 0 && !self.is_caps_locked {
            self.update_caps_lock_state(true);
        }

        let toggled_mode = if self.is_caps_locked {
            self.shift_sm.reset();
            None
        } else {
            self.shift_sm.on_key_event(evdev_key, is_press, self.mode)
        };

        if !is_press {
            if let Some(outcome) = self.handle_push_to_talk_release(evdev_key) {
                return outcome;
            }
            return self.handle_key_release(evdev_key, toggled_mode);
        }

        let shift_active = (state & IBUS_SHIFT_MASK) != 0 || (self.mods_depressed & MOD_SHIFT) != 0;
        let ctrl_active = (state & IBUS_CONTROL_MASK) != 0 || (self.mods_depressed & MOD_CTRL) != 0;
        let alt_active = (state & IBUS_MOD1_MASK) != 0 || (self.mods_depressed & MOD_ALT) != 0;
        let super_active = (state & IBUS_SUPER_MASK) != 0 || (self.mods_depressed & MOD_SUPER) != 0;
        let ctrl_or_alt = ibus_state_has_ctrl_or_alt(state)
            || (self.mods_depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0;

        if let Some(shortcut_outcome) = self.handle_global_shortcuts(
            evdev_key,
            shift_active,
            ctrl_active,
            alt_active,
            super_active,
        ) {
            self.consumed_keys.record_press(evdev_key, true);
            return shortcut_outcome;
        }

        if let Some(voice_outcome) = self.handle_voice_recording_key(evdev_key) {
            return voice_outcome;
        }

        let mut outcome =
            self.handle_key_press(evdev_key, shift_active, ctrl_or_alt, self.is_caps_locked);
        if self.is_full_width_char() {
            if let Some(ref text) = outcome.commit_text {
                outcome.commit_text = Some(to_full_width_string(text));
            }
        }
        self.consumed_keys.record_press(evdev_key, outcome.consumed);
        outcome
    }

    /// 处理按键释放阶段（含 Shift/Ctrl 防误触中英文切换判定）
    pub(crate) fn handle_key_release(
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
    pub(crate) fn handle_key_press(
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
                    let full = to_full_width_char(ch);
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
    pub(crate) fn handle_active_composition_key(
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
            KeyProcessAction::Commit(committed) => {
                if !self.engine.raw_input().is_empty() {
                    self.engine.reset();
                }
                Some(IbusKeyOutcome {
                    consumed: true,
                    commit_text: Some(committed),
                    update_ui: true,
                    ..Default::default()
                })
            }
            KeyProcessAction::CommitPartial { committed, .. } => Some(IbusKeyOutcome {
                consumed: true,
                commit_text: Some(committed),
                update_ui: true,
                ..Default::default()
            }),
        }
    }

    /// 处理空闲状态（`raw_input` 为空）下的中文标点转换与数字保护
    pub(crate) fn handle_idle_punctuation_key(
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
