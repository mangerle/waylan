//! IBus 按键事件核心分发与协议适配
//!
//! # 设计原理
//! - **实现初衷**：将 IBus 协议层传入的 (keyval, keycode, state) 转换为标准 evdev 扫描码，
//!   全权委托统一的 [`ImeRuntime::process_key_event`] 决策流水线处理，并将统一响应映射为 IbusKeyOutcome。
//! - **核心优势**：彻底消除 IBus 与 Wayland 端按键处理逻辑的不一致，使 IBus 端降级为纯粹的协议转接器；
//!   密码框保护、CapsLock 截断提交、中英模式切换、语音听写与全局快捷键全部由核心运行时单一真实源驱动。
//! - **代价与局限**：需将 X11 修饰键状态位映射至统一修饰键位掩码。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, CandidateIndexStyle, CandidateOrientation, InputMode};
use waylan_engine::InputMethodFrontend;

use super::super::engine::UiSnapshot;
use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::keyval::{
    IBUS_CONTROL_MASK, IBUS_LOCK_MASK, IBUS_MOD1_MASK, IBUS_RELEASE_MASK, IBUS_SHIFT_MASK,
    IBUS_SUPER_MASK, ibus_keyval_to_evdev,
};

/// 针对 IBus 会话结果收集的轻量前端管道适配器
pub struct IbusFrontend<'a> {
    pub outcome: &'a mut IbusKeyOutcome,
    pub page_size: usize,
    pub orientation: CandidateOrientation,
    pub index_style: CandidateIndexStyle,
    /// 全局标准拼音注音展示开关
    pub show_pinyin_comment: bool,
}

impl<'a> InputMethodFrontend for IbusFrontend<'a> {
    fn commit_text(&mut self, text: &str) {
        self.outcome.commit_text = Some(text.to_string());
    }

    fn update_preedit(&mut self, text: &str, _cursor_byte_idx: usize) {
        self.outcome.update_ui = true;
        if let Some(ref mut snap) = self.outcome.ui_snapshot {
            snap.preedit = text.to_string();
        } else {
            let mut snap = UiSnapshot::empty();
            snap.preedit = text.to_string();
            self.outcome.ui_snapshot = Some(snap);
        }
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        self.outcome.update_ui = true;
        let snap = UiSnapshot::from_candidate_box_state(
            state,
            self.page_size,
            self.orientation,
            self.index_style,
            self.show_pinyin_comment,
        );
        self.outcome.ui_snapshot = Some(snap);
    }

    fn hide_candidates(&mut self) {
        self.outcome.update_ui = true;
        self.outcome.ui_snapshot = Some(UiSnapshot::empty());
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
    /// 同步外部 IBus 修饰键状态与 CapsLock 锁定状态至核心会话
    fn sync_modifiers_from_ibus(&mut self, evdev_key: u32, is_press: bool, state: u32) {
        let mut external_mods = 0u32;
        if (state & IBUS_SHIFT_MASK) != 0 {
            external_mods |= MOD_SHIFT;
        }
        if (state & IBUS_CONTROL_MASK) != 0 {
            external_mods |= MOD_CTRL;
        }
        if (state & IBUS_MOD1_MASK) != 0 {
            external_mods |= MOD_ALT;
        }
        if (state & IBUS_SUPER_MASK) != 0 {
            external_mods |= MOD_SUPER;
        }
        self.runtime
            .core
            .sync_external_modifiers(evdev_key, is_press, external_mods);

        // 外部状态指示的 CapsLock 锁定标志补齐：
        // 仅当当前按键并非 CapsLock 键自身、且外部明确指示 Lock 掩码、且当前并未主动切回中文时，
        // 才允许补齐对齐（如开机初次带大写或外部窗口切换进来的首键）。
        // 绝不在普通按键未带掩码时误冲刷内部大写状态（因多数客户端普通键击不会附带锁定掩码）。
        if evdev_key != KEY_CAPSLOCK
            && (state & IBUS_LOCK_MASK) != 0
            && !self.is_caps_locked()
            && self.runtime.core.input_mode() != InputMode::Chinese
        {
            self.update_caps_lock_state(true);
        }
    }

    /// 处理 IBus `ProcessKeyEvent(keyval, keycode, state)` 请求
    pub fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> IbusKeyOutcome {
        let _ = self.sync_shared_mode();
        let is_press = (state & IBUS_RELEASE_MASK) == 0;
        let evdev_key = ibus_keyval_to_evdev(keyval, keycode);

        self.sync_modifiers_from_ibus(evdev_key, is_press, state);

        let mut outcome = IbusKeyOutcome::default();
        let is_password_or_locked = self.is_screen_locked || self.is_password_context;
        let surrounding = self.surrounding_text.clone();
        let page_size = self.runtime.core.engine.config.page_size;
        let orientation = self.runtime.candidate_orientation();
        let index_style = self.runtime.candidate_index_style();

        let (consumed, commit_text) = {
            let mut frontend = IbusFrontend {
                outcome: &mut outcome,
                page_size,
                orientation,
                index_style,
                show_pinyin_comment: self.runtime.core.engine.config.show_pinyin_comment,
            };
            let res = self.runtime.process_key_event(
                &mut frontend,
                waylan_engine::ProcessKeyEventParams {
                    key: evdev_key,
                    is_press,
                    is_password_or_locked,
                    surrounding_text: surrounding.as_ref(),
                    inline_preedit: false,
                },
            );
            // 若当前处于 V 模式动态输入，预编辑区保留原始输入串以与历史行为对齐
            let raw = self.runtime.core.engine.raw_input();
            if raw.starts_with('v')
                && !self.runtime.is_voice_active()
                && let Some(ref mut snap) = frontend.outcome.ui_snapshot
            {
                snap.preedit = raw.to_string();
            }
            res
        };

        outcome.consumed = consumed;
        if outcome.commit_text.is_none() {
            outcome.commit_text = commit_text;
        }

        outcome
    }
}
