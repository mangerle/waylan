use std::sync::atomic::Ordering;

use log::{info, warn};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_core::InputMode;

use crate::fs_watcher::InotifyWatcher;
use crate::input_core::check_watched_paths_changed;

use super::WaylanV2Session;

impl WaylanV2Session {
    /// 挂起 Activate 事件（等待 Done 事件原子提交）
    pub fn stage_activate(&mut self) {
        self.pending_active = true;
        self.pending_focus_reset = true;
        self.pending_surrounding_text = None;
        self.pending_is_password = false;
    }

    /// 挂起 Deactivate 事件（等待 Done 事件原子提交）
    pub fn stage_deactivate(&mut self) {
        self.pending_active = false;
        self.pending_focus_reset = true;
        self.pending_surrounding_text = None;
        self.pending_is_password = false;
    }

    /// 挂起 SurroundingText 事件（等待 Done 事件原子提交）
    pub fn stage_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        self.pending_surrounding_text = Some((text, cursor, anchor));
    }

    /// 在收到 Done 事件时原子应用挂起的双缓冲状态并递增 done_serial，返回生效后的激活状态
    pub fn apply_done_state(&mut self) -> bool {
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
                if self.is_password_context && !self.engine.raw_input().is_empty() {
                    self.cancel_input();
                }
            }
        } else {
            self.pending_surrounding_text = None;
            self.surrounding_text = None;
            self.pending_is_password = false;
            self.is_password_context = false;
            if was_active || focus_reset {
                if self.mods_depressed != 0 && self.has_virtual_keymap {
                    if let Some(ref vk) = self.virtual_keyboard {
                        vk.modifiers(0, 0, self.mods_locked, self.mods_group);
                    }
                }
                self.reset_focus_state();
                let _ = self.engine.maybe_compact_user_dict();
            }
        }
        now_active
    }

    /// 重置焦点关联的键盘修饰键、防误触状态机与输入引擎状态
    pub(crate) fn reset_focus_state(&mut self) {
        self.mods_depressed = 0;
        self.phys_mods = 0;
        self.consumed_keys.clear();
        self.repeat_state = None;
        self.mods_latched = 0;
        self.mods_locked = 0;
        self.mods_group = 0;
        self.shift_state.reset();
        self.punctuation_processor.reset();
        self.engine.reset();
        self.hide_candidate_surface();
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（优先基于 inotify 事件感知，回退时使用时间戳轮询）
    pub fn sync_external_state(&mut self) {
        let mut should_reload = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::Relaxed))
            .unwrap_or(false);

        let changed = if let Some(ref mut watcher) = self.inotify_watcher {
            watcher.drain_events()
        } else {
            check_watched_paths_changed(&self.watched_paths, &mut self.last_watched_mtime)
        };
        if changed {
            should_reload = true;
        }

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                let phrase_path = CustomPhraseManager::default_path();
                self.engine.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
                let wal_opt = self.engine.user_dict_path.clone();
                if let Some(ref wal_path) = wal_opt {
                    self.engine.set_user_dict_path(wal_path);
                }
            }
            info!("[Wayland v2] 已完成用户词库与运行配置热重载。");
        }

        self.sync_mode_from_shared_flag();
    }

    /// 尝试根据当前被监控路径列表初始化 inotify 事件监控器
    pub fn init_inotify_watcher(&mut self) {
        if !self.watched_paths.is_empty() && self.inotify_watcher.is_none() {
            match InotifyWatcher::try_new(&self.watched_paths) {
                Ok(watcher) => {
                    info!(
                        "[微澜 Wayland V2] 成功初始化 inotify 文件监控，关闭高频后台 stat 轮询。"
                    );
                    self.inotify_watcher = Some(watcher);
                }
                Err(e) => {
                    warn!(
                        "[微澜 Wayland V2] 初始化 inotify 失败 ({})，回退至定时轮询模式。",
                        e
                    );
                }
            }
        }
    }

    /// 按键热路径轻量同步：仅检查原子标志位，避免每次敲击按键触发磁盘文件 metadata 系统调用
    pub fn sync_atomic_state(&mut self) {
        let requested = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.load(Ordering::Acquire))
            .unwrap_or(false);
        if requested {
            self.sync_external_state();
            return;
        }

        self.sync_mode_from_shared_flag();
    }

    /// 从共享原子标志同步输入模式，若在拼音组合期间切至英文则立即取消输入并隐藏候选框
    pub(crate) fn sync_mode_from_shared_flag(&mut self) {
        if let Some(ref zh_flag) = self.shared_is_chinese {
            let external_zh = zh_flag.load(Ordering::Acquire);
            let current_zh = matches!(self.mode, InputMode::Chinese | InputMode::VMode);
            if external_zh != current_zh {
                self.mode = if external_zh {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
                if self.mode == InputMode::English && !self.engine.raw_input().is_empty() {
                    self.cancel_input();
                }
            }
        }
    }
}
