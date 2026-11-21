//! 简拼缩写、单声母首音节补全与模糊音高频整词兜底收集器
//!
//! # 设计原理
//! - **实现初衷**：将兜底补全策略与常规整词/整句主收集路径解耦，保持 `collector.rs` 精简清晰。
//! - **核心优势**：集中管理简拼、首音节单字补全及未开模糊音时的高频模糊整词探查逻辑。
//! - **代价与局限**：模糊音兜底仅在未显式开启模糊音且精确整词词频偏低时触发，避免正常输入额外开销。

use crate::decoder::{
    score_phrase_match, to_positive_score, DatPhraseMatcher, DecodeResult, LatticeOptions,
    MatcherOptions, PrismFsa, SyllableLattice,
};
use crate::engine::AlgoEngine;
use crate::pipeline::is_rare_ext_char;
use crate::types::{AlgoCandidate, CandidateTier, FuzzyConfig};
use waylan_core::config::TypoCorrectionRules;

/// 未开启模糊音时，模糊整词需领先当前精确最高分至少该阈值才触发兜底
const FUZZY_FALLBACK_LEAD: i64 = 6_000;
/// 模糊音兜底整词的最低静态词频门槛
const FUZZY_FALLBACK_MIN_FREQ: u32 = 500;

impl AlgoEngine {
    /// 纯简拼缩写与单声母/首音节单字兜底补全
    pub(super) fn collect_acronym_and_single_char_fallbacks(
        &self,
        raw_input: &str,
        lattice: &SyllableLattice,
        out: &mut Vec<AlgoCandidate>,
    ) {
        // 1. 纯字母短串简拼检索（如 zg -> 中国, rmb -> 人民币）
        if self.config.enable_acronym
            && raw_input.len() >= 2
            && raw_input.len() <= 8
            && raw_input.bytes().all(|b| b.is_ascii_lowercase())
        {
            for mut cand in self.resources.table.lookup_acronym_top_k(raw_input, 12) {
                if self.config.filter_rare_ext_chars && is_rare_ext_char(&cand.text) {
                    continue;
                }
                cand.tier = CandidateTier::Acronym;
                cand.consumed_len = Some(raw_input.len());
                out.push(cand);
            }
        }

        // 2. 单声母高频单字或首个精确音节的补充单字（供翻页选字与前向截断提交）
        if raw_input.len() == 1 {
            self.push_initial_raw_candidates(raw_input, 1, 24, out);
            return;
        }
        if raw_input.is_empty() {
            return;
        }

        let start = lattice.skip_separators(0);
        if start >= lattice.len {
            return;
        }
        let first_matches = PrismFsa::global().match_syllables(&lattice.raw[start..], 0);
        if let Some((first_syl, rel_end)) = first_matches.last().copied() {
            let consumed = start + rel_end;
            for mut cand in self.resources.table.lookup_top_k(first_syl, 12) {
                if self.config.filter_rare_ext_chars && is_rare_ext_char(&cand.text) {
                    continue;
                }
                cand.tier = CandidateTier::PrefixPartial;
                cand.consumed_len = Some(consumed);
                out.push(cand);
            }
        } else {
            let initial = &lattice.raw[start..start + 1];
            self.push_initial_raw_candidates(initial, start + 1, 6, out);
        }
    }

    /// 从单声母索引表拉取前 `limit` 条单字候选并追加到输出列表
    fn push_initial_raw_candidates(
        &self,
        initial: &str,
        consumed_len: usize,
        limit: usize,
        out: &mut Vec<AlgoCandidate>,
    ) {
        let Some(entries) = self.resources.table.get_initial_raw_entries(initial) else {
            return;
        };
        for (text, score, word_id) in entries.into_iter().take(limit) {
            if self.config.filter_rare_ext_chars && is_rare_ext_char(&text) {
                continue;
            }
            out.push(AlgoCandidate {
                text,
                pinyin: initial.to_string(),
                score,
                comment: None,
                tier: CandidateTier::PrefixPartial,
                is_expandable: false,
                consumed_len: Some(consumed_len),
                sub_candidates: Vec::new(),
                word_id,
                typo_count: 0,
            });
        }
    }

    /// 未开启模糊音时，对精确解码偏弱的输入探查常用模糊音高频整词兜底
    pub(super) fn collect_fuzzy_fallback_candidates(
        &self,
        raw_input: &str,
        decode: &DecodeResult,
        out: &mut Vec<AlgoCandidate>,
    ) {
        if self.config.fuzzy.has_active_rules() || !should_probe_fuzzy_fallback(raw_input, decode) {
            return;
        }

        let fuzzy_opts = LatticeOptions {
            fuzzy: FuzzyConfig::recommended(),
            typo_rules: TypoCorrectionRules::default(),
            enable_mixed_jianpin: false,
            enable_typo_edges: false,
        };
        let fuzzy_lattice = SyllableLattice::build(raw_input, &fuzzy_opts);
        let matcher_opts = MatcherOptions {
            max_syllables: 6,
            max_typos: 0,
            top_k_per_key: 8,
            max_initial_steps: 0,
            filter_rare_ext_chars: self.config.filter_rare_ext_chars,
        };

        let mut matches =
            DatPhraseMatcher::match_from(&fuzzy_lattice, 0, &self.resources.table, &matcher_opts);
        matches.retain(|m| {
            m.end == raw_input.len()
                && m.syllable_count >= 2
                && !m.is_exact_all
                && !m.has_partial_tail
                && m.freq >= FUZZY_FALLBACK_MIN_FREQ
        });
        if matches.is_empty() {
            return;
        }

        let best_exact_score = decode
            .whole_words
            .iter()
            .filter(|m| m.typo_count == 0 && !m.has_initial)
            .map(|m| score_phrase_match(m, &self.config.scoring_weights))
            .max()
            .or_else(|| {
                decode
                    .sentences
                    .iter()
                    .filter(|s| s.typo_count == 0 && !s.has_initial && s.word_count == 1)
                    .map(|s| s.score)
                    .max()
            });

        matches.sort_by_key(|m| {
            std::cmp::Reverse(score_phrase_match(m, &self.config.scoring_weights))
        });

        let mut added = 0usize;
        for m in matches {
            let m_score = score_phrase_match(&m, &self.config.scoring_weights);
            if let Some(exact_score) = best_exact_score {
                if m_score < exact_score.saturating_add(FUZZY_FALLBACK_LEAD) {
                    continue;
                }
            }
            if out.iter().any(|c| c.text == m.word) {
                continue;
            }
            out.push(AlgoCandidate {
                text: m.word,
                pinyin: m.matched_key.to_string(),
                score: to_positive_score(m_score),
                comment: Some(format!("[模糊: {}]", m.matched_key)),
                tier: CandidateTier::RuleNormalization,
                is_expandable: false,
                consumed_len: Some(raw_input.len()),
                sub_candidates: Vec::new(),
                word_id: m.word_id,
                typo_count: 0,
            });
            added += 1;
            if added >= 2 {
                break;
            }
        }
    }
}

/// 判断精确解码结果是否偏弱，值得触发模糊音高频整词兜底
#[inline]
fn should_probe_fuzzy_fallback(raw_input: &str, decode: &DecodeResult) -> bool {
    if raw_input.len() < 4 || !raw_input.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    let max_exact_whole_freq = decode
        .whole_words
        .iter()
        .filter(|m| m.typo_count == 0 && !m.has_initial)
        .map(|m| m.freq)
        .max();

    if let Some(freq) = max_exact_whole_freq {
        // 精确整词本身不够高频时，才探查是否有压倒性的模糊高频整词（如 dansi -> 但是）
        return freq < 50_000;
    }

    match decode.sentences.first() {
        None => true,
        Some(best) => {
            best.word_count >= 2
                || best.typo_count > 0
                || best.has_initial
                || best.has_partial_tail
                || !best.is_exact_all
        }
    }
}
