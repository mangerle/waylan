//! IBus 界面渲染快照提取与预编辑/候选列表格式化
//!
//! # 设计原理
//! - **实现初衷**：在锁外发射 D-Bus UI 刷新信号，必须预先从 `IbusSessionCore` 中提取不可变的状态快照。
//! - **核心优势**：锁保护范围被极限压缩为纯内存快照提取，耗时的 D-Bus 序列化与信号发送均在锁外完成。
//! - **代价与局限**：每次 UI 刷新需要浅克隆当前的候选列表与预编辑字符串。

use waylan_core::{
    Candidate, CandidateBoxState, CandidateExpansion, CandidateIndexStyle, CandidateOrientation,
};
use waylan_engine::{CandidateBoxBuildOptions, build_common_candidate_box_state};

use super::super::keys::IbusSessionCore;

/// 锁外发射 UI 刷新信号所需的轻量状态快照
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiSnapshot {
    pub(crate) preedit: String,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) page_size: usize,
    pub(crate) selected_index: usize,
    pub(crate) orientation: i32,
    pub(crate) index_style: CandidateIndexStyle,
    pub(crate) show_pinyin_comment: bool,
}

/// 从候选列表与选中下标提取二级卡片展开子项集合（供界面快照与按键核心共用，消除重复提取）
pub(crate) fn extract_expanded_card_candidates(
    candidates: &[Candidate],
    selected_index: usize,
) -> Vec<Candidate> {
    let cand_opt = candidates
        .get(selected_index)
        .filter(|c| c.is_expandable && !c.sub_candidates.is_empty())
        .or_else(|| {
            candidates
                .iter()
                .find(|c| c.is_expandable && !c.sub_candidates.is_empty())
        });
    cand_opt
        .map(|cand| {
            cand.sub_candidates
                .iter()
                .map(|sub| Candidate::new(sub.clone(), cand.pinyin.clone(), cand.score))
                .collect()
        })
        .unwrap_or_default()
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
            show_pinyin_comment: false,
        }
    }

    /// 由统一核心渲染状态 [`CandidateBoxState`] 直接映射构建 IBus 界面快照
    ///
    /// # 设计原理
    /// - 彻底消除 `IbusFrontend` 丢弃 `CandidateBoxState` 后再由 `UiSnapshot::capture` 二次重算的冗余路径。
    pub(crate) fn from_candidate_box_state(
        box_state: &CandidateBoxState,
        page_size: usize,
        orientation: CandidateOrientation,
        index_style: CandidateIndexStyle,
        show_pinyin_comment: bool,
    ) -> Self {
        if box_state.voice.is_some() {
            return Self {
                preedit: box_state.preedit.clone(),
                candidates: box_state.candidates.clone(),
                page_size: 1,
                selected_index: 0,
                orientation: super::super::types::IBUS_ORIENTATION_HORIZONTAL,
                index_style: CandidateIndexStyle::None,
                show_pinyin_comment,
            };
        }

        match box_state.expansion {
            CandidateExpansion::Card { selected_index } => {
                let candidates = extract_expanded_card_candidates(
                    &box_state.candidates,
                    box_state.selected_index,
                );
                let count = candidates.len().max(1);
                Self {
                    preedit: box_state.preedit.clone(),
                    candidates,
                    page_size: count,
                    selected_index,
                    orientation: super::super::types::IBUS_ORIENTATION_VERTICAL,
                    index_style,
                    show_pinyin_comment,
                }
            }
            CandidateExpansion::Grid { selected_index } => {
                let candidates = box_state.grid_candidates.clone();
                let count = candidates.len().max(1);
                Self {
                    preedit: box_state.preedit.clone(),
                    candidates,
                    page_size: count,
                    selected_index,
                    orientation: super::super::types::IBUS_ORIENTATION_VERTICAL,
                    index_style,
                    show_pinyin_comment,
                }
            }
            CandidateExpansion::None => {
                let ibus_orientation = match orientation {
                    CandidateOrientation::Horizontal => {
                        super::super::types::IBUS_ORIENTATION_HORIZONTAL
                    }
                    CandidateOrientation::Vertical => {
                        super::super::types::IBUS_ORIENTATION_VERTICAL
                    }
                };
                Self {
                    preedit: box_state.preedit.clone(),
                    candidates: box_state.candidates.clone(),
                    page_size: page_size.max(1),
                    selected_index: box_state.selected_index,
                    orientation: ibus_orientation,
                    index_style,
                    show_pinyin_comment,
                }
            }
        }
    }

    /// 从当前会话核心提取 UI 快照（供点击选词、焦点重置等非按键直通路径使用）
    pub(crate) fn capture(core: &IbusSessionCore) -> Self {
        let is_voice = core.is_voice_active();
        if !is_voice && core.runtime.core.engine.raw_input().is_empty() {
            return Self::empty();
        }

        let voice_state = core
            .runtime
            .voice_manager()
            .and_then(|vm| vm.render_state());

        let mut box_state = build_common_candidate_box_state(
            &core.runtime.core.engine,
            CandidateBoxBuildOptions::from_session(&core.runtime.core, true, false, voice_state),
        );
        if !is_voice {
            box_state.preedit = Self::resolve_preedit(core, box_state.preedit);
        }

        Self::from_candidate_box_state(
            &box_state,
            core.runtime.core.engine.config.page_size,
            core.runtime.candidate_orientation(),
            core.runtime.candidate_index_style(),
            core.runtime.core.engine.config.show_pinyin_comment,
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
    use waylan_core::input::{VoiceKeyAction, VoiceKeyDispatchParams, VoiceProvider};
    use waylan_core::{VoiceBoxState, VoiceConfig};
    use waylan_engine::{AlgoConfig, AlgoEngine};

    /// 轻量模拟语音提供者（仅用于验证候选框快照隔离，避免引入完整语音引擎依赖）
    struct MockVoiceProvider {
        config: VoiceConfig,
        display_text: Option<String>,
        partial_text: Option<String>,
        waveform: Option<String>,
    }

    impl MockVoiceProvider {
        fn new(config: VoiceConfig) -> Self {
            Self {
                config,
                display_text: None,
                partial_text: None,
                waveform: None,
            }
        }
    }

    impl VoiceProvider for MockVoiceProvider {
        fn is_enabled(&self) -> bool {
            self.config.enabled
        }
        fn config(&self) -> &VoiceConfig {
            &self.config
        }
        fn is_push_to_talk_recording(&self) -> bool {
            false
        }
        fn is_active(&self) -> bool {
            self.display_text.is_some()
        }
        fn is_recording(&self) -> bool {
            false
        }
        fn cancel(&mut self) {}
        fn clear_state(&mut self) {
            self.display_text = None;
            self.partial_text = None;
            self.waveform = None;
        }
        fn update_config(&mut self, cfg: &VoiceConfig) {
            self.config = cfg.clone();
        }
        fn poll_events(&mut self) -> Option<String> {
            None
        }
        fn dispatch_key(&mut self, _params: VoiceKeyDispatchParams<'_>) -> VoiceKeyAction {
            VoiceKeyAction::PassThrough
        }
        fn display_text(&self) -> Option<String> {
            self.display_text.clone()
        }
        fn set_display_text(&mut self, text: Option<String>) {
            self.display_text = text;
        }
        fn partial_text(&self) -> Option<String> {
            self.partial_text.clone()
        }
        fn set_partial_text(&mut self, text: Option<String>) {
            self.partial_text = text;
        }
        fn set_waveform(&mut self, waveform: Option<String>) {
            self.waveform = waveform;
        }
        fn render_state(&self) -> Option<VoiceBoxState> {
            if !self.is_active() {
                return None;
            }
            Some(VoiceBoxState {
                partial_text: self.partial_text.clone().unwrap_or_default(),
                waveform: self.waveform.clone().unwrap_or_default(),
                display_text: self.display_text.clone(),
                is_listening: true,
                visual_effect: self.config.interaction.visual_effect,
            })
        }
    }

    #[test]
    fn test_ui_snapshot_voice_isolation() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut core = IbusSessionCore::new(engine)
            .with_voice_provider(Box::new(MockVoiceProvider::new(VoiceConfig::default())));

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
