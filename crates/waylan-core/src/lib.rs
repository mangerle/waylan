pub mod config;
pub mod config_types;
pub use config::{
    AppConfig, CandidateOrientation, FuzzyPinyinRules, PageKeys, SwitchKey, ThemeMode,
    TypoCorrectionRules,
};

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
}

impl Candidate {
    pub fn new(text: impl Into<String>, pinyin: impl Into<String>, score: u32) -> Self {
        Self {
            text: text.into(),
            pinyin: pinyin.into(),
            score,
            is_expandable: false,
            sub_candidates: Vec::new(),
        }
    }

    /// 标记为可展开并挂载二级子候选格式列表
    pub fn with_sub_candidates(mut self, sub_candidates: Vec<String>) -> Self {
        self.is_expandable = !sub_candidates.is_empty();
        self.sub_candidates = sub_candidates;
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
    /// 当前工作模式提示文字（如：“中”、“英”、“V”）
    pub mode_badge: String,
    /// 是否处于 5 行大候选网格面板展开状态
    pub is_grid_expanded: bool,
    /// 展开状态下的多行网格候选词列表（通常预加载前 30 个候选词）
    pub grid_candidates: Vec<Candidate>,
}

impl CandidateBoxState {
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
}

/// 输入法算法引擎通用抽象特型 (ImeEngine)
///
/// # 设计原理
/// - **实现初衷**：解除顶层 Wayland 协议层与 Daemon 服务层对具体拼音算法实现的硬编码强耦合。
///   实现该特型后，上层应用不仅可接入现有的双字前缀字典树/Viterbi 算法引擎，亦可无缝对接双拼、五笔或外部输入后端。
/// - **核心优势**：接口边界清晰内聚，状态流转纯粹，便于上层快速编写基于 Mock 引擎的协议与界面测试。
/// - **代价与局限**：高级特有功能（如特定字典热重载细节）需通过内部引用或辅助特型向下透传。
pub trait ImeEngine: Send {
    /// 向输入引擎推入一个 ASCII 字符
    fn push_char(&mut self, ch: char) -> bool;

    /// 从当前光标位置删除前一个字符（退格操作）
    fn pop_char(&mut self) -> bool;

    /// 获取当前未转换的原始字母输入串
    fn raw_input(&self) -> &str;

    /// 获取格式化后的预编辑字符串（含音节分词符）
    fn formatted_preedit(&self) -> String;

    /// 获取当前候选词列表
    fn candidates(&self) -> &[Candidate];

    /// 获取当前选中的高亮候选词索引
    fn selected_index(&self) -> usize;

    /// 确认选择指定序号的候选词并上屏
    ///
    /// 返回 `(上屏文本, 剩余预编辑文本长度)`
    fn select_candidate(&mut self, index: usize, timestamp: u64) -> Option<(String, usize)>;

    /// 翻至下一页候选词
    fn next_page(&mut self) -> bool;

    /// 翻至上一页候选词
    fn prev_page(&mut self) -> bool;

    /// 高亮移动到下一个候选词
    fn next_candidate(&mut self);

    /// 高亮移动到上一个候选词
    fn prev_candidate(&mut self);

    /// 重置清空当前输入状态
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_creation_and_expandable() {
        let cand = Candidate::new("你好", "ni'hao", 100);
        assert_eq!(cand.text, "你好");
        assert_eq!(cand.pinyin, "ni'hao");
        assert_eq!(cand.score, 100);
        assert!(!cand.is_expandable);
        assert!(cand.sub_candidates.is_empty());

        let cand_with_sub =
            cand.with_sub_candidates(vec!["你好世界".to_string(), "你好啊".to_string()]);
        assert!(cand_with_sub.is_expandable);
        assert_eq!(cand_with_sub.sub_candidates.len(), 2);
    }

    #[test]
    fn test_expanded_card_state_machine() {
        let collapsed = ExpandedCardState::default();
        assert_eq!(collapsed, ExpandedCardState::Collapsed);
        assert!(!collapsed.is_expanded());
        assert_eq!(collapsed.selected_index(), None);

        let expanded = ExpandedCardState::Expanded { selected_index: 2 };
        assert!(expanded.is_expanded());
        assert_eq!(expanded.selected_index(), Some(2));
    }

    #[test]
    fn test_candidate_box_state_methods() {
        let mut state = CandidateBoxState::default();
        assert!(!state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 0);

        state.set_expanded(true, 3);
        assert!(state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 3);

        state.set_expanded(false, 0);
        assert!(!state.is_expanded());
        assert_eq!(state.expanded_selected_index(), 0);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let cand = Candidate::new("微澜", "wei'lan", 9999);
        let serialized = serde_json::to_string(&cand).expect("序列化候选词成功");
        let deserialized: Candidate =
            serde_json::from_str(&serialized).expect("反序列化候选词成功");
        assert_eq!(cand, deserialized);
    }

    #[test]
    fn test_input_mode_default() {
        assert_eq!(InputMode::default(), InputMode::Chinese);
    }
}
