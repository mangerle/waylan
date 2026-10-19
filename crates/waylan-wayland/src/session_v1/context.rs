//! Wayland V1 会话子组件上下文数据模型
//!
//! # 设计原理
//! - **实现初衷**：消除 `WaylanV1Session` 上帝结构体，将散乱平铺字段解构为协议、表面、监控 3 个高内聚子结构体。
//! - **核心优势**：
//!   1. 职责明确：区分扩展协议句柄、双缓冲像素管理与生命周期协调器；
//!   2. 借用解耦：通过独立字段借用，使按键与状态机处理天然不相交；
//!   3. 架构对齐：与 V2 会话的设计哲学保持一致。
//! - **代价与局限**：深层字段访问需通过子结构体路径（如 `session.protocol.active_context`）。

use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_keyboard::WlKeyboard, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::protocol_v1::input_method_v1::zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1;
use crate::surface::CandidateSurfaceManager;

/// Wayland V1 协议原生对象句柄与事务同步上下文
#[derive(Default)]
pub struct WaylandV1ProtocolContext {
    /// 当前激活的应用上下文引用
    pub active_context: Option<ZwpInputMethodContextV1>,
    /// 捕获的键盘对象
    pub grabbed_keyboard: Option<WlKeyboard>,
    /// V1 输入面板表面角色对象 (zwp_input_panel_surface_v1)
    pub panel_surface: Option<ZwpInputPanelSurfaceV1>,
    /// WLR Layer Shell 表面角色对象（用于 Sway / wlroots 等环境）
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    /// 当前硬件按键事件序号
    pub current_serial: u32,
    /// 文本输入上下文状态同步序号
    pub commit_serial: u32,
}

/// Wayland V1 表面渲染与像素缓冲区上下文
#[derive(Default)]
pub struct WaylandV1SurfaceContext {
    /// 候选框悬浮图层管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 关联的共享内存管理器句柄
    pub shm: Option<WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<WlBuffer>,
}

/// Wayland V1 外部环境监控与会话生命周期协调器（统合类型别名）
pub type WaylandV1WatcherContext =
    crate::session_common::WaylandWatcherContext<super::WaylanV1Session>;
