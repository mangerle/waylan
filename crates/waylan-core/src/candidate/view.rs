//! 候选框界面顶级互斥呈现形态与代数视图
//!
//! # 设计原理
//! - **实现初衷**：遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   将隐藏、语音胶囊、网格展开、二级卡片展开与普通紧凑条收敛为单层严格互斥枚举。

use serde::{Deserialize, Serialize};

use super::entity::Candidate;
use super::state::VoiceBoxState;

/// 候选框界面顶级互斥呈现形态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CandidateViewMode {
    /// 隐藏或未激活状态
    #[default]
    Hidden,
    /// 语音听写流式波形与胶囊展示态
    VoiceCapsule,
    /// 5 行大候选网格展开态
    GridExpanded,
    /// 二级扩展卡片展开态（如日期时间格式多选）
    ExpandedCard,
    /// 标准单行/紧凑候选条展示态
    Compact,
}

/// 候选框界面强类型代数视图（物理杜绝非法状态组合）
#[derive(Debug, PartialEq, Eq)]
pub enum CandidateBoxView<'a> {
    /// 隐藏或未激活状态
    Hidden,
    /// 语音听写流式波形与胶囊展示态
    VoiceCapsule(&'a VoiceBoxState),
    /// 5 行大候选网格展开态
    GridExpanded {
        grid_candidates: &'a [Candidate],
        selected_index: usize,
    },
    /// 二级扩展卡片展开态
    ExpandedCard {
        preedit: &'a str,
        candidates: &'a [Candidate],
        selected_index: usize,
        expanded_selected_index: usize,
        mode_badge: &'a str,
    },
    /// 标准单行/紧凑候选条展示态
    Compact {
        preedit: &'a str,
        candidates: &'a [Candidate],
        selected_index: usize,
        mode_badge: &'a str,
    },
}
