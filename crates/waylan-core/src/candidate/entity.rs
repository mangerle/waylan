//! 候选词与优先级层级领域实体
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、UI 渲染引擎、语音转录与 Wayland/IBus 协议前端提供统一的候选词领域契约。
//! - **核心优势**：包含匹配拼音、综合分值、二级展开子项与全局 Word ID，支持高效打分与分级排序。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// 候选词注释类别
///
/// # 设计原理
/// - **实现初衷**：消除 UI 渲染与跨层传递中对字符串前缀（如 `starts_with('(')`）的黑盒猜测，
///   以强类型明确注释的业务意图。
/// - **核心优势**：严格区分“用户配置控制的标准注音”与“始终需要告知用户的特异性提示”。
/// - **代价与局限**：轻量 `Copy` 枚举，仅供展示判断，不破坏序列化协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentKind {
    /// 读音偏离键入拼音的特异性注音（如键入 cang 命中“常”，标注 `(chang)`）
    /// 属于重要读音纠偏提示，始终常驻展示。
    PhoneticDifference,
    /// 物理击键纠错提示（如 `(ni'hao)` 或兼容旧版 `[纠错: ni'hao]`）
    /// 属于重要输入提示，始终常驻展示。
    TypoCorrection,
    /// 特殊功能或模式提示（如 `[混拼]`、`[习惯音]`、`当前时间`、`拆字` 等）
    /// 始终常驻展示。
    SpecialHint,
    /// 用户按需开启的标准拼音全拼注音（如 `[ni'hao]`）
    /// 受 `show_pinyin_comment` 开关控制。
    StandardPinyin,
}

/// 候选词优先级层级（数值越大优先级越高）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum CandidateTier {
    /// 部分前缀匹配或残余拼音
    PrefixPartial = 1,
    /// 次位邻键容错纠错候选（概率性物理误触猜测：QWERTY 邻键替换、相邻字母倒置）
    TypoCorrection = 2,
    /// 首字母缩写简拼（如 bj 匹配 北京，yyds 匹配 永远的神）
    Acronym = 3,
    /// 整句预测（Viterbi 多音节长句拼接）
    SentenceViterbi = 4,
    /// 拼音方案确定性归一候选（如 `niou`→`niu`、`guei`→`gui`、`zon`→`zong`）
    ///
    /// 分层原理：归一改写命中即为标准拼写，不存在 [`Self::TypoCorrection`] 的误触概率问题，
    /// 故置于概率猜测与整句解码之上——两音节生造整句（如 `guei` 拼出的“股诶”）不应压住
    /// 唯一合法读音（“贵”）；但它仍是对用户键入的二次解释，故低于用户自造词与原生成词。
    RuleNormalization = 5,
    /// 用户自适应学习词或动态自造词
    UserHistory = 6,
    /// 完整整词匹配（词库中完整匹配的成词，如“我们”、“中国”、“西安”、“先”）
    #[default]
    ExactWord = 7,
    /// 专有名词与受保护实体（如技术专名、中英实体，享有最高置顶优先级）
    ProtectedEntity = 8,
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
    /// 该候选沿解码路径消耗的纠错次数（0 表示未纠错；仅用于最终排序的竞争判定）
    #[serde(default)]
    pub typo_count: u8,
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
            typo_count: 0,
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

    /// 设置该候选解码路径消耗的纠错次数
    pub fn with_typo_count(mut self, typo_count: u8) -> Self {
        self.typo_count = typo_count;
        self
    }

    /// 获取当前注释的强类型类别（若无注释或为空则返回 `None`）
    #[inline]
    pub fn comment_kind(&self) -> Option<CommentKind> {
        let comment = self.comment.as_deref()?.trim();
        if comment.is_empty() {
            return None;
        }
        if comment.starts_with('(') && comment.ends_with(')') {
            if self.tier == CandidateTier::TypoCorrection || self.typo_count > 0 {
                Some(CommentKind::TypoCorrection)
            } else {
                Some(CommentKind::PhoneticDifference)
            }
        } else if comment.starts_with("[纠错") {
            Some(CommentKind::TypoCorrection)
        } else if comment.starts_with('[') && comment.ends_with(']') {
            let inner = &comment[1..comment.len() - 1];
            // 若中括号内部为纯 ASCII 拼音字母及隔音符，则为标准拼音注音；否则为中文特殊提示（如 [混拼]、[习惯音]）
            if inner
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || b == b'\'' || b.is_ascii_whitespace())
            {
                Some(CommentKind::StandardPinyin)
            } else {
                Some(CommentKind::SpecialHint)
            }
        } else {
            Some(CommentKind::SpecialHint)
        }
    }

    /// 根据全局标准拼音展示开关，判定当前注释是否应该在界面上展示
    #[inline]
    pub fn should_show_comment(&self, show_standard_pinyin: bool) -> bool {
        match self.comment_kind() {
            Some(CommentKind::StandardPinyin) => show_standard_pinyin,
            Some(
                CommentKind::PhoneticDifference
                | CommentKind::TypoCorrection
                | CommentKind::SpecialHint,
            ) => true,
            None => false,
        }
    }

    /// 根据全局标准拼音展示开关，获取用于格式化显示的注释切片
    #[inline]
    pub fn display_comment(&self, show_standard_pinyin: bool) -> Option<&str> {
        if self.should_show_comment(show_standard_pinyin) {
            self.comment.as_deref()
        } else {
            None
        }
    }

    /// 根据全局标准拼音展示开关，合成候选词与其注释文本（无注释或关闭展示时零堆分配借用原切片）
    #[inline]
    pub fn format_display_text<'a>(&'a self, show_standard_pinyin: bool) -> Cow<'a, str> {
        match self.display_comment(show_standard_pinyin) {
            Some(comment) if !comment.is_empty() => {
                Cow::Owned(format!("{} {}", self.text, comment))
            }
            _ => Cow::Borrowed(self.text.as_str()),
        }
    }
}
