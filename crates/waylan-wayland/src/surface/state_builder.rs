//! 候选框状态装配与表面提交逻辑
//!
//! # 设计原理
//! - **实现初衷**：收敛 Wayland V1 与 V2 会话中从拼音引擎、输入模式与语音状态组装 CandidateBoxState 的统一逻辑。
//! - **核心优势**：消除 V1 和 V2 对 Candidate、ExpandedCardState 与 VoiceBoxState 的重复转换代码。
//! - **代价与局限**：提供参数对象 CandidateSurfaceCommitParams 收敛 Wayland 渲染提交入参。

use waylan_core::CandidateBoxState;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

use crate::surface::CandidateSurfaceManager;

pub use waylan_engine::{CandidateBoxBuildOptions, build_common_candidate_box_state};

/// 提交候选框图层所需的 Wayland 目标上下文参数对象
pub struct CandidateSurfaceCommitParams<'a, S> {
    /// 共享内存协议句柄
    pub shm: Option<&'a WlShm>,
    /// 候选框目标表面句柄
    pub popup_wl_surface: Option<&'a WlSurface>,
    /// 事件队列句柄
    pub qhandle: Option<&'a QueueHandle<S>>,
    /// 活跃像素缓冲区借用
    pub active_buffer: &'a mut Option<WlBuffer>,
}

/// 提交候选框状态至 Wayland 表面并更新活跃缓冲区（采用参数对象收敛入参）
pub fn commit_candidate_surface<S>(
    surface_manager: &mut CandidateSurfaceManager,
    box_state: &CandidateBoxState,
    params: CandidateSurfaceCommitParams<'_, S>,
) where
    S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    surface_manager.commit_to_wayland(
        box_state,
        params.shm,
        params.popup_wl_surface,
        params.qhandle,
        params.active_buffer,
    );
}

/// 隐藏候选框并解绑关联的 Wayland 表面缓冲区
pub fn hide_candidate_surface_buffer(
    surface_manager: &mut CandidateSurfaceManager,
    popup_wl_surface: Option<&WlSurface>,
    active_buffer: &mut Option<WlBuffer>,
) {
    surface_manager.hide_wayland_surface(popup_wl_surface, active_buffer);
}
