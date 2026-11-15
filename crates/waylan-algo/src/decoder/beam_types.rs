//! 束搜索解码器数据契约与状态节点定义

use super::matcher::{MatcherOptions, PhraseMatch};
use super::weights::ScoringWeights;

/// 句首词段候选（用于长句第 2 候选位上屏前半截并保留剩余拼音继续编辑）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceHeadSegment {
    /// 句首词段中文文本
    pub text: String,
    /// 句首词段对应的格式化拼音键
    pub pinyin: String,
    /// 该词段覆盖原始输入的精确 UTF-8 字节长度
    pub consumed_len: usize,
    /// 解码综合得分
    pub score: i64,
}

/// 束搜索解码产出的整句候选
#[derive(Debug, Clone)]
pub struct DecodedSentence {
    /// 整句中文文本
    pub text: String,
    /// 整句格式化拼音键（音节间以 `'` 连接）
    pub pinyin: String,
    /// 路径总对数得分
    pub score: i64,
    /// 组成该句子的词数
    pub word_count: usize,
    /// 路径累计消耗的容错次数
    pub typo_count: u8,
    /// 是否包含混简拼展开
    pub has_initial: bool,
    /// 是否包含末尾半成品补全
    pub has_partial_tail: bool,
    /// 是否全路径均为精确全拼
    pub is_exact_all: bool,
    /// 首词文本
    pub first_word: String,
    /// 首词拼音键
    pub first_word_pinyin: String,
    /// 首词结束的原始输入字节长度
    pub first_word_end: usize,
}

/// 统一束搜索解码结果包
#[derive(Debug, Clone, Default)]
pub struct DecodeResult {
    /// 完整覆盖全输入的 Top-K 整句候选
    pub sentences: Vec<DecodedSentence>,
    /// 完整覆盖 `0..N` 的整词匹配（按综合词分降序）
    pub whole_words: Vec<PhraseMatch>,
    /// 提取的句首词段候选列表
    pub prefix_heads: Vec<SentenceHeadSegment>,
    /// 最优格式化预编辑拼音串
    pub best_preedit: String,
}

/// 束搜索解码参数对象
#[derive(Debug, Clone)]
pub struct BeamDecodeOptions<'a> {
    /// 每个字节位置保留的束宽（默认 24）
    pub beam_width: usize,
    /// 单条路径允许的最大容错次数（默认 2）
    pub max_typos: u8,
    /// 输出的最大整句候选数（默认 8）
    pub max_sentences: usize,
    /// DAT 词组匹配选项
    pub matcher_options: MatcherOptions,
    /// 评分权重
    pub weights: ScoringWeights,
    /// 当前评估时间戳（秒），用于词对时间衰减计算
    pub eval_ts: u64,
    /// 上一轮上屏历史词（为句首第一个词提供 Bigram 与用户词对上下文）
    pub last_commit_context: Option<&'a str>,
}

impl Default for BeamDecodeOptions<'_> {
    fn default() -> Self {
        Self {
            beam_width: 24,
            max_typos: 2,
            max_sentences: 8,
            matcher_options: MatcherOptions::default(),
            weights: ScoringWeights::default(),
            eval_ts: 0,
            last_commit_context: None,
        }
    }
}

/// 束搜索回溯节点（存放于连续 Arena 中，避免状态扩展时复制整条词序列）
#[derive(Debug, Clone, Copy)]
pub(super) struct ArenaNode {
    pub(super) parent: Option<usize>,
    pub(super) word_ref: Option<(usize, usize)>,
}

/// 束搜索单位置状态节点
#[derive(Debug, Clone, Copy)]
pub(super) struct BeamState {
    pub(super) node_idx: usize,
    pub(super) score: i64,
    pub(super) typo_count: u8,
    pub(super) has_initial: bool,
    pub(super) has_partial_tail: bool,
    pub(super) is_exact_all: bool,
    pub(super) word_count: usize,
    pub(super) last_word_ref: Option<(usize, usize)>,
    pub(super) last_word_id: Option<u32>,
    pub(super) first_word_end: Option<usize>,
}
