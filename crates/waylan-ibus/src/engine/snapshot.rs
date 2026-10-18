//! IBus 界面渲染快照提取与预编辑/候选列表格式化
//!
//! # 设计原理
//! - **实现初衷**：在锁外发射 D-Bus UI 刷新信号，必须预先从 `IbusSessionCore` 中提取不可变的状态快照。
//! - **核心优势**：锁保护范围被极限压缩为纯内存快照提取，耗时的 D-Bus 序列化与信号发送均在锁外完成。
//! - **代价与局限**：每次 UI 刷新需要浅克隆当前的候选列表与预编辑字符串。

use waylan_core::{
    Candidate, CandidateIndexStyle, CandidateOrientation, ExpandedCardState, GridState,
};
use waylan_engine::{build_common_candidate_box_state, CandidateBoxBuildOptions};

use super::super::keys::IbusSessionCore;

/// 锁外发射 UI 刷新信号所需的轻量状态快照
pub(crate) struct UiSnapshot {
    pub(crate) preedit: String,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) page_size: usize,
    pub(crate) selected_index: usize,
    pub(crate) orientation: i32,
    pub(crate) index_style: CandidateIndexStyle,
}

impl UiSnapshot {
    /// 构造空 UI 快照（零堆分配开销）
    pub(crate) fn empty() -> Self {
        Self {
            preedit: String::new(),
            candidates: Vec::new(),
            page_size: 1,
            selected_index: 0,
            orientation: super::super::types::IBUS_ORIENTATION_HORIZONTAL,
            index_style: CandidateIndexStyle::None,
        }
    }

    /// 从当前会话核心提取 UI 快照
    pub(crate) fn capture(core: &IbusSessionCore) -> Self {
        let is_voice = core.is_voice_active();
        if !is_voice && core.runtime.core.engine.raw_input().is_empty() {
            return Self::empty();
        }

        let box_state = build_common_candidate_box_state(
            &core.runtime.core.engine,
            CandidateBoxBuildOptions {
                mode: core.runtime.core.mode,
                is_active: true,
                grid_state: GridState::Collapsed,
                expanded_card: Some(core.runtime.core.expanded_card),
                inline_preedit: false,
                voice_manager: core.runtime.voice_manager.as_ref(),
            },
        );

        if is_voice {
            return Self {
                preedit: box_state.preedit,
                candidates: box_state.candidates,
                page_size: 1,
                selected_index: 0,
                orientation: super::super::types::IBUS_ORIENTATION_HORIZONTAL,
                index_style: CandidateIndexStyle::None,
            };
        }

        let is_expanded =
            core.runtime.core.is_card_expanded() || core.runtime.core.engine.is_expanded();
        let (page_size, selected_index, orientation) = if is_expanded {
            let sel = if let ExpandedCardState::Expanded { selected_index } =
                core.runtime.core.expanded_card
            {
                selected_index
            } else {
                core.runtime.core.engine.expanded_selected_index()
            };
            (
                box_state.candidates.len().max(1),
                sel,
                super::super::types::IBUS_ORIENTATION_VERTICAL,
            )
        } else {
            let orientation = match core.candidate_orientation {
                CandidateOrientation::Horizontal => {
                    super::super::types::IBUS_ORIENTATION_HORIZONTAL
                }
                CandidateOrientation::Vertical => super::super::types::IBUS_ORIENTATION_VERTICAL,
            };
            (
                core.runtime.core.engine.config.page_size,
                box_state.selected_index,
                orientation,
            )
        };

        let preedit = if core.runtime.core.engine.raw_input().starts_with('v') {
            core.runtime.core.engine.raw_input().to_string()
        } else {
            box_state.preedit
        };

        Self {
            preedit,
            candidates: box_state.candidates,
            page_size,
            selected_index,
            orientation,
            index_style: core.candidate_index_style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_engine::{AlgoConfig, AlgoEngine};

    #[test]
    fn test_ui_snapshot_voice_isolation() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);

        // 模拟正常输入状态
        let _ = core.process_key_event(b'n' as u32, 49, 0);
        let normal_snap = UiSnapshot::capture(&core);
        assert_eq!(normal_snap.preedit, "n");
        assert!(!normal_snap.candidates.is_empty());

        // 模拟语音激活状态：正在聆听中...
        core.runtime.voice_manager.as_mut().unwrap().display_text =
            Some("正在聆听中...".to_string());
        core.runtime.voice_manager.as_mut().unwrap().waveform = Some(" ▂▃▄▅".to_string());
        let voice_snap = UiSnapshot::capture(&core);

        // 核心断言：输入框预编辑必须完全为空，严禁将波形塞入输入框光标处
        assert_eq!(
            voice_snap.preedit, "",
            "录音期间预编辑必须为空以防污染输入框"
        );
        // 核心断言：语音状态必须呈现在候选框中且不带数字序号
        assert_eq!(voice_snap.candidates.len(), 1);
        assert_eq!(voice_snap.candidates[0].text, "正在聆听中...");
        assert_eq!(voice_snap.candidates[0].pinyin, " ▂▃▄▅");
        assert_eq!(voice_snap.index_style, CandidateIndexStyle::None);
    }
}
