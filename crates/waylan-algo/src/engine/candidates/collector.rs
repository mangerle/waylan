//! 基于统一束搜索解码结果的候选词收集与提权器
//!
//! # 设计原理
//! - **实现初衷**：直接消费 `PositionalBeamDecoder` 产出的整词、整句与句首词段，
//!   统一完成专名保护、纯简拼缩写兜底、首音节单字补全与用户自适应学习提权。
//! - **核心优势**：彻底消除旧版多路冗余查表与孤立纠错猜测的冲突，所有变体共享同一份束搜索结果。
//! - **代价与局限**：整词单键默认拉取 Top-12，冷门同音单字由首音节单字兜底补全。

use crate::decoder::alias::is_habitual_alias_key;
use crate::decoder::{
    score_phrase_match, to_positive_score, BigramLanguageModel, DatPhraseMatcher, DecodeResult,
    LatticeOptions, MatcherOptions, PhraseMatch, PrismFsa, SyllableLattice,
};
use crate::engine::AlgoEngine;
use crate::pipeline::is_rare_ext_char;
use crate::table::extract_raw_score;
use crate::types::{AlgoCandidate, CandidateTier, FuzzyConfig};
use waylan_core::config::TypoCorrectionRules;

/// 未开启模糊音时，模糊整词需领先当前精确最高分至少该阈值才触发兜底
const FUZZY_FALLBACK_LEAD: i64 = 6_000;
/// 模糊音兜底整词的最低静态词频门槛
const FUZZY_FALLBACK_MIN_FREQ: u32 = 500;

/// 判断解码结果中是否存在无容错、无混简拼的干净全拼（含合法尾部半成品）锚点
#[inline]
fn has_clean_decode_anchor(decode: &DecodeResult) -> bool {
    decode
        .whole_words
        .iter()
        .any(|m| m.typo_count == 0 && !m.has_initial)
        || decode
            .sentences
            .iter()
            .any(|s| s.typo_count == 0 && !s.has_initial)
}

impl AlgoEngine {
    /// 从统一解码结果及用户词库收集全量候选词
    pub(crate) fn collect_all_raw_candidates(
        &self,
        lattice: &SyllableLattice,
        decode: &DecodeResult,
    ) -> Vec<AlgoCandidate> {
        let raw_input = self.composition.as_str();
        let mut all_candidates = Vec::with_capacity(64);

        // 1. 专名保护与受保护实体匹配
        if let Some(protected) = self.resources.entity_manager.match_exact(raw_input) {
            all_candidates.push(AlgoCandidate {
                text: protected,
                pinyin: raw_input.to_string(),
                score: u32::MAX / 2,
                comment: Some("专名".to_string()),
                tier: CandidateTier::ProtectedEntity,
                is_expandable: false,
                consumed_len: Some(raw_input.len()),
                sub_candidates: Vec::new(),
                word_id: None,
            });
        }

        // 2. 收集统一束搜索整词匹配
        self.collect_whole_word_candidates(raw_input, decode, &mut all_candidates);

        // 3. 收集统一束搜索整句与句首词段（确保长句第 2 候选位稳定出现句首词段）
        self.collect_sentence_and_head_candidates(raw_input, decode, &mut all_candidates);

        // 4. 用户自适应词典匹配
        self.collect_user_adaptive_candidates(raw_input, decode, &mut all_candidates);

        // 5. 纯简拼缩写与单声母/首音节单字兜底补全
        self.collect_acronym_and_single_char_fallbacks(raw_input, lattice, &mut all_candidates);

        // 5.5 未开启模糊音时，针对明显误拼的高频常用整词提供模糊音兜底（如 zidao -> 知道）
        self.collect_fuzzy_fallback_candidates(raw_input, decode, &mut all_candidates);

        // 6. 叠加用户自适应词频权重、词对权重与上一轮上屏上下文
        self.apply_user_and_context_boost(&mut all_candidates);

        all_candidates
    }

    /// 收集覆盖全输入的整词候选（精确整词、混简拼整词、半成品整词、容错整词）
    fn collect_whole_word_candidates(
        &self,
        raw_input: &str,
        decode: &DecodeResult,
        out: &mut Vec<AlgoCandidate>,
    ) {
        let has_exact_whole_or_sentence = decode.whole_words.iter().any(|m| m.is_exact_all)
            || decode.sentences.iter().any(|s| s.is_exact_all);
        let has_clean_anchor = has_clean_decode_anchor(decode);
        let has_positive_typo_word = decode
            .whole_words
            .iter()
            .any(|m| m.typo_count > 0 && score_phrase_match(m, &self.config.scoring_weights) > 0);

        for pm in &decode.whole_words {
            if pm.typo_count > 0 && has_clean_anchor {
                continue;
            }
            if let Some(cand) = self.phrase_match_to_candidate(
                pm,
                raw_input.len(),
                (has_exact_whole_or_sentence, has_positive_typo_word),
            ) {
                out.push(cand);
            }
        }
    }

    /// 将 `PhraseMatch` 转换为带精确语义层级的 `AlgoCandidate`
    fn phrase_match_to_candidate(
        &self,
        pm: &PhraseMatch,
        raw_len: usize,
        anchor_flags: (bool, bool),
    ) -> Option<AlgoCandidate> {
        let (has_exact_anchor, has_positive_typo_word) = anchor_flags;
        let mut weighted = score_phrase_match(pm, &self.config.scoring_weights);
        if let Some(prev) = self.composition.last_commit_context() {
            let bigram_bonus = self
                .resources
                .table
                .get_bigram_affinity(prev, &pm.word)
                .unwrap_or_else(|| BigramLanguageModel::estimate_bigram_affinity(prev, &pm.word));
            weighted = weighted.saturating_add(bigram_bonus);
        }
        let score = to_positive_score(weighted);

        let (tier, comment) = if pm.is_exact_all {
            (CandidateTier::ExactWord, None)
        } else if pm.typo_count > 0 {
            (
                CandidateTier::TypoCorrection,
                Some(format!("[纠错: {}]", pm.matched_key)),
            )
        } else if pm.has_initial {
            let tier = if pm.has_partial_tail && has_positive_typo_word && score <= 100 {
                CandidateTier::PrefixPartial
            } else {
                CandidateTier::SentenceViterbi
            };
            (tier, Some("[混拼]".to_string()))
        } else if pm.has_partial_tail {
            let tier = if has_exact_anchor {
                CandidateTier::PrefixPartial
            } else {
                CandidateTier::SentenceViterbi
            };
            (tier, None)
        } else if is_habitual_alias_key(&pm.matched_key) {
            (CandidateTier::ExactWord, Some("[习惯音]".to_string()))
        } else {
            (CandidateTier::RuleNormalization, None)
        };

        Some(AlgoCandidate {
            text: pm.word.clone(),
            pinyin: pm.matched_key.clone(),
            score,
            comment,
            tier,
            is_expandable: false,
            consumed_len: Some(raw_len),
            sub_candidates: Vec::new(),
            word_id: pm.word_id,
        })
    }

    /// 收集整句候选与句首词段
    fn collect_sentence_and_head_candidates(
        &self,
        raw_input: &str,
        decode: &DecodeResult,
        out: &mut Vec<AlgoCandidate>,
    ) {
        let has_clean_anchor = has_clean_decode_anchor(decode);
        let has_positive_typo_word = decode
            .whole_words
            .iter()
            .any(|m| m.typo_count > 0 && score_phrase_match(m, &self.config.scoring_weights) > 0);
        let mut top_sentence_score = 10_000u32;
        for (idx, sent) in decode.sentences.iter().enumerate() {
            if sent.typo_count > 0 && has_clean_anchor {
                continue;
            }
            let base_score = to_positive_score(sent.score);
            if idx == 0 {
                top_sentence_score = base_score;
            }
            let tier = if sent.typo_count > 0 && !sent.is_exact_all {
                CandidateTier::TypoCorrection
            } else if sent.has_initial
                && sent.has_partial_tail
                && has_positive_typo_word
                && base_score <= 100
            {
                CandidateTier::PrefixPartial
            } else {
                CandidateTier::SentenceViterbi
            };
            let comment = if sent.typo_count > 0 {
                Some(format!("[纠错: {}]", sent.pinyin))
            } else if sent.has_initial {
                Some("[混拼]".to_string())
            } else {
                None
            };
            out.push(AlgoCandidate {
                text: sent.text.clone(),
                pinyin: sent.pinyin.clone(),
                score: base_score,
                comment,
                tier,
                is_expandable: false,
                consumed_len: Some(raw_input.len()),
                sub_candidates: Vec::new(),
                word_id: None,
            });
        }

        let top_text = decode.sentences.first().map(|s| s.text.as_str());
        for (h_idx, head) in decode.prefix_heads.iter().enumerate() {
            if top_text == Some(head.text.as_str()) {
                continue;
            }
            let (tier, head_score) = if h_idx == 0 {
                let s = if decode.sentences.is_empty() {
                    to_positive_score(head.score)
                } else {
                    top_sentence_score.saturating_sub(50)
                };
                (CandidateTier::SentenceViterbi, s)
            } else {
                (CandidateTier::PrefixPartial, to_positive_score(head.score))
            };
            out.push(AlgoCandidate {
                text: head.text.clone(),
                pinyin: head.pinyin.clone(),
                score: head_score,
                comment: None,
                tier,
                is_expandable: false,
                consumed_len: Some(head.consumed_len),
                sub_candidates: Vec::new(),
                word_id: None,
            });
        }
    }

    /// 收集用户自适应自造词与历史高频词汇匹配
    fn collect_user_adaptive_candidates(
        &self,
        raw_input: &str,
        decode: &DecodeResult,
        out: &mut Vec<AlgoCandidate>,
    ) {
        let eval_ts = self.adaptive.last_timestamp;
        let preedit_key = if decode.best_preedit.is_empty() {
            raw_input
        } else {
            decode.best_preedit.as_str()
        };

        let mut user_cands = Vec::new();
        self.adaptive
            .model
            .lookup_user_words_into(preedit_key, eval_ts, &mut user_cands);
        for mut cand in user_cands {
            if self.config.filter_rare_ext_chars && is_rare_ext_char(&cand.text) {
                continue;
            }
            cand.tier = CandidateTier::UserHistory;
            cand.consumed_len = Some(raw_input.len());
            out.push(cand);
        }
    }

    /// 纯简拼缩写与单声母/首音节单字兜底补全
    fn collect_acronym_and_single_char_fallbacks(
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
            if let Some(entries) = self.resources.table.get_initial_raw_entries(raw_input) {
                for (text, score, word_id) in entries.into_iter().take(24) {
                    if self.config.filter_rare_ext_chars && is_rare_ext_char(&text) {
                        continue;
                    }
                    out.push(AlgoCandidate {
                        text,
                        pinyin: raw_input.to_string(),
                        score,
                        comment: None,
                        tier: CandidateTier::PrefixPartial,
                        is_expandable: false,
                        consumed_len: Some(1),
                        sub_candidates: Vec::new(),
                        word_id,
                    });
                }
            }
        } else if !raw_input.is_empty() {
            let start = lattice.skip_separators(0);
            if start < lattice.len {
                let first_matches = PrismFsa::global().match_syllables(&lattice.raw[start..], 0);
                if let Some((first_syl, rel_end)) = first_matches.last().copied() {
                    let consumed = start + rel_end;
                    let cands = self.resources.table.lookup_top_k(first_syl, 12);
                    for mut cand in cands {
                        if self.config.filter_rare_ext_chars && is_rare_ext_char(&cand.text) {
                            continue;
                        }
                        cand.tier = CandidateTier::PrefixPartial;
                        cand.consumed_len = Some(consumed);
                        out.push(cand);
                    }
                } else {
                    let initial = &lattice.raw[start..start + 1];
                    if let Some(entries) = self.resources.table.get_initial_raw_entries(initial) {
                        for (text, score, word_id) in entries.into_iter().take(6) {
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
                                consumed_len: Some(start + 1),
                                sub_candidates: Vec::new(),
                                word_id,
                            });
                        }
                    }
                }
            }
        }
    }

    /// 叠加用户自适应词频权重、词对权重与上一轮上屏上下文
    fn apply_user_and_context_boost(&self, candidates: &mut [AlgoCandidate]) {
        if !self.config.dynamic_adjust_freq {
            return;
        }
        let eval_ts = self.adaptive.last_timestamp;
        let prev_ctx = self.composition.last_commit_context();
        let scale = self.config.scoring_weights.user_boost_scale_permille as u64;

        for cand in candidates.iter_mut() {
            let before = cand.score;
            self.adaptive.model.apply_boost(cand, eval_ts);
            let boost_delta = cand.score.saturating_sub(before);
            let scaled_delta = ((boost_delta as u64 * scale) / 1000) as u32;
            cand.score = before.saturating_add(scaled_delta);

            if let Some(prev) = prev_ctx {
                let pair_boost = self
                    .adaptive
                    .model
                    .pair_boost_score(prev, &cand.text, eval_ts);
                if pair_boost > 0 {
                    cand.score = cand.score.saturating_add(pair_boost as u32);
                }
            }

            if cand.tier == CandidateTier::ExactWord {
                let raw_freq = extract_raw_score(cand.score);
                if raw_freq > 0 {
                    cand.score = cand.score.max(raw_freq);
                }
            }
        }
    }

    /// 未开启模糊音时，对精确解码偏弱的输入探查常用模糊音高频整词兜底
    fn collect_fuzzy_fallback_candidates(
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
                pinyin: m.matched_key.clone(),
                score: to_positive_score(m_score),
                comment: Some(format!("[模糊: {}]", m.matched_key)),
                tier: CandidateTier::RuleNormalization,
                is_expandable: false,
                consumed_len: Some(raw_input.len()),
                sub_candidates: Vec::new(),
                word_id: m.word_id,
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
