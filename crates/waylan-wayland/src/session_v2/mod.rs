mod keys;
mod lifecycle;
mod surrounding;

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, mpsc::SyncSender, Arc};
use std::time::{Instant, SystemTime};

use log::{debug, warn};
use waylan_algo::{AlgoEngine, ImeCoreSession};
use waylan_core::CandidateBoxState;
use waylan_voice::VoiceInteractionManager;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_pointer::WlPointer, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::fs_watcher::InotifyWatcher;
use crate::loop_wakeup::LoopWakeup;
use crate::session_common::*;
use crate::surface::{CandidateSurfaceManager, CursorRect};
pub use waylan_core::input::*;
pub use waylan_core::InputMode;

pub use surrounding::{calculate_delete_backward_range, calculate_delete_forward_range};

/// 配置与词库热重载回调函数类型
pub type ReloadCallback = Box<dyn FnMut(&mut WaylanV2Session) + Send>;

/// 微澜面向现代 Wayland 合成器 (KDE Plasma 6 / GNOME 46+ / Hyprland) 的 V2 会话状态
pub struct WaylanV2Session {
    /// 核心输入法决策会话与算法状态机
    pub core: ImeCoreSession,
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
    /// 当前锁存的修饰键位掩码（XKB mods_latched）
    pub mods_latched: u32,
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
    /// 外部按键事件拦截回调（用于语音听写快捷键等扩展）
    pub key_filter: Option<KeyFilterCallback>,
    /// 统一语音按键交互与录音流式管理器
    pub voice_manager: Option<VoiceInteractionManager>,
}

impl Deref for WaylanV2Session {
    type Target = ImeCoreSession;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl DerefMut for WaylanV2Session {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

impl WaylanV2Session {
    /// 基于已加载并配置好的核心算法引擎构造 Wayland V2 输入法会话
    ///
    /// # 设计原理
    /// - **实现初衷**：对接 Wayland `zwp_input_method_v2` 协议标准，通过双缓冲事务机制实现与现代桌面合成器的输入法会话解耦。
    /// - **核心优势**：零拷贝算法引擎集成，支持虚拟键盘直通回退、周围文本感知、光标矩形防溢出对齐与客户端按键连击加速。
    /// - **代价与局限**：强依赖合成器完整实现 V2 输入法扩展协议，不支持无该协议的早期合成器环境。
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            core: ImeCoreSession::new(engine),
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
            mods_latched: 0,
            mods_group: 0,
            has_virtual_keymap: false,
            repeat_info: (25, 400),
            repeat_state: None,
            is_unavailable: false,
            exit_requested: None,
            is_grid_expanded: false,
            grid_selected_index: 0,
            pointer: None,
            pointer_pos: (0.0, 0.0),
            pointer_in_surface: false,
            inotify_watcher: None,
            loop_wakeup: None,
            key_filter: None,
            voice_manager: None,
        }
    }

    /// 查询当前是否处于语音录音或语音浮条激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.voice_manager
            .as_ref()
            .map(|v| v.is_active())
            .unwrap_or(false)
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    pub fn poll_voice_cycle(&mut self) -> Option<String> {
        if let Some(ref mut vm) = self.voice_manager {
            if vm.is_active() {
                let commit_opt = vm.poll_events();
                if let Some(ref text) = commit_opt {
                    self.commit_text(text);
                    self.punctuation_processor.record_commit(text);
                    self.hide_candidate_surface();
                    return commit_opt;
                } else {
                    self.refresh_candidate_surface();
                }
            }
        }
        None
    }

    /// 设置关联的 V2 输入法实例
    pub(crate) fn set_input_method(&mut self, im: ZwpInputMethodV2) {
        self.input_method = Some(im);
    }

    /// 根据当前算法引擎与输入模式构建候选框渲染状态
    pub(crate) fn build_candidate_box_state(&self) -> CandidateBoxState {
        keys::build_candidate_box_state(self)
    }

    /// 触发候选框悬浮表面重绘并提交至 Wayland 表面
    ///
    /// # 设计原理
    /// - **实现初衷**：在候选词状态变迁、用户词库重载或配置变更后主动刷新并提交合成器渲染候选框。
    /// - **核心优势**：自动协调多缓冲槽位轮转与脏区域计算，仅在存在有效候选词时请求合成器展示。
    /// - **代价与局限**：涉及矢量绘制与共享内存提交，频繁调用会产生微秒级渲染开销。
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
    ///
    /// # 设计原理
    /// - **实现初衷**：在输入结束、用户取消或失焦时隐藏悬浮图层，清理屏幕遮挡。
    /// - **核心优势**：主动解绑活跃缓冲并调用 attach(None)，避免无效占用桌面图层资源。
    /// - **代价与局限**：向合成器发送清空表面指令，需等待合成器下发 release 事件回收缓冲槽位。
    pub fn hide_candidate_surface(&mut self) {
        hide_candidate_surface_buffer(
            &mut self.surface_manager,
            self.popup_wl_surface.as_ref(),
            &mut self.active_buffer,
        );
    }

    /// 提交指定文本上屏，并重置预编辑状态
    pub(crate) fn commit_text(&mut self, text: &str) {
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
    pub(crate) fn update_preedit(&mut self, preedit: &str) {
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
    pub(crate) fn commit_partial(&mut self, text: &str, remaining_preedit: &str) {
        if let Some(im) = &self.input_method {
            im.commit_string(text.to_string());
            let cursor = self.engine.formatted_cursor_byte_offset() as i32;
            im.set_preedit_string(remaining_preedit.to_string(), cursor, cursor);
            im.commit(self.done_serial);
        }
        self.refresh_candidate_surface();
    }

    /// 取消当前输入，重置引擎与预编辑区
    pub(crate) fn cancel_input(&mut self) {
        self.is_grid_expanded = false;
        self.grid_selected_index = 0;
        self.engine.reset();
        self.update_preedit("");
    }

    /// 触发退格向后删除（空缓冲区或英文模式下删除光标前 1 个字符或删除当前选区）
    ///
    /// # 设计原理
    /// - **实现初衷**：基于周围文本感知实现精确的退格字符删除或文本选区清理。
    /// - **核心优势**：在协议层直接通过 `delete_surrounding_text` 提交删除事务，无需通过虚拟键盘注入按键。
    /// - **代价与局限**：依赖目标应用程序正确上报周围文本（surrounding_text），若应用不支持则无法执行。
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
    ///
    /// # 设计原理
    /// - **实现初衷**：在 Delete 键触发或向前编辑时按字符或选区范围精确执行删除。
    /// - **核心优势**：单次事务原子执行，避免光标漂移与删除滞后。
    /// - **代价与局限**：同样依赖宿主文本控件上报合法且准确的周围文本上下文。
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
    pub(crate) fn forward_key_to_seat(&mut self, time: u32, key: u32, is_press: bool) {
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
    pub(crate) fn process_key_event(&mut self, key: u32, is_press: bool) -> (bool, Option<String>) {
        keys::process_key_event(self, key, is_press)
    }

    /// 计算下一次 poll 等待超时时间（毫秒），兼顾按键长按连击定时器
    pub(crate) fn next_poll_timeout_ms(&self, default_ms: i32) -> i32 {
        keys::next_poll_timeout_ms(self, default_ms)
    }

    /// 检查并触发到期的长按按键连击事件
    pub(crate) fn drive_key_repeat(&mut self) {
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
