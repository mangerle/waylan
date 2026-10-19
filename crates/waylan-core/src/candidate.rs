//! 候选词、候选框展示状态与输入法动作领域模型
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、UI 渲染引擎、语音转录与 Wayland/IBus 协议前端提供统一的候选词与动作契约。
//! - **核心优势**：严格遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   通过强类型枚举彻底排除未展开却存在选中索引等非法状态组合。
//! - **代价与局限**：作为跨 Crate 流转的核心数据契约，字段改动需评估向下兼容性。

use serde::{Deserialize, Serialize};

/// 候选词优先级层级（数值越大优先级越高）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum CandidateTier {
    /// 部分前缀匹配或残余拼音
    PrefixPartial = 1,
    /// 次位邻键容错纠错候选
    TypoCorrection = 2,
    /// 首字母缩写简拼（如 bj 匹配 北京，yyds 匹配 永远的神）
    Acronym = 3,
    /// 整句预测（Viterbi 多音节长句拼接）
    SentenceViterbi = 4,
    /// 用户自适应学习词或动态自造词
    UserHistory = 5,
    /// 完整整词匹配（词库中完整匹配的成词，如“我们”、“中国”、“西安”、“先”）
    #[default]
    ExactWord = 6,
    /// 专有名词与受保护实体（如技术专名、中英实体，享有最高置顶优先级）
    ProtectedEntity = 7,
}

/// 候选词信息（统一领域候选实体）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// 候选文字（如：“你好”）
    pub text: String,
    /// 匹配的拼音序列（如：“ni'hao”）
    pub pinyin: String,
    /// 综合打分（静态词频 + 动态习惯加权）
    pub score: u32,
    /// 是否为可展开的二级动态功能项
    pub is_expandable: bool,
    /// 二级子候选列表（如多种展开的日期或时间格式）
    pub sub_candidates: Vec<String>,
    /// 候选附加注释信息（如多音字、反查或注音提示）
    #[serde(default)]
    pub comment: Option<String>,
    /// 候选词优先级层级
    #[serde(default)]
    pub tier: CandidateTier,
    /// 选词时显式消费的原始输入按键字节长度（若为 None 则回退至按 pinyin 自动推断）
    #[serde(default)]
    pub consumed_len: Option<usize>,
    /// 词条在静态词库中的全局整数 ID（若来源于系统词典，可用于直接加速二元转移打分）
    #[serde(default)]
    pub word_id: Option<u32>,
}

impl Candidate {
    /// 创建基础候选词（默认 ExactWord 级别）
    pub fn new(text: impl Into<String>, pinyin: impl Into<String>, score: u32) -> Self {
        Self {
            text: text.into(),
            pinyin: pinyin.into(),
            score,
            is_expandable: false,
            sub_candidates: Vec::new(),
            comment: None,
            tier: CandidateTier::ExactWord,
            consumed_len: None,
            word_id: None,
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

    /// 指定优先级层级
    pub fn with_tier(mut self, tier: CandidateTier) -> Self {
        self.tier = tier;
        self
    }

    /// 设置显式按键消费长度
    pub fn with_consumed_len(mut self, len: usize) -> Self {
        self.consumed_len = Some(len);
        self
    }

    /// 设置词库全局 ID
    pub fn with_word_id(mut self, word_id: Option<u32>) -> Self {
        self.word_id = word_id;
        self
    }

    /// 恒等转换为自身（平滑兼容旧 API 消除重分配开销）
    #[inline]
    pub fn to_core_candidate(&self) -> Self {
        self.clone()
    }

    /// 消费所有权恒等转换（平滑兼容旧 API 消除重分配开销）
    #[inline]
    pub fn into_core_candidate(self) -> Self {
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

/// 候选框界面顶级互斥呈现形态
///
/// 遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”的设计原则，
/// 将隐藏、语音胶囊、网格展开、二级卡片展开与普通紧凑条收敛为单层严格互斥枚举。
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

impl CandidateBoxState {
    /// 计算并提取当前唯一确定的强类型代数视图
    pub fn view(&self) -> CandidateBoxView<'_> {
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

    #[test]
    fn test_candidate_view_mode_mutually_exclusive() {
        let mut state = CandidateBoxState::default();
        assert_eq!(state.view_mode(), CandidateViewMode::Hidden);

        // 包含普通候选时为 Compact
        state.candidates.push(Candidate::new("测试", "ce'shi", 100));
        assert_eq!(state.view_mode(), CandidateViewMode::Compact);

        // 展开二级卡片时为 ExpandedCard
        state.set_expanded(true, 0);
        assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);

        // 网格展开时优先为 GridExpanded
        state.set_grid_state(GridState::Expanded { selected_index: 0 });
        assert_eq!(state.view_mode(), CandidateViewMode::GridExpanded);

        // 激活语音时最高优先级为 VoiceCapsule
        state.voice = Some(VoiceBoxState {
            is_listening: true,
            ..Default::default()
        });
        assert_eq!(state.view_mode(), CandidateViewMode::VoiceCapsule);
    }

    #[test]
    fn test_candidate_box_state_mutual_exclusion() {
        let mut state = CandidateBoxState::default();
        state.candidates.push(Candidate::new("测试", "ce'shi", 100));

        // 展开二级卡片
        state.set_expanded(true, 1);
        assert!(state.is_expanded());
        assert!(!state.is_grid_expanded());
        assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);

        // 设置大网格展开，二级卡片必须被物理自动折叠
        state.set_grid_state(GridState::Expanded { selected_index: 2 });
        assert!(state.is_grid_expanded());
        assert!(!state.is_expanded());
        assert_eq!(state.view_mode(), CandidateViewMode::GridExpanded);

        // 重新展开二级卡片，大网格必须被物理自动折叠
        state.set_expanded(true, 0);
        assert!(state.is_expanded());
        assert!(!state.is_grid_expanded());
        assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);
    }
}
