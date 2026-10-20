//! IBus 按键事件核心分发与协议适配
//!
//! # 设计原理
//! - **实现初衷**：将 IBus 协议层传入的 (keyval, keycode, state) 转换为标准 evdev 扫描码，
//!   全权委托统一的 [`ImeRuntime::process_key_event`] 决策流水线处理，并将统一响应映射为 IbusKeyOutcome。
//! - **核心优势**：彻底消除 IBus 与 Wayland 端按键处理逻辑的不一致，使 IBus 端降级为纯粹的协议转接器；
//!   密码框保护、CapsLock 截断提交、中英模式切换、语音听写与全局快捷键全部由核心运行时单一真实源驱动。
//! - **代价与局限**：需将 X11 修饰键状态位映射至统一修饰键位掩码。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::InputMethodFrontend;

use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::keyval::{
    ibus_keyval_to_evdev, IBUS_CONTROL_MASK, IBUS_LOCK_MASK, IBUS_MOD1_MASK, IBUS_RELEASE_MASK,
    IBUS_SHIFT_MASK, IBUS_SUPER_MASK,
};

/// 针对 IBus 会话结果收集的轻量前端管道适配器
pub struct IbusFrontend<'a> {
    pub outcome: &'a mut IbusKeyOutcome,
}

impl<'a> InputMethodFrontend for IbusFrontend<'a> {
    fn commit_text(&mut self, text: &str) {
        self.outcome.commit_text = Some(text.to_string());
    }

    fn update_preedit(&mut self, _text: &str, _cursor_byte_idx: usize) {
        self.outcome.update_ui = true;
    }

    fn update_candidates(&mut self, _state: &CandidateBoxState) {
        self.outcome.update_ui = true;
    }

    fn hide_candidates(&mut self) {
        self.outcome.update_ui = true;
    }

    fn move_cursor(&mut self, offset: i32) {
        self.outcome.cursor_offset += offset;
    }

    fn sync_input_mode(&mut self, mode: InputMode) {
        self.outcome.mode_changed = Some(mode);
        self.outcome.update_ui = true;
    }
}

impl IbusSessionCore {
    /// 同步并计算最终的修饰键位掩码与 CapsLock 状态
    fn sync_and_calculate_modifiers(&mut self, evdev_key: u32, is_press: bool, state: u32) -> u32 {
        self.runtime
            .core
            .update_modifier_masks(&mut self.phys_mods, evdev_key, is_press);

        let mut event_mods = self.runtime.core.mods_depressed();
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

        // 外部状态指示的 CapsLock 锁定标志补齐：
        // 仅当当前按键并非 CapsLock 键自身时，才允许由外部 state 掩码补齐激活。
        // 若当前按键是 KEY_CAPSLOCK，按键抬起 (Release) 事件的 state 往往依然残留上一状态的 IBUS_LOCK_MASK，
        // 若不排除 KEY_CAPSLOCK，将导致松开按键时被误判定为重新锁定，造成无法退出大写的死锁。
        if evdev_key != KEY_CAPSLOCK && (state & IBUS_LOCK_MASK) != 0 && !self.is_caps_locked() {
            self.update_caps_lock_state(true);
        }

        event_mods
    }

    /// 处理 CapsLock 键切换时的状态更新并计算生效的模式指示
    fn handle_caps_lock_toggle(&mut self, evdev_key: u32, is_press: bool) -> Option<InputMode> {
        if evdev_key != KEY_CAPSLOCK || !is_press {
            return None;
        }

        let next_locked = !self.is_caps_locked();
        self.update_caps_lock_state(next_locked);
        if self.runtime.core.input_mode() != InputMode::English {
            let effective_mode = if next_locked {
                InputMode::English
            } else {
                self.runtime.core.input_mode()
            };
            Some(effective_mode)
        } else {
            None
        }
    }

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);

        let event_mods = self.sync_and_calculate_modifiers(evdev_key, is_press, state);

        let saved_mods = self.runtime.core.mods_depressed();
        self.runtime.core.set_mods_depressed(event_mods);

        let mut outcome = IbusKeyOutcome::default();
        let is_password_or_locked = self.is_screen_locked || self.is_password_context;
        let surrounding = self.surrounding_text.clone();

        let (consumed, commit_text) = {
            let mut frontend = IbusFrontend {
                outcome: &mut outcome,
            };
            self.runtime.process_key_event(
                &mut frontend,
                waylan_engine::ProcessKeyEventParams {
                    key: evdev_key,
                    is_press,
                    is_password_or_locked,
                    surrounding_text: surrounding.as_ref(),
                    inline_preedit: false,
                },
            )
        };
        self.runtime.core.set_mods_depressed(saved_mods);

        outcome.consumed = consumed;
        if outcome.commit_text.is_none() {
            outcome.commit_text = commit_text;
        }

        let caps_mode_changed = self.handle_caps_lock_toggle(evdev_key, is_press);
        if let Some(caps_mode) = caps_mode_changed {
            outcome.mode_changed = Some(caps_mode);
            outcome.update_ui = true;
        }

        outcome
    }
}
