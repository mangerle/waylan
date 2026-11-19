//! 词组匹配结果类型、排序截断与延迟物化模块

use std::sync::Arc;

use super::beam_scoring::{compute_raw_match_score, RawScoreParams};
use super::weights::ScoringWeights;
use crate::table::CompactMmapLexicon;

/// 词组匹配结果边（供束搜索解码器消费）
#[derive(Debug, Clone)]
pub struct PhraseMatch {
    /// 原始输入中的起始字节偏移
    pub start: usize,
    /// 原始输入中的结束字节偏移
    pub end: usize,
    /// 命中的中文词条文本
    pub word: String,
    /// 词条在紧凑词典中的唯一 ID（用于 O(1) Bigram 查表）
    pub word_id: Option<u32>,
    /// 词条词频（经惯用音/长度校准前或后的词频得分）
    pub freq: u32,
    /// 命中的规范拼音键（如 `"wan'quan'ke'yi"`），同键多词条共享同一底层切片
    pub matched_key: Arc<str>,
    /// 覆盖音节数
    pub syllable_count: usize,
    /// 消耗的纠错次数
    pub typo_count: u8,
    /// 累计图边惩罚代价
    pub edge_cost: u32,
    /// 是否包含混简拼声母扩展
    pub has_initial: bool,
    /// 是否包含末尾半成品前缀扩展
    pub has_partial_tail: bool,
    /// 是否整条词边完全由精确全拼构成
    pub is_exact_all: bool,
}

/// 词组匹配配置选项
#[derive(Debug, Clone, Copy)]
pub struct MatcherOptions {
    /// 单个词条最多跨越的音节数（默认 6）
    pub max_syllables: usize,
    /// 单个词条最多允许消耗的纠错次数（默认 2）
    pub max_typos: u8,
    /// 每个命中的拼音键最多拉取的同音候选词条数（默认 12）
    pub top_k_per_key: usize,
    /// 单条词路径最多允许包含的混简拼声母步数（默认 4）
    pub max_initial_steps: usize,
    /// 是否过滤 CJK Ext-B 及以上生僻单字
    pub filter_rare_ext_chars: bool,
}

impl Default for MatcherOptions {
    fn default() -> Self {
        Self {
            max_syllables: 6,
            max_typos: 2,
            top_k_per_key: 12,
            max_initial_steps: 4,
            filter_rare_ext_chars: true,
        }
    }
}

/// 延迟物化的轻量词组匹配中间条目（不含 `word: String`，避免被剪枝候选产生堆分配）
pub(super) struct RawPhraseMatch {
    pub(super) end: usize,
    pub(super) word_id: u32,
    pub(super) char_len: u8,
    pub(super) freq: u32,
    pub(super) matched_key: Arc<str>,
    pub(super) syllable_count: usize,
    pub(super) typo_count: u8,
    pub(super) edge_cost: u32,
    pub(super) has_initial: bool,
    pub(super) has_partial_tail: bool,
    pub(super) is_exact_all: bool,
}

#[inline]
pub(super) fn score_raw_match(m: &RawPhraseMatch, weights: &ScoringWeights) -> i64 {
    compute_raw_match_score(
        RawScoreParams {
            freq: m.freq,
            char_len: m.char_len as usize,
            edge_cost: m.edge_cost,
            typo_count: m.typo_count,
            has_initial: m.has_initial,
            has_partial_tail: m.has_partial_tail,
        },
        weights,
    )
}

/// 对轻量原始匹配结果执行生僻字过滤、排序打分与 Top-48 截断
pub(super) fn filter_and_truncate_raw_matches(
    raw: &mut Vec<RawPhraseMatch>,
    start_pos: usize,
    target_len: usize,
    weights: &ScoringWeights,
    keep_start_extras: bool,
) {
    // 常规输入（<= 63 字节）使用栈上 u64 位图完成 O(1) 零堆分配判定；超长输入回退布尔表
    let mut common_mask: u64 = 0;
    let mut common_overflow: Vec<bool> = Vec::new();
    let use_mask = target_len < 64;
    if !use_mask {
        common_overflow.resize(target_len + 1, false);
    }

    for o in raw.iter() {
        if o.freq >= 10 {
            if use_mask {
                common_mask |= 1u64 << o.end;
            } else if let Some(slot) = common_overflow.get_mut(o.end) {
                *slot = true;
            }
        }
    }

    raw.retain(|m| {
        let is_rare_single = m.syllable_count == 1
            && m.freq < 10
            && m.char_len == 1
            && (start_pos > 0 || m.end < target_len);
        if !is_rare_single {
            return true;
        }
        let has_common = if use_mask {
            (common_mask & (1u64 << m.end)) != 0
        } else {
            common_overflow.get(m.end).copied().unwrap_or(false)
        };
        !has_common
    });

    raw.sort_by_key(|m| std::cmp::Reverse(score_raw_match(m, weights)));
    if keep_start_extras {
        let mut kept_top = 0usize;
        raw.retain(|m| {
            let is_start_extra = m.end == target_len
                || (m.end < target_len
                    && m.syllable_count >= 2
                    && m.typo_count == 0
                    && !m.has_initial);
            if kept_top < 48 {
                kept_top += 1;
                true
            } else {
                is_start_extra
            }
        });
    } else {
        raw.truncate(48);
    }
}

/// 将截断后的轻量索引匹配条目物化为完整 `PhraseMatch`
pub(super) fn materialize_raw_matches(
    mmap: &CompactMmapLexicon,
    start_pos: usize,
    raw: Vec<RawPhraseMatch>,
) -> Vec<PhraseMatch> {
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        if let Some(word) = mmap.get_word(r.word_id) {
            out.push(PhraseMatch {
                start: start_pos,
                end: r.end,
                word: word.to_string(),
                word_id: Some(r.word_id),
                freq: r.freq,
                matched_key: r.matched_key,
                syllable_count: r.syllable_count,
                typo_count: r.typo_count,
                edge_cost: r.edge_cost,
                has_initial: r.has_initial,
                has_partial_tail: r.has_partial_tail,
                is_exact_all: r.is_exact_all,
            });
        }
    }
    out
}
