//! 候选框界面渲染状态与多层级展开状态机
//!
//! # 设计原理
//! - **实现初衷**：遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   将是否展开与选中项索引绑定在强类型枚举状态中，彻底排除“未展开却存在选中索引”等非法状态。

use serde::{Deserialize, Serialize};

use super::entity::Candidate;
use super::view::{CandidateBoxView, CandidateViewMode};

/// 语音听写与动态波形渲染状态
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoiceBoxState {
    /// 正在流式听写的部分预编辑断句文本（如“正在聆听…”或“今天天气”）
    pub partial_text: String,
    /// 动态振幅波形字符序列（如“ ▂▃▄▅”）或归一化振幅序列
    pub waveform: String,
    /// 最终识别完成的展示文本
    pub display_text: Option<String>,
    /// 是否正处于录音聆听激活状态
    pub is_listening: bool,
}

/// 二级展开卡片状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExpandedCardState {
    /// 未展开状态
    #[default]
    Collapsed,
    /// 已展开状态，携带当前选中的子候选索引（0-indexed）
    Expanded { selected_index: usize },
}

impl ExpandedCardState {
    /// 是否处于展开状态
    #[inline]
    pub fn is_expanded(&self) -> bool {
        matches!(self, Self::Expanded { .. })
    }

    /// 获取展开状态下的当前选中索引
    #[inline]
    pub fn selected_index(&self) -> Option<usize> {
        match self {
            Self::Expanded { selected_index } => Some(*selected_index),
            Self::Collapsed => None,
        }
    }
}

/// 5 行大候选网格面板展开状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GridState {
    /// 未展开状态（折叠）
    #[default]
    Collapsed,
    /// 已展开状态，携带当前选中的网格项索引（0-indexed）
    Expanded { selected_index: usize },
}

impl GridState {
    /// 是否处于大网格展开状态
    #[inline]
    pub fn is_expanded(&self) -> bool {
        matches!(self, Self::Expanded { .. })
    }

    /// 获取展开状态下的当前选中索引
    #[inline]
    pub fn selected_index(&self) -> Option<usize> {
        match self {
            Self::Expanded { selected_index } => Some(*selected_index),
            Self::Collapsed => None,
        }
    }
}

/// 候选框界面渲染状态
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateBoxState {
    /// 候选框是否可见
    pub visible: bool,
    /// 当前预编辑字符串
    pub preedit: String,
    /// 当前页的候选词列表
    pub candidates: Vec<Candidate>,
    /// 当前选中的高亮候选词索引（0-indexed）
    pub selected_index: usize,
    /// 二级扩展卡片展开状态机
    pub expanded_card: ExpandedCardState,
    /// 当前工作模式提示文字（如：“中”、“英”、“V”、“语”）
    pub mode_badge: String,
    /// 5 行大候选网格面板展开状态机
    #[serde(default)]
    pub grid_state: GridState,
    /// 展开状态下的多行网格候选词列表（通常预加载前 30 个候选词）
    pub grid_candidates: Vec<Candidate>,
    /// 语音交互原生状态（若当前处于语音录音或展示模式则为 Some）
    #[serde(default)]
    pub voice: Option<VoiceBoxState>,
}

impl CandidateBoxState {
    /// 计算并提取当前唯一确定的强类型代数视图
    pub fn view(&self) -> CandidateBoxView<'_> {
        if !self.visible {
            return CandidateBoxView::Hidden;
        }
        if let Some(ref voice) = self.voice {
            if voice.is_listening {
                return CandidateBoxView::VoiceCapsule(voice);
            }
        }
        if self.is_grid_expanded() {
            return CandidateBoxView::GridExpanded {
                grid_candidates: &self.grid_candidates,
                selected_index: self.grid_selected_index(),
            };
        }
        if self.candidates.is_empty() {
            return CandidateBoxView::Hidden;
        }
        if self.is_expanded() {
            return CandidateBoxView::ExpandedCard {
                preedit: &self.preedit,
                candidates: &self.candidates,
                selected_index: self.selected_index,
                expanded_selected_index: self.expanded_selected_index(),
                mode_badge: &self.mode_badge,
            };
        }
        CandidateBoxView::Compact {
            preedit: &self.preedit,
            candidates: &self.candidates,
            selected_index: self.selected_index,
            mode_badge: &self.mode_badge,
        }
    }

    /// 计算当前唯一确定的顶级互斥呈现形态
    #[inline]
    pub fn view_mode(&self) -> CandidateViewMode {
        match self.view() {
            CandidateBoxView::Hidden => CandidateViewMode::Hidden,
            CandidateBoxView::VoiceCapsule(_) => CandidateViewMode::VoiceCapsule,
            CandidateBoxView::GridExpanded { .. } => CandidateViewMode::GridExpanded,
            CandidateBoxView::ExpandedCard { .. } => CandidateViewMode::ExpandedCard,
            CandidateBoxView::Compact { .. } => CandidateViewMode::Compact,
        }
    }

    /// 查询当前是否处于语音听写激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.voice.as_ref().map(|v| v.is_listening).unwrap_or(false)
    }

    /// 兼容便捷判断：是否处于二级扩展卡片展开状态
    #[inline]
    pub fn is_expanded(&self) -> bool {
        self.expanded_card.is_expanded()
    }

    /// 兼容便捷获取：展开状态选中的子候选索引，若未展开返回 0
    #[inline]
    pub fn expanded_selected_index(&self) -> usize {
        self.expanded_card.selected_index().unwrap_or(0)
    }

    /// 切换或设置展开状态（排他收敛：展开卡片时自动折叠大网格）
    pub fn set_expanded(&mut self, expanded: bool, selected_index: usize) {
        if expanded {
            self.expanded_card = ExpandedCardState::Expanded { selected_index };
            self.grid_state = GridState::Collapsed;
        } else {
            self.expanded_card = ExpandedCardState::Collapsed;
        }
    }

    /// 查询当前是否处于大候选网格展开态
    #[inline]
    pub fn is_grid_expanded(&self) -> bool {
        self.grid_state.is_expanded()
    }

    /// 获取大候选网格展开态下的选中索引，未展开则返回 0
    #[inline]
    pub fn grid_selected_index(&self) -> usize {
        self.grid_state.selected_index().unwrap_or(0)
    }

    /// 切换或设置大网格展开状态机（排他收敛：展开网格时自动折叠二级卡片）
    pub fn set_grid_state(&mut self, state: GridState) {
        if state.is_expanded() {
            self.expanded_card = ExpandedCardState::Collapsed;
        }
        self.grid_state = state;
    }
}
