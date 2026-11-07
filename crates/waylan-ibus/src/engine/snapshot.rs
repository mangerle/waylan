//! IBus 界面渲染快照提取与预编辑/候选列表格式化
//!
//! # 设计原理
//! - **实现初衷**：在锁外发射 D-Bus UI 刷新信号，必须预先从 `IbusSessionCore` 中提取不可变的状态快照。
//! - **核心优势**：锁保护范围被极限压缩为纯内存快照提取，耗时的 D-Bus 序列化与信号发送均在锁外完成。
//! - **代价与局限**：每次 UI 刷新需要浅克隆当前的候选列表与预编辑字符串。

use waylan_core::{Candidate, CandidateBoxState, CandidateIndexStyle, CandidateOrientation};
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
    ///
    /// # 设计原理
    /// - 快照提取全程只读且在锁外完成，语音激活态与常规组词态分别走独立的快照构造路径，
    ///   确保语音波形绝不会污染输入框预编辑区。
    pub(crate) fn capture(core: &IbusSessionCore) -> Self {
        let is_voice = core.is_voice_active();
        if !is_voice && core.runtime.core.engine.raw_input().is_empty() {
            return Self::empty();
        }

        let voice_state = core
            .runtime
            .voice_manager()
            .and_then(|vm| vm.render_state());

        let box_state = build_common_candidate_box_state(
            &core.runtime.core.engine,
            CandidateBoxBuildOptions::from_session(&core.runtime.core, true, false, voice_state),
        );

        if is_voice {
            return Self::from_voice_box_state(box_state);
        }

        let candidates = core.current_candidates();
        let (page_size, selected_index, orientation) =
            Self::resolve_page_layout(core, &box_state, candidates.len());
        let preedit = Self::resolve_preedit(core, box_state.preedit);

        Self {
            preedit,
            candidates,
            page_size,
            selected_index,
            orientation,
            index_style: core.candidate_index_style,
        }
    }

    /// 构造语音激活态下的界面快照
    ///
    /// # 设计原理
    /// - 语音态固定为单条目候选列表且不带数字序号，候选区承载语音文本与波形。
    fn from_voice_box_state(box_state: CandidateBoxState) -> Self {
        Self {
            preedit: box_state.preedit,
            candidates: box_state.candidates,
            page_size: 1,
            selected_index: 0,
            orientation: super::super::types::IBUS_ORIENTATION_HORIZONTAL,
            index_style: CandidateIndexStyle::None,
        }
    }

    /// 依据卡片或网格展开状态与候选框布局推导页大小、选中索引与候选框方向
    ///
    /// # 设计原理
    /// - 二级卡片展开态强制纵向排列并以卡片内选中项为准，页大小与展开子项条数对齐；
    /// - 大网格展开态同样强制纵向排列并将页大小撑满展开条目数；
    /// - 常规态则沿用引擎配置页大小与候选框自身的选中索引。
    fn resolve_page_layout(
        core: &IbusSessionCore,
        box_state: &CandidateBoxState,
        candidates_len: usize,
    ) -> (usize, usize, i32) {
        if core.runtime.core.is_card_expanded() {
            let sel = core.runtime.core.card_selected_index();
            return (
                candidates_len.max(1),
                sel,
                super::super::types::IBUS_ORIENTATION_VERTICAL,
            );
        }
        if core.runtime.core.is_grid_expanded() {
            let sel = core.runtime.core.grid_selected_index();
            return (
                candidates_len.max(1),
                sel,
                super::super::types::IBUS_ORIENTATION_VERTICAL,
            );
        }
        let orientation = match core.candidate_orientation {
            CandidateOrientation::Horizontal => super::super::types::IBUS_ORIENTATION_HORIZONTAL,
            CandidateOrientation::Vertical => super::super::types::IBUS_ORIENTATION_VERTICAL,
        };
        (
            core.runtime.core.engine.config.page_size,
            box_state.selected_index,
            orientation,
        )
    }

    /// 解析预编辑文本：V 模式保留原始拼音串，其余场景沿用候选框预编辑
    fn resolve_preedit(core: &IbusSessionCore, box_preedit: String) -> String {
        if core.runtime.core.engine.raw_input().starts_with('v') {
            core.runtime.core.engine.raw_input().to_string()
        } else {
            box_preedit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::VoiceConfig;
    use waylan_engine::{AlgoConfig, AlgoEngine};
    use waylan_voice::VoiceInteractionManager;

    #[test]
    fn test_ui_snapshot_voice_isolation() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine).with_voice_provider(Box::new(
            VoiceInteractionManager::new(VoiceConfig::default()),
        ));

        // 模拟正常输入状态
        let _ = core.process_key_event(b'n' as u32, 49, 0);
        let normal_snap = UiSnapshot::capture(&core);
        assert_eq!(normal_snap.preedit, "n");
        assert!(!normal_snap.candidates.is_empty());

        // 模拟语音激活状态：正在聆听中...
        core.runtime
            .voice_manager_mut()
            .expect("测试语音管理器应存在")
            .set_display_text(Some("正在聆听中...".to_string()));
        core.runtime
            .voice_manager_mut()
            .expect("测试语音管理器应存在")
            .set_waveform(Some(" ▂▃▄▅".to_string()));
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

    #[test]
    fn test_ui_snapshot_date_tab_expansion() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine);

        // 模拟输入 "riqi"
        for ch in ['r', 'i', 'q', 'i'] {
            let _ = core.process_key_event(ch as u32, 0, 0);
        }

        // 展开前：快照应包含普通首屏候选词（首位为“日期”，带有可展开项）
        let initial_snap = UiSnapshot::capture(&core);
        assert!(initial_snap.candidates.len() >= 2);
        assert_eq!(initial_snap.candidates[0].text, "日期");

        // 按 Tab 键触发展开
        let tab_out = core.process_key_event(0xff09, waylan_core::input::constants::KEY_TAB, 0);
        assert!(tab_out.consumed);
        assert!(core.runtime.core.is_card_expanded());

        // 展开后：快照必须提取全部 7 种细分日期格式，且强制设置为纵向排布展示
        let expanded_snap = UiSnapshot::capture(&core);
        assert_eq!(expanded_snap.candidates.len(), 7);
        assert!(expanded_snap.candidates[0].text.contains('年'));
        assert_eq!(
            expanded_snap.orientation,
            crate::types::IBUS_ORIENTATION_VERTICAL
        );
        assert_eq!(expanded_snap.page_size, 7);
        assert_eq!(expanded_snap.selected_index, 0);
    }
}
