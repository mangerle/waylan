//! 候选框界面渲染状态与多层级展开状态机
//!
//! # 设计原理
//! - **实现初衷**：遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   将原平行的二级大卡片与五行大网格展开状态收敛为单一互斥枚举 `CandidateExpansion`，
//!   彻底排除“未展开却存在选中索引”以及“大卡片与大网格同时处于展开状态”的矛盾组合。
//! - **核心优势**：
//!   1. 编译期保证单选互斥，杜绝外部赋值或反序列化组合漏洞导致的双重展开；
//!   2. 减少状态机字段冗余，提升可读性与可维护性；
//!   3. 保留完备的辅助方法，平滑兼容消费侧 API。
//! - **代价与局限**：
//!   结构体字段由两项收敛为一项。

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

/// 候选框展开交互状态枚举（二级大卡片与多行大网格在类型层面严格互斥）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CandidateExpansion {
    /// 常规紧凑折叠状态（未展开）
    #[default]
    None,
    /// 二级扩展大卡片展开状态，携带当前选中的子候选索引（0-indexed）
    Card { selected_index: usize },
    /// 5 行大候选网格面板展开状态，携带当前选中的网格项索引（0-indexed）
    Grid { selected_index: usize },
}

impl CandidateExpansion {
    /// 查询是否处于二级扩展大卡片展开状态
    #[inline]
    pub fn is_card_expanded(&self) -> bool {
        matches!(self, Self::Card { .. })
    }

    /// 查询是否处于 5 行大网格展开状态
    #[inline]
    pub fn is_grid_expanded(&self) -> bool {
        matches!(self, Self::Grid { .. })
    }

    /// 查询是否处于任意形式的展开状态
    #[inline]
    pub fn is_expanded(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// 获取展开状态下的当前选中索引（无论卡片还是网格）
    #[inline]
    pub fn selected_index(&self) -> Option<usize> {
        match self {
            Self::Card { selected_index } | Self::Grid { selected_index } => Some(*selected_index),
            Self::None => None,
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
    /// 候选框二级展开交互状态（大卡片与多行大网格严格互斥）
    #[serde(default)]
    pub expansion: CandidateExpansion,
    /// 当前工作模式提示文字（如：“中”、“英”、“V”、“语”）
    pub mode_badge: String,
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
        if let CandidateExpansion::Grid { selected_index } = self.expansion {
            return CandidateBoxView::GridExpanded {
                grid_candidates: &self.grid_candidates,
                selected_index,
            };
        }
        if self.candidates.is_empty() {
            return CandidateBoxView::Hidden;
        }
        if let CandidateExpansion::Card { selected_index } = self.expansion {
            return CandidateBoxView::ExpandedCard {
                preedit: &self.preedit,
                candidates: &self.candidates,
                selected_index: self.selected_index,
                expanded_selected_index: selected_index,
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

    /// 便捷方法：是否处于二级扩展卡片展开状态
    #[inline]
    pub fn is_expanded(&self) -> bool {
        self.expansion.is_card_expanded()
    }

    /// 便捷方法：展开卡片选中的子候选索引，若未展开返回 0
    #[inline]
    pub fn expanded_selected_index(&self) -> usize {
        match self.expansion {
            CandidateExpansion::Card { selected_index } => selected_index,
            _ => 0,
        }
    }

    /// 切换或设置二级扩展卡片展开状态
    pub fn set_expanded(&mut self, expanded: bool, selected_index: usize) {
        if expanded {
            self.expansion = CandidateExpansion::Card { selected_index };
        } else if self.expansion.is_card_expanded() {
            self.expansion = CandidateExpansion::None;
        }
    }

    /// 便捷方法：查询当前是否处于大候选网格展开态
    #[inline]
    pub fn is_grid_expanded(&self) -> bool {
        self.expansion.is_grid_expanded()
    }

    /// 获取大候选网格展开态下的选中索引，未展开则返回 0
    #[inline]
    pub fn grid_selected_index(&self) -> usize {
        match self.expansion {
            CandidateExpansion::Grid { selected_index } => selected_index,
            _ => 0,
        }
    }

    /// 切换或设置大网格展开状态机
    pub fn set_grid_expanded(&mut self, expanded: bool, selected_index: usize) {
        if expanded {
            self.expansion = CandidateExpansion::Grid { selected_index };
        } else if self.expansion.is_grid_expanded() {
            self.expansion = CandidateExpansion::None;
        }
    }

    /// 直接设置完整的展开交互状态
    pub fn set_expansion(&mut self, expansion: CandidateExpansion) {
        self.expansion = expansion;
    }
}
