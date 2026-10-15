mod keys;
mod lifecycle;
mod surrounding;

use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, mpsc::SyncSender, Arc};
use std::time::{Instant, SystemTime};

use log::{debug, warn};
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use waylan_core::{CandidateBoxState, InputMode};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_pointer::WlPointer, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::fs_watcher::InotifyWatcher;
use crate::input_core::*;
use crate::loop_wakeup::LoopWakeup;
use crate::session_common::*;
use crate::surface::{CandidateSurfaceManager, CursorRect};

pub use surrounding::{calculate_delete_backward_range, calculate_delete_forward_range};

/// 配置与词库热重载回调函数类型
pub type ReloadCallback = Box<dyn FnMut(&mut WaylanV2Session) + Send>;

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
    pub shm: Option<WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前跟踪的光标坐标
    pub current_cursor: CursorRect,
    /// 当前输入语言模式
    pub mode: InputMode,
    /// 与外部系统托盘共享的中英文状态标志
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    /// 与外部系统托盘共享的 CapsLock 大写锁定状态标志
    pub shared_is_caps_locked: Option<Arc<AtomicBool>>,
    /// 键盘切换中英文时唤醒系统托盘刷新图标的通知发送端
    pub mode_notify_tx: Option<SyncSender<()>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<ReloadCallback>,
    /// 监控外部进程修改的配置文件与词库路径列表（用于在焦点或按键事件时自动感知变更）
    pub watched_paths: Vec<PathBuf>,
    /// 上次记录的被监控文件最新修改时间戳
    pub last_watched_mtime: Option<SystemTime>,
    /// 客户端同步的上下文环境文本及光标位置 (text, cursor_byte_idx, anchor_byte_idx)
    pub surrounding_text: Option<(String, u32, u32)>,
    /// 双缓冲挂起的会话激活状态（在收到 Done 事件时原子生效）
    pub pending_active: bool,
    /// 双缓冲挂起的上下文环境文本（在收到 Done 事件时原子生效）
    pub pending_surrounding_text: Option<(String, u32, u32)>,
    /// 双缓冲挂起的密码/PIN码输入框标识（在收到 Done 事件时原子生效）
    pub pending_is_password: bool,
    /// 当前激活上下文是否为密码或 PIN 输入框（若为 true 则自动直通按键规避拼音拦截）
    pub is_password_context: bool,
    /// 标记当前批次是否发生了焦点激活/失焦切换（用于在 Done 事件中重置状态机）
    pub pending_focus_reset: bool,
    /// Done 事件累计序列号（用于 commit 请求原子同步）
    pub done_serial: u32,
    /// 当前按下的修饰键位掩码（XKB mods_depressed）
    pub mods_depressed: u32,
    /// 左右物理修饰键独立按下位掩码
    pub phys_mods: u8,
    /// 按键按下与释放对称消费跟踪器
    pub consumed_keys: ConsumedKeyTracker,
    /// 当前锁存的修饰键位掩码（XKB mods_latched）
    pub mods_latched: u32,
    /// 当前锁定的修饰键位掩码（XKB mods_locked，如 CapsLock / NumLock）
    pub mods_locked: u32,
    /// 当前激活的键盘布局组索引（XKB group）
    pub mods_group: u32,
    /// 标记虚拟键盘是否已完成 keymap 映射表初始化（防止未发送 keymap 即调用 key 引发协议错误）
    pub has_virtual_keymap: bool,
    /// 合成器下发的键盘长按连击参数 (rate_keys_per_sec, delay_ms)
    pub repeat_info: (i32, i32),
    /// 当前正处于长按连击计时中的消费按键状态 (keycode, next_fire_instant)
    pub repeat_state: Option<(u32, Instant)>,
    /// 标记 V2 会话是否已被合成器宣告不可用（触发自动降级）
    pub is_unavailable: bool,
    /// 与外部系统托盘共享的优雅退出请求标志
    pub exit_requested: Option<Arc<AtomicBool>>,
    /// 动态键盘布局映射表（支持根据 XKB Keymap 动态更新）
    pub keymap_layout: KeymapLayout,
    /// 5 行网格大候选词面板是否展开
    pub is_grid_expanded: bool,
    /// 5 行网格大候选词面板当前选中的全局候选词索引
    pub grid_selected_index: usize,
    /// 鼠标指针对象（用于接收候选框上的鼠标点击与移动）
    pub pointer: Option<WlPointer>,
    /// 鼠标指针在候选表面内的当前局部物理坐标 (x, y)
    pub pointer_pos: (f64, f64),
    /// 鼠标指针当前是否位于候选表面内部
    pub pointer_in_surface: bool,
    /// 基于 Linux inotify 的配置文件与词库变更监控器
    pub inotify_watcher: Option<InotifyWatcher>,
    /// 事件循环跨线程唤醒器
    pub loop_wakeup: Option<LoopWakeup>,
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
            shared_is_caps_locked: None,
            mode_notify_tx: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            surrounding_text: None,
            pending_active: false,
            pending_surrounding_text: None,
            pending_is_password: false,
            is_password_context: false,
            pending_focus_reset: false,
            done_serial: 0,
            mods_depressed: 0,
            phys_mods: 0,
            consumed_keys: ConsumedKeyTracker::new(),
            mods_latched: 0,
            mods_locked: 0,
            mods_group: 0,
            has_virtual_keymap: false,
            repeat_info: (25, 400),
            repeat_state: None,
            is_unavailable: false,
            exit_requested: None,
            keymap_layout: KeymapLayout::default(),
            is_grid_expanded: false,
            grid_selected_index: 0,
            pointer: None,
            pointer_pos: (0.0, 0.0),
            pointer_in_surface: false,
            inotify_watcher: None,
            loop_wakeup: None,
        }
    }

    /// 根据按键按下/释放事件更新修饰键位掩码
    pub fn update_modifier_by_key(&mut self, keycode: u32, is_press: bool) {
        update_session_modifiers(
            &mut self.mods_depressed,
            &mut self.phys_mods,
            keycode,
            is_press,
        );
    }

    /// 判断当前是否处于 Shift 按下状态
    #[inline]
    pub fn is_shift_active(&self) -> bool {
        is_shift_active_state(self.mods_depressed, self.phys_mods)
    }

    /// 判断当前是否按下了 Ctrl / Alt / Super 组合修饰键
    #[inline]
    pub fn has_ctrl_or_alt(&self) -> bool {
        has_ctrl_or_alt_state(self.mods_depressed, self.phys_mods)
    }

    /// 判断当前是否处于 CapsLock 大写锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        is_caps_locked_state(self.mods_locked)
    }

    /// 设置关联的 V2 输入法实例
    pub fn set_input_method(&mut self, im: ZwpInputMethodV2) {
        self.input_method = Some(im);
    }

    /// 根据当前算法引擎与输入模式构建候选框渲染状态
    pub fn build_candidate_box_state(&self) -> CandidateBoxState {
        keys::build_candidate_box_state(self)
    }

    /// 触发候选框悬浮表面重绘并提交至 Wayland 表面
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        let params = CandidateSurfaceCommitParams {
            shm: self.shm.as_ref(),
            popup_wl_surface: self.popup_wl_surface.as_ref(),
            qhandle: self.qhandle.as_ref(),
            active_buffer: &mut self.active_buffer,
        };
        commit_candidate_surface(&mut self.surface_manager, &box_state, params);
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    pub fn hide_candidate_surface(&mut self) {
        hide_candidate_surface_buffer(
            &mut self.surface_manager,
            self.popup_wl_surface.as_ref(),
            &mut self.active_buffer,
        );
    }

    /// 提交指定文本上屏，并重置预编辑状态
    pub fn commit_text(&mut self, text: &str) {
        self.is_grid_expanded = false;
        self.grid_selected_index = 0;
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
        self.is_grid_expanded = false;
        self.grid_selected_index = 0;
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
        if !self.has_virtual_keymap {
            debug!(
                "[Wayland v2 键盘回投] 虚拟键盘 keymap 尚未就绪，跳过转发键码 {} (press={})",
                key, is_press
            );
            return;
        }
        if let Some(ref vk) = self.virtual_keyboard {
            let state_val = if is_press { 1 } else { 0 };
            vk.key(time, key, state_val);
        } else {
            warn!(
                "[Wayland v2 键盘回投 警告] 当前会话未绑定 zwp_virtual_keyboard_v1 实例，键码 {} 被丢弃",
                key
            );
        }
    }

    /// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
    pub fn process_key_event(&mut self, key: u32, is_press: bool) -> (bool, Option<String>) {
        keys::process_key_event(self, key, is_press)
    }

    /// 计算下一次 poll 等待超时时间（毫秒），兼顾按键长按连击定时器
    pub fn next_poll_timeout_ms(&self, default_ms: i32) -> i32 {
        keys::next_poll_timeout_ms(self, default_ms)
    }

    /// 检查并触发到期的长按按键连击事件
    pub fn drive_key_repeat(&mut self) {
        keys::drive_key_repeat(self);
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
