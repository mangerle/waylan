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

/// 提交候选框状态至 Wayland 表面并更新活跃缓冲区（采用参数对象收敛入参），返回是否实际提交了新缓冲
pub fn commit_candidate_surface<S>(
    surface_manager: &mut CandidateSurfaceManager,
    box_state: &CandidateBoxState,
    params: CandidateSurfaceCommitParams<'_, S>,
) -> bool
where
    S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    surface_manager.commit_to_wayland(
        box_state,
        params.shm,
        params.popup_wl_surface,
        params.qhandle,
        params.active_buffer,
    )
}

/// V1 LayerSurface 提交与布局更新所需的目标上下文
pub struct V1LayerCommitParams<'a, S> {
    pub shm: Option<&'a WlShm>,
    pub popup_wl_surface: Option<&'a WlSurface>,
    pub layer_surface: Option<&'a wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub qhandle: Option<&'a QueueHandle<S>>,
    pub active_buffer: &'a mut Option<WlBuffer>,
}

/// 统一执行 V1 候选框渲染提交并同步 LayerSurface 尺寸与边距
pub fn commit_v1_layer_surface<S>(
    surface_manager: &mut CandidateSurfaceManager,
    box_state: &CandidateBoxState,
    params: V1LayerCommitParams<'_, S>,
) where
    S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    let commit_params = CandidateSurfaceCommitParams {
        shm: params.shm,
        popup_wl_surface: params.popup_wl_surface,
        qhandle: params.qhandle,
        active_buffer: params.active_buffer,
    };
    let committed = commit_candidate_surface(surface_manager, box_state, commit_params);

    if committed
        && box_state.visible
        && !box_state.candidates.is_empty()
        && let Some(layer_surf) = params.layer_surface
    {
        let scale = surface_manager.scale.max(1) as u32;
        let (phys_w, phys_h) = surface_manager.current_size;
        let logical_w = (phys_w / scale).max(1);
        let logical_h = (phys_h / scale).max(1);
        let (pos_x, pos_y) = surface_manager.compute_popup_position(logical_w, logical_h);
        layer_surf.set_size(logical_w, logical_h);
        layer_surf.set_margin(pos_y.max(0), 0, 0, pos_x.max(0));
        if let Some(surf) = params.popup_wl_surface {
            surf.commit();
        }
    }
}
