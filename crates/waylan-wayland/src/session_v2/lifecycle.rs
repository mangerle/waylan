use crate::session_common::{WaylandSessionLifecycle, WaylandWatcherContext};
use waylan_engine::ImeRuntime;

use super::WaylanV2Session;

impl WaylanV2Session {
    /// 挂起 Activate 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_activate(&mut self) {
        self.focus.pending_active = true;
        self.focus.pending_focus_reset = true;
        self.focus.pending_surrounding_text = None;
        self.focus.pending_is_password = false;
    }

    /// 挂起 Deactivate 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_deactivate(&mut self) {
        self.focus.pending_active = false;
        self.focus.pending_focus_reset = true;
        self.focus.pending_surrounding_text = None;
        self.focus.pending_is_password = false;
    }

    /// 挂起 SurroundingText 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        self.focus.pending_surrounding_text = Some((text, cursor, anchor));
    }

    /// 在收到 Done 事件时原子应用挂起的双缓冲状态并递增 done_serial，返回生效后的激活状态
    pub(crate) fn apply_done_state(&mut self) -> bool {
        self.protocol.done_serial = self.protocol.done_serial.wrapping_add(1);
        let was_active = self.focus.is_active;
        let now_active = self.focus.pending_active;
        let focus_reset = self.focus.pending_focus_reset;
        self.focus.is_active = now_active;
        self.focus.pending_focus_reset = false;

        if now_active {
            self.focus.surrounding_text = self.focus.pending_surrounding_text.clone();
            self.focus.is_password_context = self.focus.pending_is_password;
            self.runtime
                .set_protected_context(self.focus.is_password_context);
            if !was_active || focus_reset {
                self.sync_external_state();
                self.reset_focus_state();
            } else {
                self.sync_atomic_state();
                if self.focus.is_password_context
                    && !self.runtime.core.engine.raw_input().is_empty()
                {
                    self.cancel_input();
                }
            }
        } else {
            self.focus.pending_surrounding_text = None;
            self.focus.surrounding_text = None;
            self.focus.pending_is_password = false;
            self.focus.is_password_context = false;
            self.runtime.set_protected_context(false);
            if was_active || focus_reset {
                if self.runtime.core.mods_depressed() != 0 && self.protocol.has_virtual_keymap {
                    if let Some(ref vk) = self.protocol.virtual_keyboard {
                        vk.modifiers(
                            0,
                            0,
                            self.runtime.core.mods_locked(),
                            self.keyboard.mods_group,
                        );
                    }
                }
                self.reset_focus_state();
                let _ = self.runtime.core.engine.maybe_compact_user_dict();
            }
        }
        now_active
    }

    /// 重置焦点关联的键盘修饰键、防误触状态机与输入引擎状态
    pub(crate) fn reset_focus_state(&mut self) {
        self.runtime.core.reset_input_state();
        self.keyboard.repeat_state = None;
        self.keyboard.mods_latched = 0;
        self.keyboard.mods_group = 0;
        self.hide_candidate_surface();
    }
}

impl WaylandSessionLifecycle for WaylanV2Session {
    #[inline]
    fn runtime_ref(&self) -> &ImeRuntime {
        &self.runtime
    }

    #[inline]
    fn runtime(&mut self) -> &mut ImeRuntime {
        &mut self.runtime
    }

    #[inline]
    fn watcher_ref(&self) -> &WaylandWatcherContext<Self> {
        &self.watcher
    }

    #[inline]
    fn watcher(&mut self) -> &mut WaylandWatcherContext<Self> {
        &mut self.watcher
    }

    #[inline]
    fn session_label(&self) -> &'static str {
        "V2"
    }

    fn on_english_mode_switch(&mut self) {
        self.cancel_input();
    }
}
