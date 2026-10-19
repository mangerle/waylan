//! 通用候选框与语音卡片状态构建器
//!
//! # 设计原理
//! - **实现初衷**：将候选框与语音听写卡片的不可变渲染快照构建逻辑从 `ImeRuntime` 调度循环中彻底剥离。
//! - **核心优势**：职责单一（SRP），消除单文件超标坏味道；方便针对候选框构建与模式徽章格式化编写高覆盖单元测试。
//! - **代价与局限**：对算法引擎与语音管理器采用借用传递。

#[cfg(not(feature = "voice"))]
use super::voice_stub::VoiceInteractionManager;
use waylan_algo::AlgoEngine;
use waylan_core::{
    Candidate, CandidateBoxState, ExpandedCardState, GridState, InputMode, VoiceBoxState,
};
#[cfg(feature = "voice")]
use waylan_voice::VoiceInteractionManager;

/// 候选词卡片构建参数配置
#[derive(Clone, Copy)]
pub struct CandidateBoxBuildOptions<'a> {
    pub mode: InputMode,
    pub is_active: bool,
    pub grid_state: GridState,
    pub expanded_card: Option<ExpandedCardState>,
    pub inline_preedit: bool,
    pub voice_manager: Option<&'a VoiceInteractionManager>,
}

/// 统合构建通用候选框渲染状态模型
pub fn build_common_candidate_box_state(
    engine: &AlgoEngine,
    opts: CandidateBoxBuildOptions<'_>,
) -> CandidateBoxState {
    let CandidateBoxBuildOptions {
        mode,
        is_active,
        grid_state,
        expanded_card: card_opt,
        inline_preedit,
        voice_manager,
    } = opts;

    // 若当前处于语音输入状态，优先呈现语音识别流式预览与动态波形（与 IBus 视觉契约 100% 对齐）
    if let Some(vm) = voice_manager {
        if vm.is_active() {
            let partial_text = vm.partial_text.clone().unwrap_or_default();
            let waveform = vm.waveform.clone().unwrap_or_default();
            let display_text = vm.display_text.clone();

            let voice_state = VoiceBoxState {
                partial_text: partial_text.clone(),
                waveform: waveform.clone(),
                display_text: display_text.clone(),
                is_listening: true,
            };

            let candidates = if let Some(ref voice_text) = display_text {
                vec![Candidate::new(voice_text.clone(), waveform, 1000)]
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
                grid_state: GridState::Collapsed,
                grid_candidates: Vec::new(),
                voice: Some(voice_state),
            };
        }
    }

    let candidates_slice = engine.candidates();
    let mut core_candidates = Vec::with_capacity(candidates_slice.len());
    for ac in candidates_slice {
        core_candidates.push(ac.to_core_candidate());
    }

    let (expanded_card, grid_state_resolved, grid_candidates) = if grid_state.is_expanded() {
        let all = engine.all_candidates();
        let take_count = all.len().min(30);
        let mut grid = Vec::with_capacity(take_count);
        for ac in all.iter().take(take_count) {
            grid.push(ac.to_core_candidate());
        }
        (ExpandedCardState::Collapsed, grid_state, grid)
    } else if let Some(card @ ExpandedCardState::Expanded { .. }) = card_opt {
        (card, GridState::Collapsed, Vec::new())
    } else {
        (
            ExpandedCardState::Collapsed,
            GridState::Collapsed,
            Vec::new(),
        )
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

    let selected_index = if let Some(idx) = grid_state_resolved.selected_index() {
        idx
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
        grid_state: grid_state_resolved,
        grid_candidates,
        voice: None,
    }
}
