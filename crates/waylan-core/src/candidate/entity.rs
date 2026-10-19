//! 候选词与优先级层级领域实体
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、UI 渲染引擎、语音转录与 Wayland/IBus 协议前端提供统一的候选词领域契约。
//! - **核心优势**：包含匹配拼音、综合分值、二级展开子项与全局 Word ID，支持高效打分与分级排序。

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
}
