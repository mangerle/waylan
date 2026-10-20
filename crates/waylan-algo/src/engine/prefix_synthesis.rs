//! 跨词边界短语前缀合成与二元文法打分
//!
//! # 设计原理
//! - **实现初衷**：解决诸如 "womendeshij" 等在全拼输入未完结时，跨词边界召回“我们的时间”、“我们的世界”等高频二元短语。
//! - **核心优势**：基于动态音节回溯切分前驱与尾部声母链，结合 Viterbi 最优前驱与尾部前缀展开，实现高质量跨词合成。
//! - **代价与局限**：涉及笛卡尔积合成与二元亲和度打分计算，限制最大返回候选项数量以保证微秒级响应。

use super::AlgoEngine;
use crate::syllables::is_valid_syllable;
use crate::table::{calculate_length_boost, extract_raw_score};
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

/// 跨词短语合成的前驱片段上下文参数对象
///
/// 用途：在笛卡尔积合成中复用同一份前驱候选的文本、拼音、得分与末字信息，避免参数列表膨胀。
struct PhraseHead<'a> {
    /// 前驱候选文本
    text: &'a str,
    /// 前驱候选拼音键
    pinyin: &'a str,
    /// 前驱候选原始得分
    score: u32,
    /// 前驱候选末字（用于二元亲和度的末字回退查询）
    last_char: String,
}

impl AlgoEngine {
    /// 跨词边界短语前缀合成检索（解决诸如 "womendeshij" 召回“我们的时间”、“我们的世界”等）
    pub(crate) fn synthesize_prefix_phrases(
        &self,
        syllables: &[String],
        remaining: &str,
    ) -> Vec<AlgoCandidate> {
        let (split_idx, initial_chain) =
            match self.resolve_head_and_tail_split(syllables, remaining) {
                Some(res) => res,
                None => return Vec::new(),
            };

        // 1. 检索尾部展开候选词集合
        let tail_cands = self.lookup_tail_cands(&initial_chain, syllables, split_idx, remaining);
        if tail_cands.is_empty() {
            return Vec::new();
        }

        // 2. 确定真正的前驱短语音节切片
        let effective_head_syls = if initial_chain.len() == 1 {
            if split_idx == 0 {
                return Vec::new();
            }
            &syllables[..split_idx - 1]
        } else {
            &syllables[..split_idx]
        };

        if effective_head_syls.is_empty() {
            return tail_cands;
        }

        // 3. 求解前驱段候选并与尾部进行笛卡尔积合成与打分
        let head_options = self.decode_head_candidates(effective_head_syls);
        self.combine_and_score_phrases(&head_options, &tail_cands)
    }

    /// 动态向前回溯切分声母链与前驱音节
    fn resolve_head_and_tail_split(
        &self,
        syllables: &[String],
        remaining: &str,
    ) -> Option<(usize, Vec<String>)> {
        let n = syllables.len();
        if n == 0 || remaining.is_empty() {
            return None;
        }

        let mut initial_chain = vec![remaining.to_string()];
        let mut split_idx = n;

        while split_idx > 0 {
            let prev_syl = &syllables[split_idx - 1];
            if !is_valid_syllable(prev_syl) || matches!(prev_syl.as_str(), "n" | "ng" | "m") {
                initial_chain.insert(0, prev_syl.clone());
                split_idx -= 1;
            } else {
                break;
            }
        }

        let head_syls = &syllables[..split_idx];
        if !head_syls.iter().all(|s| {
            is_valid_syllable(s) && (s.len() >= 2 || matches!(s.as_str(), "a" | "o" | "e"))
        }) {
            return None;
        }

        Some((split_idx, initial_chain))
    }

    /// 检索尾部展开候选词集合
    fn lookup_tail_cands(
        &self,
        initial_chain: &[String],
        syllables: &[String],
        split_idx: usize,
        remaining: &str,
    ) -> Vec<AlgoCandidate> {
        if initial_chain.len() == 1 {
            if split_idx == 0 {
                return Vec::new();
            }
            let last_syl_idx = split_idx - 1;
            let tail_syl = &syllables[last_syl_idx..split_idx];
            self.resources
                .table
                .lookup_prefix_extension(tail_syl, remaining)
        } else {
            if !self.config.enable_acronym {
                return Vec::new();
            }
            let acronym_key = initial_chain.concat();
            self.resources.table.lookup_acronym(&acronym_key)
        }
    }

    /// 求解前驱段文本与得分
    fn decode_head_candidates(&self, effective_head_syls: &[String]) -> Vec<(String, String, u32)> {
        let head_key = effective_head_syls.join("'");
        let mut head_options: Vec<(String, String, u32)> = Vec::with_capacity(2);

        if let Some(sentence) = ViterbiDecoder::decode_sentence_with_context_options(
            effective_head_syls,
            "",
            &self.resources.table,
            self.composition.last_commit_context(),
            self.config.enable_acronym,
        ) {
            head_options.push((sentence.text, head_key.clone(), sentence.score));
        }

        let exact_matches = self.resources.table.lookup_top_k(&head_key, 1);
        if let Some(best_exact) = exact_matches.first() {
            if !head_options.iter().any(|(t, _, _)| t == &best_exact.text) {
                head_options.push((best_exact.text.clone(), head_key, best_exact.score));
            }
        }

        head_options
    }

    /// 跨词短语笛卡尔积合成与二元文法打分
    fn combine_and_score_phrases(
        &self,
        head_options: &[(String, String, u32)],
        tail_cands: &[AlgoCandidate],
    ) -> Vec<AlgoCandidate> {
        let mut results = Vec::with_capacity(head_options.len() * 5);
        for (head_text, head_pinyin, head_score) in head_options {
            let head = PhraseHead {
                text: head_text,
                pinyin: head_pinyin,
                score: *head_score,
                last_char: head_text
                    .chars()
                    .last()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            };
            for tail in tail_cands.iter().take(5) {
                results.push(self.build_phrase_candidate(&head, tail));
            }
        }

        results.sort_by_key(|c| std::cmp::Reverse(c.score));
        results.truncate(10);
        results
    }

    /// 合成单个跨词短语候选并计算最终得分（字长加成 + 二元文法加成）
    fn build_phrase_candidate(&self, head: &PhraseHead<'_>, tail: &AlgoCandidate) -> AlgoCandidate {
        let phrase_text = format!("{}{}", head.text, tail.text);
        let phrase_pinyin = format!("{}'{}", head.pinyin, tail.pinyin);

        let char_len = (phrase_text.chars().count() as u32).min(64);
        let length_boost = calculate_length_boost(char_len).min(30_000_000);

        let bigram_bonus = self.resolve_bigram_bonus(head, tail);
        let head_base = extract_raw_score(head.score) / 8;
        let tail_base = extract_raw_score(tail.score) / 4;
        let final_score = head_base
            .saturating_add(tail_base)
            .saturating_add(length_boost)
            .saturating_add((bigram_bonus.max(0) as u32).min(50_000) / 2);

        AlgoCandidate::new(phrase_text, phrase_pinyin, final_score)
            .with_tier(CandidateTier::ExactWord)
    }

    /// 解析跨词短语的二元文法亲和度：优先整词对命中，回退至前驱末字与尾部词的组合查询
    fn resolve_bigram_bonus(&self, head: &PhraseHead<'_>, tail: &AlgoCandidate) -> i64 {
        self.resources
            .table
            .get_bigram_affinity(head.text, &tail.text)
            .unwrap_or_else(|| {
                let rule_score =
                    BigramLanguageModel::estimate_bigram_affinity(head.text, &tail.text);
                if rule_score > 0 {
                    rule_score
                } else {
                    self.resources
                        .table
                        .get_bigram_affinity(&head.last_char, &tail.text)
                        .unwrap_or(0)
                }
            })
    }
}
