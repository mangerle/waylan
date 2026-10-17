//! IBus 界面渲染快照提取与预编辑/候选列表格式化
//!
//! # 设计原理
//! - **实现初衷**：在锁外发射 D-Bus UI 刷新信号，必须预先从 `IbusSessionCore` 中提取不可变的状态快照。
//! - **核心优势**：锁保护范围被极限压缩为纯内存快照提取，耗时的 D-Bus 序列化与信号发送均在锁外完成。
//! - **代价与局限**：每次 UI 刷新需要浅克隆当前的候选列表与预编辑字符串。

use waylan_core::{Candidate, CandidateIndexStyle, CandidateOrientation};

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
    /// 提取处于语音录入或语音展示期间的 UI 快照
    fn capture_voice_snapshot(core: &IbusSessionCore) -> Self {
        let candidates = if let Some(ref voice_text) = core.voice_manager.display_text {
            vec![Candidate::new(
                voice_text.clone(),
                core.voice_manager.waveform.clone().unwrap_or_default(),
                1000,
            )]
        } else {
            Vec::new()
        };
        let preedit = core.voice_manager.partial_text.clone().unwrap_or_default();
        Self {
            preedit,
            candidates,
            page_size: 1,
            selected_index: 0,
            orientation: super::super::types::IBUS_ORIENTATION_HORIZONTAL,
            index_style: CandidateIndexStyle::None,
        }
    }

    /// 提取常规键盘输入组词状态下的 UI 快照
    fn capture_input_snapshot(core: &IbusSessionCore) -> Self {
        let is_v_mode = core.core.engine.raw_input().starts_with('v');
        let preedit = if core.core.engine.raw_input().is_empty() {
            String::new()
        } else if is_v_mode {
            core.core.engine.raw_input().to_string()
        } else {
            core.core.engine.formatted_preedit()
        };
        let is_expanded = core.core.engine.is_expanded();
        let candidates = core.current_candidates();
        let (page_size, selected_index, orientation) = if is_expanded {
            (
                candidates.len().max(1),
                core.core.engine.expanded_selected_index(),
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
                core.core.engine.config.page_size,
                core.core.engine.selected_index(),
                orientation,
            )
        };
        Self {
            preedit,
            candidates,
            page_size,
            selected_index,
            orientation,
            index_style: core.candidate_index_style,
        }
    }

    /// 从当前会话核心提取 UI 快照
    pub(crate) fn capture(core: &IbusSessionCore) -> Self {
        if core.is_voice_active() {
            Self::capture_voice_snapshot(core)
        } else {
            Self::capture_input_snapshot(core)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_algo::{AlgoConfig, AlgoEngine};

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
        core.voice_manager.display_text = Some("正在聆听中...".to_string());
        core.voice_manager.waveform = Some(" ▂▃▄▅".to_string());
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
