//! Wayland V2 会话组合子上下文与状态解构
//!
//! # 设计原理
//! - **实现初衷**：将原本堆叠在 `WaylanV2Session` 中的 35 个平铺字段，按照关注点分离原则拆解为 5 个高内聚子结构体。
//! - **核心优势**：
//!   1. 彻底打破借用检查器僵局，消除 `split_runtime_and_frontend` 防御性借用拆包代码；
//!   2. 明确协议句柄、渲染表面、焦点与环境文本、键盘修饰键跟踪及文件监控的边界；
//!   3. 零技术债与单一职责。
//! - **代价与局限**：
//!   深层字段访问需通过子结构体路径（如 `session.focus.is_active`）。

use std::time::Instant;

use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_pointer::WlPointer, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::surface::{CandidateSurfaceManager, CursorRect};

/// Wayland V2 核心输入法扩展协议句柄与事务同步上下文
#[derive(Default)]
pub struct WaylandV2ProtocolContext {
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
    /// Done 事件累计序列号（用于 commit 请求原子同步）
    pub done_serial: u32,
    /// 标记虚拟键盘是否已完成 keymap 映射表初始化
    pub has_virtual_keymap: bool,
    /// 标记 V2 会话是否已被合成器宣告不可用（触发自动降级）
    pub is_unavailable: bool,
}

/// Wayland V2 悬浮候选框表面渲染与鼠标交互上下文
#[derive(Default)]
pub struct WaylandV2SurfaceContext {
    /// 候选框透明悬浮 Surface 管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<WlBuffer>,
    /// 当前跟踪的光标坐标
    pub current_cursor: CursorRect,
    /// 鼠标指针对象（用于接收候选框上的鼠标点击与移动）
    pub pointer: Option<WlPointer>,
    /// 鼠标指针在候选表面内的当前局部物理坐标 (x, y)
    pub pointer_pos: (f64, f64),
    /// 鼠标指针当前是否位于候选表面内部
    pub pointer_in_surface: bool,
}

/// Wayland V2 会话激活、焦点与周围文本环境状态上下文
#[derive(Default)]
pub struct WaylandV2FocusContext {
    /// 当前是否处于激活会话
    pub is_active: bool,
    /// 当前激活上下文是否为密码或 PIN 输入框
    pub is_password_context: bool,
    /// 客户端同步的上下文环境文本及光标位置 (text, cursor_byte_idx, anchor_byte_idx)
    pub surrounding_text: Option<(String, u32, u32)>,
    /// 双缓冲挂起的会话激活状态（在收到 Done 事件时原子生效）
    pub pending_active: bool,
    /// 双缓冲挂起的上下文环境文本（在收到 Done 事件时原子生效）
    pub pending_surrounding_text: Option<(String, u32, u32)>,
    /// 双缓冲挂起的密码/PIN码输入框标识（在收到 Done 事件时原子生效）
    pub pending_is_password: bool,
    /// 标记当前批次是否发生了焦点激活/失焦切换
    pub pending_focus_reset: bool,
}

/// Wayland V2 键盘修饰键锁存与长按连击跟踪器
#[derive(Default)]
pub struct WaylandV2KeyboardTracker {
    /// 当前锁存的修饰键位掩码（XKB mods_latched）
    pub mods_latched: u32,
    /// 当前激活的键盘布局组索引（XKB group）
    pub mods_group: u32,
    /// 合成器下发的键盘长按连击参数 (rate_keys_per_sec, delay_ms)
    pub repeat_info: (i32, i32),
    /// 当前正处于长按连击计时中的消费按键状态 (keycode, next_fire_instant)
    pub repeat_state: Option<(u32, Instant)>,
}

/// Wayland V2 外部环境监控与会话生命周期协调器（统合类型别名）
pub type WaylandV2WatcherContext =
    crate::session_common::WaylandWatcherContext<super::WaylanV2Session>;
