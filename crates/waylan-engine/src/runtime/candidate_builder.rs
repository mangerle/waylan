//! 通用候选框与语音卡片状态构建器
//!
//! # 设计原理
//! - **实现初衷**：将候选框与语音听写卡片的不可变渲染快照构建逻辑从 `ImeRuntime` 调度循环中彻底剥离。
//! - **核心优势**：职责单一（SRP），消除单文件超标坏味道；方便针对候选框构建与模式徽章格式化编写高覆盖单元测试。
//! - **代价与局限**：对算法引擎与语音管理器采用借用传递。

use waylan_algo::AlgoEngine;
use waylan_core::{Candidate, CandidateBoxState, CandidateExpansion, InputMode, VoiceBoxState};

/// 候选词卡片构建参数配置
#[derive(Clone, Default)]
pub struct CandidateBoxBuildOptions {
    pub mode: InputMode,
    pub is_active: bool,
    pub page_index: usize,
    pub selected_index: usize,
    pub page_size: usize,
    pub expansion: CandidateExpansion,
    pub inline_preedit: bool,
    pub voice_state: Option<VoiceBoxState>,
}

impl CandidateBoxBuildOptions {
    /// 从核心输入会话中直接提取构建参数
    pub fn from_session(
        core: &crate::session::ImeCoreSession,
        is_active: bool,
        inline_preedit: bool,
        voice_state: Option<VoiceBoxState>,
    ) -> Self {
        Self {
            mode: core.mode,
            is_active,
            page_index: core.page_index,
            selected_index: core.selected_index,
            page_size: core.page_size,
            expansion: core.expansion,
            inline_preedit,
            voice_state,
        }
    }
}

/// 统合构建通用候选框渲染状态模型
pub fn build_common_candidate_box_state(
    engine: &AlgoEngine,
    opts: CandidateBoxBuildOptions,
) -> CandidateBoxState {
    let CandidateBoxBuildOptions {
        mode,
        is_active,
        page_index,
        selected_index,
        page_size,
        expansion: expansion_in,
        inline_preedit,
        voice_state,
    } = opts;

    // 若当前处于语音输入状态，优先呈现语音识别流式预览与动态波形（与 IBus 视觉契约 100% 对齐）
    if let Some(ref voice_box) = voice_state {
        if voice_box.is_listening {
            let candidates = if let Some(ref voice_text) = voice_box.display_text {
                vec![Candidate::new(
                    voice_text.clone(),
                    voice_box.waveform.clone(),
                    1000,
                )]
            } else {
                Vec::new()
            };

            return CandidateBoxState {
                visible: is_active,
                preedit: voice_box.partial_text.clone(),
                candidates,
                selected_index: 0,
                expansion: CandidateExpansion::None,
                mode_badge: String::new(),
                grid_candidates: Vec::new(),
                voice: Some(voice_box.clone()),
            };
        }
    }

    let all = engine.candidates();
    let size = page_size.max(1);
    let start = page_index.saturating_mul(size);
    let candidates = if start < all.len() {
        let end = (start + size).min(all.len());
        all[start..end].to_vec()
    } else {
        Vec::new()
    };

    let (expansion, grid_candidates) = match expansion_in {
        CandidateExpansion::Grid { selected_index } => {
            let take_count = all.len().min(30);
            let grid = all[..take_count].to_vec();
            (CandidateExpansion::Grid { selected_index }, grid)
        }
        card @ CandidateExpansion::Card { .. } => (card, Vec::new()),
        CandidateExpansion::None => (CandidateExpansion::None, Vec::new()),
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

    let final_selected_index = if let CandidateExpansion::Grid { selected_index } = expansion {
        selected_index
    } else {
        selected_index
    };

    CandidateBoxState {
        visible: is_active && !engine.raw_input().is_empty(),
        preedit,
        candidates,
        selected_index: final_selected_index,
        expansion,
        mode_badge,
        grid_candidates,
        voice: None,
    }
}
