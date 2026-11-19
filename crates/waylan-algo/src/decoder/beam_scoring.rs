//! 束搜索解码评分、边转移惩罚与束槽剪枝模块

use std::collections::HashMap;

use super::beam_types::BeamState;
use super::matcher::PhraseMatch;
use super::weights::ScoringWeights;

/// 每次容错的固定基础惩罚分
const TYPO_BASE_PENALTY: i64 = 4_500;
/// 混简拼路径的固定基础惩罚分
const INITIAL_MIX_PENALTY: i64 = 12_000;
/// 末尾半成品补全的固定基础惩罚分
const PARTIAL_TAIL_BASE_PENALTY: i64 = 2_500;

/// 过滤低频生僻单字并按局部得分截取当前位置 Top-48 词匹配
pub(super) fn filter_and_rank_matches(
    matches: &mut Vec<PhraseMatch>,
    target_len: usize,
    weights: &ScoringWeights,
) {
    let has_common_at_end: Vec<usize> = matches
        .iter()
        .filter(|o| o.freq >= 10)
        .map(|o| o.end)
        .collect();
    matches.retain(|m| {
        let is_rare_single = m.syllable_count == 1
            && m.freq < 10
            && m.word.chars().count() == 1
            && (m.start > 0 || m.end < target_len);
        if !is_rare_single {
            return true;
        }
        // 同跨度存在常用单字（freq >= 10）或词组时，隔离低频生僻单字，避免其参与组句挤占束宽
        !has_common_at_end.contains(&m.end)
    });
    matches.sort_by_key(|m| std::cmp::Reverse(score_phrase_match(m, weights)));
    matches.truncate(48);
}

/// 按 `(末词, 容错次数)` 去重并按 `beam_width` 截断束槽
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

    let mut best_by_key: HashMap<(&str, u8), usize> = HashMap::with_capacity(slot.len().min(64));
    let mut kept = Vec::with_capacity(beam_width.min(slot.len()));
    for state in slot.drain(..) {
        let last_word = match state.last_word_ref {
            Some((p, idx)) => spans_by_pos[p][idx].word.as_str(),
            None => seed_word,
        };
        let key = (last_word, state.typo_count);
        if best_by_key.contains_key(&key) {
            continue;
        }
        best_by_key.insert(key, kept.len());
        kept.push(state);
        if kept.len() >= beam_width {
            break;
        }
    }
    *slot = kept;
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

/// 在延迟物化前基于纯数值元数据计算词组匹配排序分
#[inline]
pub(super) fn compute_raw_match_score(params: RawScoreParams, weights: &ScoringWeights) -> i64 {
    let length_bonus = (params.char_len as i64 - 1).saturating_mul(weights.length_bonus_per_char);
    let mut penalty = (params.edge_cost as i64).saturating_mul(weights.typo_cost_penalty as i64);
    if params.typo_count > 0 {
        penalty = penalty.saturating_add((params.typo_count as i64) * TYPO_BASE_PENALTY);
    }
    if params.has_initial {
        penalty = penalty.saturating_add(INITIAL_MIX_PENALTY);
    }
    if params.has_partial_tail {
        penalty = penalty.saturating_add(
            (weights.partial_extension_penalty as i64) * 100 + PARTIAL_TAIL_BASE_PENALTY,
        );
    }
    unigram_log_score(params.freq) + length_bonus - penalty
}

/// 计算单条词边的基础转移代价（纠错/混简拼/半成品惩罚）
#[inline]
pub(super) fn compute_edge_penalty(pm: &PhraseMatch, weights: &ScoringWeights) -> i64 {
    let mut penalty = (pm.edge_cost as i64).saturating_mul(weights.typo_cost_penalty as i64);
    if pm.typo_count > 0 {
        penalty = penalty.saturating_add((pm.typo_count as i64) * TYPO_BASE_PENALTY);
    }
    if pm.has_initial {
        penalty = penalty.saturating_add(INITIAL_MIX_PENALTY);
    }
    if pm.has_partial_tail {
        penalty = penalty.saturating_add(
            (weights.partial_extension_penalty as i64) * 100 + PARTIAL_TAIL_BASE_PENALTY,
        );
    }
    penalty
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
