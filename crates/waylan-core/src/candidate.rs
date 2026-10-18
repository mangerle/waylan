//! 候选词、候选框展示状态与输入法动作领域模型
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、UI 渲染引擎、语音转录与 Wayland/IBus 协议前端提供统一的候选词与动作契约。
//! - **核心优势**：严格遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   通过强类型枚举彻底排除未展开却存在选中索引等非法状态组合。
//! - **代价与局限**：作为跨 Crate 流转的核心数据契约，字段改动需评估向下兼容性。

use serde::{Deserialize, Serialize};

/// 候选词信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// 候选文字（如：“你好”）
    pub text: String,
    /// 匹配的拼音序列（如：“ni'hao”）
    pub pinyin: String,
    /// 候选词排序得分或频次
    pub score: u32,
    /// 是否为可展开的二级动态功能项
    pub is_expandable: bool,
    /// 二级子候选列表（如多种展开的日期或时间格式）
    pub sub_candidates: Vec<String>,
    /// 候选附加注释信息（如多音字、反查或注音提示）
    #[serde(default)]
    pub comment: Option<String>,
}

impl Candidate {
    /// 创建基础候选词
    pub fn new(text: impl Into<String>, pinyin: impl Into<String>, score: u32) -> Self {
        Self {
            text: text.into(),
            pinyin: pinyin.into(),
            score,
            is_expandable: false,
            sub_candidates: Vec::new(),
            comment: None,
        }
    }

    /// 标记为可展开并挂载二级子候选格式列表
    pub fn with_sub_candidates(mut self, sub_candidates: Vec<String>) -> Self {
        self.is_expandable = !sub_candidates.is_empty();
        self.sub_candidates = sub_candidates;
        self
    }

    /// 附加注释说明（如多音字注音或来源提示）
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}

/// 输入法向外部分发的核心动作指令
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAction {
    /// 设置应用文本框内的带下划线预编辑文本
    SetPreedit(String),
    /// 更新当前候选词列表
    SetCandidates(Vec<Candidate>),
    /// 向当前焦点应用正式提交上屏文本
    Commit(String),
    /// 请求系统移动光标（例如成对符号补全时光标左移一格）
    MoveCursor(i32),
    /// 清空预编辑与候选，重置状态
    Reset,
}

/// 输入法当前的工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// 中文拼音输入模式
    #[default]
    Chinese,
    /// 英文直接穿透模式
    English,
    /// V 模式（计算器、特殊符号等快捷工具）
    VMode,
}

/// 二级展开卡片状态
///
/// 遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”的设计原则，
/// 将是否展开与选中项索引绑定在枚举状态中，防止出现“未展开却存在选中索引”的无效状态。
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
///
/// 遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”的设计原则，
/// 将是否展开与选中项索引绑定在枚举状态中，防止出现“未展开却存在选中索引”的无效状态。
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

    /// 切换或设置展开状态
    pub fn set_expanded(&mut self, expanded: bool, selected_index: usize) {
        if expanded {
            self.expanded_card = ExpandedCardState::Expanded { selected_index };
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

    /// 切换或设置大网格展开状态机
    pub fn set_grid_state(&mut self, state: GridState) {
        self.grid_state = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_creation() {
        let c = Candidate::new("你好", "ni'hao", 100);
        assert_eq!(c.text, "你好");
        assert_eq!(c.pinyin, "ni'hao");
        assert_eq!(c.score, 100);
        assert!(!c.is_expandable);
        assert!(c.sub_candidates.is_empty());
        assert_eq!(c.comment, None);

        let c_expanded = Candidate::new("今天", "jin'tian", 90)
            .with_sub_candidates(vec!["2026-10-18".to_string(), "2026年10月18日".to_string()])
            .with_comment("日期");
        assert!(c_expanded.is_expandable);
        assert_eq!(c_expanded.sub_candidates.len(), 2);
        assert_eq!(c_expanded.comment, Some("日期".to_string()));
    }

    #[test]
    fn test_candidate_box_state_defaults() {
        let state = CandidateBoxState::default();
        assert!(!state.visible);
        assert_eq!(state.preedit, "");
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.expanded_card, ExpandedCardState::Collapsed);
        assert_eq!(state.mode_badge, "");
        assert!(!state.is_grid_expanded());
        assert!(state.grid_candidates.is_empty());
        assert_eq!(state.voice, None);
        assert!(!state.is_voice_active());
        assert!(!state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 0);
    }

    #[test]
    fn test_candidate_box_state_expanded() {
        let mut state = CandidateBoxState::default();
        state.set_expanded(true, 3);
        assert!(state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 3);

        state.set_expanded(false, 0);
        assert!(!state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 0);
    }
}
