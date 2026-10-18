pub mod assets;
pub mod candidate_format;
pub mod config;
pub mod fs_watcher;
pub mod input;
pub mod loop_wakeup;
pub mod state;
pub mod text;
pub use candidate_format::{format_candidate_display, CIRCLED_NUMBERS};
pub use config::{
    AppConfig, AudioFrontEndConfig, CandidateIndexStyle, CandidateOrientation, CloudAsrConfig,
    CloudProfile, CloudProviderType, ConfigError, FullWidthCharShortcut, FullWidthPunctShortcut,
    FuzzyPinyinRules, NsEngineType, OfflineAsrConfig, OfflineModelType, PageKeys, ParsedShortcut,
    SwitchKey, ThemeMode, TraditionalShortcut, TrayDisplayMode, TypoCorrectionRules, VadConfig,
    VadModelType, VoiceConfig, VoiceEngineType, VoiceInteractionConfig, VoiceItnConfig,
    VoiceVisualEffect,
};
pub use fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
    compute_initial_signature, init_session_inotify_watcher, FileSignature, InotifyWatcher,
};
pub use input::*;
pub use loop_wakeup::LoopWakeup;
pub use state::{InputStateSnapshot, SharedInputFlags};
pub use text::{is_cjk_character, to_full_width_char, to_full_width_string};

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
    /// 是否处于 5 行大候选网格面板展开状态
    pub is_grid_expanded: bool,
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
