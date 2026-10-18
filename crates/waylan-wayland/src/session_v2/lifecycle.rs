use log::info;
use waylan_core::InputMode;

use crate::session_common::{
    apply_default_engine_reload, check_reload_requested_or_files_changed,
    init_session_inotify_watcher, is_reload_requested,
};

use super::WaylanV2Session;

impl WaylanV2Session {
    /// 挂起 Activate 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_activate(&mut self) {
        self.pending_active = true;
        self.pending_focus_reset = true;
        self.pending_surrounding_text = None;
        self.pending_is_password = false;
    }

    /// 挂起 Deactivate 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_deactivate(&mut self) {
        self.pending_active = false;
        self.pending_focus_reset = true;
        self.pending_surrounding_text = None;
        self.pending_is_password = false;
    }

    /// 挂起 SurroundingText 事件（等待 Done 事件原子提交）
    pub(crate) fn stage_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        self.pending_surrounding_text = Some((text, cursor, anchor));
    }

    /// 在收到 Done 事件时原子应用挂起的双缓冲状态并递增 done_serial，返回生效后的激活状态
    pub(crate) fn apply_done_state(&mut self) -> bool {
        self.done_serial = self.done_serial.wrapping_add(1);
        let was_active = self.is_active;
        let now_active = self.pending_active;
        let focus_reset = self.pending_focus_reset;
        self.is_active = now_active;
        self.pending_focus_reset = false;

        if now_active {
            self.surrounding_text = self.pending_surrounding_text.clone();
            self.is_password_context = self.pending_is_password;
            if !was_active || focus_reset {
                self.sync_external_state();
                self.reset_focus_state();
            } else {
                self.sync_atomic_state();
                if self.is_password_context && !self.runtime.core.engine.raw_input().is_empty() {
                    self.cancel_input();
                }
            }
        } else {
            self.pending_surrounding_text = None;
            self.surrounding_text = None;
            self.pending_is_password = false;
            self.is_password_context = false;
            if was_active || focus_reset {
                if self.runtime.core.mods_depressed() != 0 && self.has_virtual_keymap {
                    if let Some(ref vk) = self.virtual_keyboard {
                        vk.modifiers(0, 0, self.runtime.core.mods_locked(), self.mods_group);
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
        self.repeat_state = None;
        self.mods_latched = 0;
        self.mods_group = 0;
        self.hide_candidate_surface();
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（优先基于 inotify 事件感知，回退时使用时间戳轮询）
    pub(crate) fn sync_external_state(&mut self) {
        let reload_flag = self.runtime.reload_requested.clone();
        let should_reload = check_reload_requested_or_files_changed(
            reload_flag.as_deref(),
            &mut self.inotify_watcher,
            &self.watched_paths,
            &mut self.last_watched_mtime,
        );

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                apply_default_engine_reload(&mut self.runtime.core.engine);
            }
            info!("[微澜 Wayland V2] 已完成用户词库与运行配置热重载。");
        }

        self.sync_mode_from_shared_flag();
    }

    /// 尝试根据当前被监控路径列表初始化 inotify 事件监控器
    pub(crate) fn init_inotify_watcher(&mut self) {
        init_session_inotify_watcher(&self.watched_paths, &mut self.inotify_watcher, "Wayland V2");
    }

    /// 按键热路径轻量同步：仅检查原子标志位，避免每次敲击按键触发磁盘文件 metadata 系统调用
    pub(crate) fn sync_atomic_state(&mut self) {
        let reload_flag = self.runtime.reload_requested.clone();
        if is_reload_requested(reload_flag.as_deref()) {
            self.sync_external_state();
            return;
        }

        self.sync_mode_from_shared_flag();
    }

    /// 从共享原子标志同步输入模式，若在拼音组合期间切至英文则立即取消输入并隐藏候选框
    pub(crate) fn sync_mode_from_shared_flag(&mut self) {
        let changed = self.runtime.sync_shared_mode();
        if let Some(InputMode::English) = changed {
            if !self.runtime.core.engine.raw_input().is_empty() {
                self.cancel_input();
            }
        }
    }
}
