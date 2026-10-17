//! 候选框状态装配与表面提交逻辑
//!
//! # 设计原理
//! - **实现初衷**：收敛 Wayland V1 与 V2 会话中从拼音引擎、输入模式与语音状态组装 CandidateBoxState 的统一逻辑。
//! - **核心优势**：消除 V1 和 V2 对 Candidate、ExpandedCardState 与 VoiceBoxState 的重复转换代码。
//! - **代价与局限**：提供参数对象 CandidateSurfaceCommitParams 收敛 Wayland 渲染提交入参。

use waylan_algo::AlgoEngine;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_voice::VoiceInteractionManager;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

use crate::surface::CandidateSurfaceManager;

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

/// 统一构建候选框渲染状态的核心数据（收敛 V1 和 V2 对 Candidate 与 ExpandedCardState 的转换）
pub fn build_common_candidate_box_state(
    engine: &AlgoEngine,
    mode: InputMode,
    is_active: bool,
    is_grid_expanded: bool,
    grid_selected_index: usize,
    inline_preedit: bool,
    voice_manager: Option<&VoiceInteractionManager>,
) -> CandidateBoxState {
    use waylan_core::{Candidate, ExpandedCardState};

    // 若当前处于语音输入状态，优先呈现语音识别流式预览与动态波形（与 IBus 视觉契约 100% 对齐）
    if let Some(vm) = voice_manager {
        if vm.is_active() {
            let partial_text = vm.partial_text.clone().unwrap_or_default();
            let waveform = vm.waveform.clone().unwrap_or_default();
            let display_text = vm.display_text.clone();

            let voice_state = waylan_core::VoiceBoxState {
                partial_text: partial_text.clone(),
                waveform: waveform.clone(),
                display_text: display_text.clone(),
                is_listening: true,
            };

            let candidates = if let Some(ref voice_text) = display_text {
                vec![Candidate::new(voice_text.clone(), waveform.clone(), 1000)]
            } else {
                Vec::new()
            };

            return CandidateBoxState {
                visible: is_active,
                preedit: partial_text,
                candidates,
                selected_index: 0,
                expanded_card: ExpandedCardState::Collapsed,
                mode_badge: String::new(),
                is_grid_expanded: false,
                grid_candidates: Vec::new(),
                voice: Some(voice_state),
            };
        }
    }

    let core_candidates: Vec<Candidate> = engine.candidates().iter().map(Candidate::from).collect();

    let grid_candidates: Vec<Candidate> = if is_grid_expanded {
        engine
            .all_candidates()
            .iter()
            .take(30)
            .map(Candidate::from)
            .collect()
    } else {
        Vec::new()
    };

    let expanded_card = if engine.is_expanded() {
        ExpandedCardState::Expanded {
            selected_index: engine.expanded_selected_index(),
        }
    } else {
        ExpandedCardState::Collapsed
    };

    let is_dynamic_v_mode = engine.raw_input().starts_with('v');
    let (preedit, mode_badge) = if inline_preedit {
        if is_dynamic_v_mode {
            (engine.raw_input().to_string(), "V模式".to_string())
        } else {
            match mode {
                InputMode::Chinese => (String::new(), String::new()),
                InputMode::English => (String::new(), "英".to_string()),
                InputMode::VMode => (engine.formatted_preedit(), "V".to_string()),
            }
        }
    } else {
        let badge = match mode {
            InputMode::Chinese => "中".to_string(),
            InputMode::English => "英".to_string(),
            InputMode::VMode => "V".to_string(),
        };
        (engine.formatted_preedit(), badge)
    };

    let selected_index = if is_grid_expanded {
        grid_selected_index
    } else {
        engine.selected_index()
    };

    CandidateBoxState {
        visible: is_active && !engine.raw_input().is_empty(),
        preedit,
        candidates: core_candidates,
        selected_index,
        expanded_card,
        mode_badge,
        is_grid_expanded,
        grid_candidates,
        voice: None,
    }
}

/// 隐藏候选框并解绑关联的 Wayland 表面缓冲区
pub fn hide_candidate_surface_buffer(
    surface_manager: &mut CandidateSurfaceManager,
    popup_wl_surface: Option<&WlSurface>,
    active_buffer: &mut Option<WlBuffer>,
) {
    surface_manager.hide_wayland_surface(popup_wl_surface, active_buffer);
}
