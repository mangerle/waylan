//! 束搜索解码评分、边转移惩罚与束槽剪枝模块

use std::collections::HashSet;

use super::beam_types::BeamState;
use super::matcher::PhraseMatch;
use super::weights::ScoringWeights;

/// 每次容错的固定基础惩罚分
const TYPO_BASE_PENALTY: i64 = 4_500;
/// 混简拼路径的固定基础惩罚分
const INITIAL_MIX_PENALTY: i64 = 12_000;
/// 末尾半成品补全的固定基础惩罚分
const PARTIAL_TAIL_BASE_PENALTY: i64 = 2_500;

/// 候选保留判定参数对象
pub(super) struct RetainMatchCheck {
    pub(super) freq: u32,
    pub(super) syllable_count: usize,
    pub(super) char_len: usize,
    pub(super) is_exact_whole_query: bool,
    pub(super) has_common_char_at_end: bool,
    pub(super) has_common_phrase_at_end: bool,
}

/// 统一的生僻单字与低频多字词过滤判定（供全量与轻量匹配两条路径复用）
#[inline]
pub(super) fn should_retain_match(check: RetainMatchCheck) -> bool {
    if check.is_exact_whole_query {
        return true;
    }
    if check.syllable_count == 1 && check.freq < 10 && check.char_len == 1 {
        return !check.has_common_char_at_end;
    }
    if check.syllable_count >= 2 && check.freq < 30 {
        if check.freq <= 5 {
            return false;
        }
        return !check.has_common_phrase_at_end;
    }
    true
}

/// 过滤低频生僻单字并按局部得分截取当前位置 Top-48 词匹配
pub(super) fn filter_and_rank_matches(
    matches: &mut Vec<PhraseMatch>,
    target_len: usize,
    weights: &ScoringWeights,
) {
    let use_mask = target_len < 64;
    let mut common_mask: u64 = 0;
    let mut phrase_common_mask: u64 = 0;
    let mut common_overflow: Vec<bool> = Vec::new();
    let mut phrase_common_overflow: Vec<bool> = Vec::new();
    if !use_mask {
        common_overflow.resize(target_len + 1, false);
        phrase_common_overflow.resize(target_len + 1, false);
    }

    for o in matches.iter() {
        if o.freq >= 10 {
            if use_mask {
                common_mask |= 1u64 << o.end;
            } else if let Some(slot) = common_overflow.get_mut(o.end) {
                *slot = true;
            }
        }
        if o.freq >= 30 {
            if use_mask {
                phrase_common_mask |= 1u64 << o.end;
            } else if let Some(slot) = phrase_common_overflow.get_mut(o.end) {
                *slot = true;
            }
        }
    }

    matches.retain(|m| {
        let has_common = if use_mask {
            (common_mask & (1u64 << m.end)) != 0
        } else {
            common_overflow.get(m.end).copied().unwrap_or(false)
        };
        let has_common_phrase = if use_mask {
            (phrase_common_mask & (1u64 << m.end)) != 0
        } else {
            phrase_common_overflow.get(m.end).copied().unwrap_or(false)
        };
        should_retain_match(RetainMatchCheck {
            freq: m.freq,
            syllable_count: m.syllable_count,
            char_len: m.word.chars().count(),
            is_exact_whole_query: m.start == 0 && m.end == target_len,
            has_common_char_at_end: has_common,
            has_common_phrase_at_end: has_common_phrase,
        })
    });
    matches.sort_by_key(|m| std::cmp::Reverse(score_phrase_match(m, weights)));
    matches.truncate(48);
}

/// 按 `(末词, 容错次数)` 原地去重并按 `beam_width` 截断束槽（复用原有 Vec 容量，消除新 Vec 分配）
pub(super) fn prune_beam_slot(
    slot: &mut Vec<BeamState>,
    beam_width: usize,
    spans_by_pos: &[Vec<PhraseMatch>],
    seed_word: &str,
) {
    if slot.len() <= 1 {
        return;
    }
    slot.sort_by_key(|a| std::cmp::Reverse(a.score));

    let mut seen: HashSet<(&str, u8)> = HashSet::with_capacity(slot.len().min(64));
    let mut write_idx = 0usize;
    for read_idx in 0..slot.len() {
        let state = &slot[read_idx];
        let last_word = match state.last_word_ref {
            Some((p, idx)) => spans_by_pos[p][idx].word.as_str(),
            None => seed_word,
        };
        if !seen.insert((last_word, state.typo_count)) {
            continue;
        }
        if write_idx != read_idx {
            slot.swap(write_idx, read_idx);
        }
        write_idx += 1;
        if write_idx >= beam_width {
            break;
        }
    }
    slot.truncate(write_idx);
}

/// 原始轻量词匹配打分参数（用于延迟物化前排序截断）
#[derive(Debug, Clone, Copy)]
pub(super) struct RawScoreParams {
    pub freq: u32,
    pub char_len: usize,
    pub edge_cost: u32,
    pub typo_count: u8,
    pub has_initial: bool,
    pub has_partial_tail: bool,
}

/// 根据边代价与路径标记统一计算惩罚分（消除原始打分与束边打分重复）
#[inline]
fn compute_penalty_from_parts(
    edge_cost: u32,
    typo_count: u8,
    has_initial: bool,
    has_partial_tail: bool,
    weights: &ScoringWeights,
) -> i64 {
    let mut penalty = (edge_cost as i64).saturating_mul(weights.typo_cost_penalty as i64);
    if typo_count > 0 {
        penalty = penalty.saturating_add((typo_count as i64) * TYPO_BASE_PENALTY);
    }
    if has_initial {
        penalty = penalty.saturating_add(INITIAL_MIX_PENALTY);
    }
    if has_partial_tail {
        penalty = penalty.saturating_add(
            (weights.partial_extension_penalty as i64) * 100 + PARTIAL_TAIL_BASE_PENALTY,
        );
    }
    penalty
}

/// 在延迟物化前基于纯数值元数据计算词组匹配排序分
#[inline]
pub(super) fn compute_raw_match_score(params: RawScoreParams, weights: &ScoringWeights) -> i64 {
    let length_bonus = (params.char_len as i64 - 1).saturating_mul(weights.length_bonus_per_char);
    let penalty = compute_penalty_from_parts(
        params.edge_cost,
        params.typo_count,
        params.has_initial,
        params.has_partial_tail,
        weights,
    );
    unigram_log_score(params.freq) + length_bonus - penalty
}

/// 计算单条词边的基础转移代价（纠错/混简拼/半成品惩罚）
#[inline]
pub(super) fn compute_edge_penalty(pm: &PhraseMatch, weights: &ScoringWeights) -> i64 {
    compute_penalty_from_parts(
        pm.edge_cost,
        pm.typo_count,
        pm.has_initial,
        pm.has_partial_tail,
        weights,
    )
}

/// 评估单条整词匹配的综合排序分（用于单位置候选排序与整词排序）
#[inline]
pub fn score_phrase_match(pm: &PhraseMatch, weights: &ScoringWeights) -> i64 {
    let char_len = pm.word.chars().count() as i64;
    let length_bonus = (char_len - 1).saturating_mul(weights.length_bonus_per_char);
    unigram_log_score(pm.freq) + length_bonus - compute_edge_penalty(pm, weights)
}

/// 将原始静态词频转换为与二元转移矩阵同量纲的对数似然得分
#[inline]
pub fn unigram_log_score(raw_score: u32) -> i64 {
    ((raw_score as f64 + 1.0).ln() * 1_200.0) as i64
}

/// 将束搜索路径累计对数似然安全映射为正整数候选分
#[inline]
pub fn to_positive_score(total_score: i64) -> u32 {
    if total_score <= 0 {
        1
    } else {
        (total_score.min(u32::MAX as i64 - 1_000_000) as u32).saturating_add(100)
    }
}
