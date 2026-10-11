mod keys;

use crate::surface::{CandidateSurfaceManager, CursorRect};
use log::info;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::input_core::*;
use waylan_core::{CandidateBoxState, InputMode};

/// 配置与词库热重载回调函数类型
pub type ReloadCallback = Box<dyn FnMut(&mut WaylanV2Session) + Send>;

mod surrounding;

pub use surrounding::{calculate_delete_backward_range, calculate_delete_forward_range};

/// 微澜面向现代 Wayland 合成器 (KDE Plasma 6 / GNOME 46+ / Hyprland) 的 V2 会话状态
pub struct WaylanV2Session {
    /// 核心拼音算法引擎
    pub engine: AlgoEngine,
    /// 中英文状态机
    pub shift_state: ShiftToggleStateMachine,
    /// 智能标点符号处理器
    pub punctuation_processor: PunctuationProcessor,
    /// 候选词翻页快捷键模式
    pub page_keys: PageKeyConfig,
    /// 候选框透明悬浮 Surface 管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 当前是否处于激活会话
    pub is_active: bool,
    /// 关联的 zwp_input_method_v2 协议句柄
    pub input_method: Option<ZwpInputMethodV2>,
    /// 硬件键盘捕获句柄
    pub keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    /// 悬浮候选弹出层 Surface
    pub popup_surface: Option<ZwpInputPopupSurfaceV2>,
    /// 虚拟键盘输入协议句柄（用于模拟光标位移与未拦截按键透传）
    pub virtual_keyboard: Option<ZwpVirtualKeyboardV1>,
    /// 关联的共享内存管理器句柄
    pub shm: Option<wayland_client::protocol::wl_shm::WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<wayland_client::protocol::wl_surface::WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前跟踪的光标坐标
    pub current_cursor: CursorRect,
    /// 当前输入语言模式
    pub mode: InputMode,
    /// 与外部系统托盘共享的中英文状态标志
    pub shared_is_chinese: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// 键盘切换中英文时唤醒系统托盘刷新图标的通知发送端
    pub mode_notify_tx: Option<std::sync::mpsc::SyncSender<()>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    pub reload_requested: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<ReloadCallback>,
    /// 监控外部进程修改的配置文件与词库路径列表（用于在焦点或按键事件时自动感知变更）
    pub watched_paths: Vec<std::path::PathBuf>,
    /// 上次记录的被监控文件最新修改时间戳
    pub last_watched_mtime: Option<std::time::SystemTime>,
    /// 客户端同步的上下文环境文本及光标位置 (text, cursor_byte_idx, anchor_byte_idx)
    pub surrounding_text: Option<(String, u32, u32)>,
    /// Done 事件累计序列号（用于 commit 请求原子同步）
    pub done_serial: u32,
    /// 当前按下的修饰键位掩码（XKB mods_depressed）
    pub mods_depressed: u32,
    /// 标记 V2 会话是否已被合成器宣告不可用（触发自动降级）
    pub is_unavailable: bool,
    /// 与外部系统托盘共享的优雅退出请求标志
    pub exit_requested: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl WaylanV2Session {
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_state: ShiftToggleStateMachine::new(),
            punctuation_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            surface_manager: CandidateSurfaceManager::new(),
            is_active: false,
            input_method: None,
            keyboard_grab: None,
            popup_surface: None,
            virtual_keyboard: None,
            shm: None,
            popup_wl_surface: None,
            active_buffer: None,
            qhandle: None,
            current_cursor: CursorRect::default(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            mode_notify_tx: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            surrounding_text: None,
            done_serial: 0,
            mods_depressed: 0,
            is_unavailable: false,
            exit_requested: None,
        }
    }

    /// 根据按键按下/释放事件更新修饰键位掩码
    pub fn update_modifier_by_key(&mut self, keycode: u32, is_press: bool) {
        let mask = match keycode {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => MOD_SHIFT,
            KEY_LEFTCTRL | KEY_RIGHTCTRL => MOD_CTRL,
            KEY_LEFTALT | KEY_RIGHTALT => MOD_ALT,
            _ => 0,
        };
        if mask != 0 {
            if is_press {
                self.mods_depressed |= mask;
            } else {
                self.mods_depressed &= !mask;
            }
        }
    }

    /// 判断当前是否处于 Shift 按下状态
    pub fn is_shift_active(&self) -> bool {
        (self.mods_depressed & MOD_SHIFT) != 0
    }

    /// 判断当前是否按下了 Ctrl / Alt / Super 组合修饰键
    pub fn has_ctrl_or_alt(&self) -> bool {
        (self.mods_depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（同时自动感知配置文件修改时间戳变更）
    pub fn sync_external_state(&mut self) {
        use std::sync::atomic::Ordering;

        let mut should_reload = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::Relaxed))
            .unwrap_or(false);

        if !self.watched_paths.is_empty() {
            let mut latest_mtime: Option<std::time::SystemTime> = None;
            for p in &self.watched_paths {
                if let Ok(meta) = std::fs::metadata(p) {
                    if let Ok(mtime) = meta.modified() {
                        latest_mtime = Some(match latest_mtime {
                            Some(prev) => prev.max(mtime),
                            None => mtime,
                        });
                    }
                }
            }
            match (self.last_watched_mtime, latest_mtime) {
                (Some(old), Some(new)) if new > old => {
                    self.last_watched_mtime = Some(new);
                    should_reload = true;
                }
                (None, Some(new)) => {
                    self.last_watched_mtime = Some(new);
                }
                _ => {}
            }
        }

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
                self.engine.custom_phrases =
                    waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
                let wal_opt = self.engine.user_dict_path.clone();
                if let Some(ref wal_path) = wal_opt {
                    self.engine.set_user_dict_path(wal_path);
                }
            }
            info!("[Wayland v2] 已完成用户词库与运行配置热重载。");
        }

        if let Some(ref zh_flag) = self.shared_is_chinese {
            let external_zh = zh_flag.load(Ordering::Relaxed);
            let current_zh = matches!(self.mode, InputMode::Chinese | InputMode::VMode);
            if external_zh != current_zh {
                self.mode = if external_zh {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
            }
        }
    }

    /// 设置关联的 V2 输入法实例
    pub fn set_input_method(&mut self, im: ZwpInputMethodV2) {
        self.input_method = Some(im);
    }

    /// 根据当前算法引擎与输入模式构建候选框渲染状态
    pub fn build_candidate_box_state(&self) -> CandidateBoxState {
        let candidates = self.engine.candidates();
        let core_candidates: Vec<waylan_core::Candidate> = candidates
            .iter()
            .map(|c| waylan_core::Candidate {
                text: c.text.clone(),
                pinyin: c.pinyin.clone(),
                score: c.score,
                is_expandable: c.is_expandable,
                sub_candidates: c.sub_candidates.clone(),
            })
            .collect();

        let expanded_card = if self.engine.is_expanded {
            waylan_core::ExpandedCardState::Expanded {
                selected_index: self.engine.expanded_selected_index,
            }
        } else {
            waylan_core::ExpandedCardState::Collapsed
        };

        CandidateBoxState {
            visible: self.is_active && !self.engine.raw_input().is_empty(),
            preedit: self.engine.formatted_preedit(),
            candidates: core_candidates,
            selected_index: self.engine.selected_index,
            expanded_card,
            mode_badge: match self.mode {
                InputMode::Chinese => "中".to_string(),
                InputMode::English => "英".to_string(),
                InputMode::VMode => "V".to_string(),
            },
        }
    }

    /// 触发候选框悬浮表面重绘并提交至 Wayland 表面
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        self.surface_manager.commit_to_wayland(
            &box_state,
            self.shm.as_ref(),
            self.popup_wl_surface.as_ref(),
            self.qhandle.as_ref(),
            &mut self.active_buffer,
        );
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    pub fn hide_candidate_surface(&mut self) {
        self.surface_manager
            .hide_wayland_surface(self.popup_wl_surface.as_ref(), &mut self.active_buffer);
    }

    /// 提交指定文本上屏，并重置预编辑状态
    pub fn commit_text(&mut self, text: &str) {
        if let Some(im) = &self.input_method {
            im.commit_string(text.to_string());
            im.set_preedit_string(String::new(), 0, 0);
            im.commit(self.done_serial);
        }
        self.hide_candidate_surface();
    }

    /// 更新当前 preedit 预编辑字符串到目标客户端
    pub fn update_preedit(&mut self, preedit: &str) {
        if let Some(im) = &self.input_method {
            let cursor = if preedit.is_empty() {
                0
            } else {
                self.engine.formatted_cursor_byte_offset() as i32
            };
            im.set_preedit_string(preedit.to_string(), cursor, cursor);
            im.commit(self.done_serial);
        }
        if preedit.is_empty() {
            self.hide_candidate_surface();
        } else {
            self.refresh_candidate_surface();
        }
    }

    /// 分段确认部分文本上屏，并以剩余拼音继续推进预编辑与候选渲染
    pub fn commit_partial(&mut self, text: &str, remaining_preedit: &str) {
        if let Some(im) = &self.input_method {
            im.commit_string(text.to_string());
            let cursor = self.engine.formatted_cursor_byte_offset() as i32;
            im.set_preedit_string(remaining_preedit.to_string(), cursor, cursor);
            im.commit(self.done_serial);
        }
        self.refresh_candidate_surface();
    }

    /// 取消当前输入，重置引擎与预编辑区
    pub fn cancel_input(&mut self) {
        self.engine.reset();
        self.update_preedit("");
    }

    /// 触发退格向后删除（空缓冲区或英文模式下删除光标前 1 个字符或删除当前选区）
    pub fn delete_backward(&mut self) {
        if let Some(im) = &self.input_method {
            let (before, after) = calculate_delete_backward_range(self.surrounding_text.as_ref());
            if before > 0 || after > 0 {
                im.delete_surrounding_text(before, after);
                im.commit(self.done_serial);
            }
        }
    }

    /// 触发向前删除（Delete 键删除光标后 1 个字符或删除当前选区）
    pub fn delete_forward(&mut self) {
        if let Some(im) = &self.input_method {
            let (before, after) = calculate_delete_forward_range(self.surrounding_text.as_ref());
            if before > 0 || after > 0 {
                im.delete_surrounding_text(before, after);
                im.commit(self.done_serial);
            }
        }
    }

    /// 将未被输入法消费的原始物理按键通过虚拟键盘协议回投到席位，确保快捷键与控制键正常透传
    pub fn forward_key_to_seat(&mut self, time: u32, key: u32, is_press: bool) {
        if let Some(ref vk) = self.virtual_keyboard {
            vk.modifiers(self.mods_depressed, 0, 0, 0);
            let state_val = if is_press { 1 } else { 0 };
            vk.key(time, key, state_val);
        }
    }

    /// 处理英文模式下的按键输入
    fn process_english_mode_key(&mut self, key: u32) -> (bool, Option<String>) {
        keys::process_english_mode_key(self, key)
    }

    /// 处理处于拼音组词状态下的键盘拦截
    fn process_composition_key(&mut self, key: u32, now: u64) -> Option<(bool, Option<String>)> {
        keys::process_composition_key(self, key, now)
    }

    /// 处理输入缓冲区为空时的空闲按键输入
    fn process_idle_key(&mut self, key: u32) -> Option<(bool, Option<String>)> {
        keys::process_idle_key(self, key)
    }

    /// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
    pub fn process_key_event(&mut self, key: u32, is_press: bool) -> (bool, Option<String>) {
        self.update_modifier_by_key(key, is_press);

        // 1. 优先经由 Shift/Ctrl 防误触状态机判定
        if let Some(new_mode) = self.shift_state.on_key_event(key, is_press, self.mode) {
            self.mode = new_mode;
            if let Some(ref zh_flag) = self.shared_is_chinese {
                zh_flag.store(
                    matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                    std::sync::atomic::Ordering::Release,
                );
            }
            if let Some(ref notify_tx) = self.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            let mode_str = match new_mode {
                InputMode::Chinese => "中文",
                InputMode::English => "英文",
                InputMode::VMode => "V模式",
            };
            info!("[Wayland v2] 切换输入模式为：{}", mode_str);

            if new_mode == InputMode::English && !self.engine.raw_input().is_empty() {
                let raw = self.engine.raw_input().to_string();
                self.commit_text(&raw);
                self.engine.reset();
                return (true, Some(raw));
            }
            return (true, None);
        }

        // 按键释放阶段：若处于拼音组词状态则吞掉释放事件
        if !is_press {
            let in_composition = !self.engine.raw_input().is_empty();
            return (in_composition, None);
        }

        // 快捷键穿透：Ctrl / Alt / Super 等修饰组合键透传
        if self.has_ctrl_or_alt() {
            return (false, None);
        }

        // 2. 英文模式
        if self.mode == InputMode::English {
            return self.process_english_mode_key(key);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 3. 正在组合拼音状态下的专门按键拦截
        if !self.engine.raw_input().is_empty() {
            if let Some(result) = self.process_composition_key(key, now) {
                return result;
            }
        }

        // 4. 输入缓冲区为空时的按键处理
        if self.engine.raw_input().is_empty() {
            if let Some(result) = self.process_idle_key(key) {
                return result;
            }
        }

        // 5. 字母键输入（A-Z）：进入拼音算法管线
        if let Some(ch) = keycode_to_char(key) {
            self.engine.push_char(ch);
            let preedit = self.engine.formatted_preedit();
            self.update_preedit(&preedit);
            return (true, None);
        }

        (false, None)
    }

    /// 处理单个硬件按键按下/释放事件（返回本次直接上屏的文本，便于无合成器环境下单元测试验证）
    pub fn handle_key_event(&mut self, key: u32, is_press: bool) -> Option<String> {
        self.process_key_event(key, is_press).1
    }
}

mod dispatch;
mod protocol;

#[cfg(test)]
#[path = "session_v2_tests.rs"]
mod tests;
