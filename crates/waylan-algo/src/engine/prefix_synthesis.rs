use std::collections::HashSet;

use super::typo::SpatialTypoCorrector;
use super::AlgoEngine;
use crate::evaluator::DateTimeEvaluator;
use crate::prism::SegmentPath;
use crate::syllables::is_valid_syllable;
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

impl AlgoEngine {
    /// 注入首页扩展候选（物理空间纠错、动态日期时间、英文联想、符号表情、自定义短语与前缀分段短词）
    pub(crate) fn inject_page_zero_extensions(
        &self,
        raw_input: &str,
        paths: &[SegmentPath],
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        // 1. 状态机感知与容错门禁控制：
        // 若当前输入处于正常的合法前缀打字态（已成功检索出 ExactWord 级别的完整成词或受保护专名），
        // 免受激进物理空间容错打扰；仅在无任何合法成词命中时（如输入 nihso, cehng），才激活次位容错。
        let has_exact_or_prefix_words = candidates.iter().any(|c| {
            c.tier == CandidateTier::ExactWord || c.tier == CandidateTier::ProtectedEntity
        });

        if !has_exact_or_prefix_words {
            let typo_cands = SpatialTypoCorrector::generate_candidates(raw_input, &self.table);
            for (i, mut typo_cand) in typo_cands.into_iter().take(2).enumerate() {
                if self.config.traditional_chinese {
                    typo_cand.text = self.opencc_dict.convert_text(&typo_cand.text);
                }
                candidates.retain(|c| c.text != typo_cand.text);
                let target_pos = if candidates.is_empty() {
                    0
                } else {
                    (1 + i).min(candidates.len())
                };
                candidates.insert(target_pos, typo_cand);
            }
        }

        // 2. 前缀高频短词混排注入（支持长句分段确认 Partial Commit，先于扩展项编排避免偏移错乱）
        if let Some(best_path) = paths.first() {
            self.inject_prefix_segment_candidates(best_path, candidates);
        }

        // 3. 收集扩展项（用户自定义短语、动态日期时间、英文单词联想、符号表情）
        let mut extensions: Vec<AlgoCandidate> = Vec::new();

        // 3.1 用户自定义短语（最高优先级，标记为 ProtectedEntity 置顶）
        let phrase_cands = self.custom_phrases.lookup(raw_input);
        for mut phrase in phrase_cands {
            if self.config.traditional_chinese {
                phrase.text = self.opencc_dict.convert_text(&phrase.text);
            }
            phrase.tier = CandidateTier::ProtectedEntity;
            extensions.push(phrase);
        }

        // 3.2 动态扩展功能（日期、时间、干支等强相关功能卡片）
        if let Some(mut dynamic_cand) = DateTimeEvaluator::try_evaluate(raw_input) {
            if self.config.traditional_chinese {
                dynamic_cand.text = self.opencc_dict.convert_text(&dynamic_cand.text);
                for sub in &mut dynamic_cand.sub_candidates {
                    *sub = self.opencc_dict.convert_text(sub);
                }
            }
            extensions.push(dynamic_cand);
        }

        // 3.3 英文单词联想（次要辅助建议，至多取 1 项避免污染候选列表）
        let eng_cands = self.english_suggester.lookup(raw_input);
        for eng in eng_cands.into_iter().take(1) {
            if !candidates
                .iter()
                .any(|c| c.text.eq_ignore_ascii_case(&eng.text))
            {
                extensions.push(eng);
            }
        }

        // 3.4 符号与表情联想（次要辅助建议，至多取 1 项）
        let sym_cands = self.symbol_suggester.lookup(raw_input);
        for sym in sym_cands.into_iter().take(1) {
            if !candidates.iter().any(|c| c.text == sym.text) {
                extensions.push(sym);
            }
        }

        // 4. 按照确定的语义层级编排扩展候选，杜绝多个扩展项连续 insert(1) 暴力覆盖挤出中文最优词
        for ext in extensions {
            if ext.tier == CandidateTier::ProtectedEntity {
                // 自定义短语置顶
                candidates.retain(|c| c.text != ext.text);
                candidates.insert(0, ext);
            } else if ext.is_expandable {
                // 动态日期/时间功能项：若已有候选项，插入到 index 1（紧随首选常规成词），否则置顶
                candidates.retain(|c| !(c.text == ext.text && c.is_expandable));
                let insert_pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(insert_pos, ext);
            } else {
                // 英文联想与符号表情等辅助备选项：
                // 排在核心成词与动态功能之后（至少在 index 2 或末尾），确保首选与次选中文成词不受挤压
                candidates.retain(|c| c.text != ext.text);
                let insert_pos = if candidates.len() <= 1 {
                    candidates.len()
                } else {
                    2.min(candidates.len())
                };
                candidates.insert(insert_pos, ext);
            }
        }
    }

    /// 提取前 3 音节与前 2 音节高频成词并混排注入到全长成词之后
    fn inject_prefix_segment_candidates(
        &self,
        best_path: &SegmentPath,
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        let total_syls =
            best_path.syllables.len() + if best_path.remaining.is_empty() { 0 } else { 1 };
        if total_syls < 3 || best_path.syllables.len() < 2 {
            return;
        }

        let mut prefix_items = Vec::new();

        // 若总音节 >= 4 且已完成音节 >= 3，优先提取前 3 音节全拼高频词（如 wo'men'de -> 我们的）
        if total_syls >= 4 && best_path.syllables.len() >= 3 {
            let key_3 = best_path.syllables[..3].join("'");
            if let Some(m) = self.table.lookup_top_k(&key_3, 1).into_iter().next() {
                if m.pinyin == key_3 {
                    prefix_items.push(self.format_prefix_candidate(m));
                }
            }
        }

        // 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，回退查简拼召回 Top-2 二字高频词）
        let key_2 = best_path.syllables[..2].join("'");
        let mut found_exact_2 = false;
        if let Some(m) = self.table.lookup_top_k(&key_2, 1).into_iter().next() {
            if m.pinyin == key_2 {
                prefix_items.push(self.format_prefix_candidate(m));
                found_exact_2 = true;
            }
        }
        if !found_exact_2 {
            let acronym_2 = best_path.syllables[..2].concat();
            for mut cand in self.table.lookup_acronym_top_k(&acronym_2, 2) {
                cand.consumed_len = Some(Self::compute_consumed_len(
                    self.composition.as_str(),
                    &key_2,
                ));
                prefix_items.push(self.format_prefix_candidate(cand));
            }
        }

        if prefix_items.is_empty() {
            return;
        }

        let mut injected = Vec::with_capacity(candidates.len() + prefix_items.len());
        let mut seen = HashSet::with_capacity(candidates.len() + prefix_items.len());

        let full_len_count = candidates
            .iter()
            .take_while(|c| {
                c.tier != CandidateTier::PrefixPartial && c.text.chars().count() >= total_syls
            })
            .count();
        let keep_count = full_len_count.clamp(2, 3).min(candidates.len());

        for c in candidates.drain(..keep_count) {
            if seen.insert(c.text.clone()) {
                injected.push(c);
            }
        }
        for pi in prefix_items {
            if seen.insert(pi.text.clone()) {
                injected.push(pi);
            }
        }
        for c in candidates.drain(..) {
            if seen.insert(c.text.clone()) {
                injected.push(c);
            }
        }

        *candidates = injected;
    }

    /// 归一化前缀分段候选词的简繁文本、拼音注释与层级标签
    fn format_prefix_candidate(&self, mut cand: AlgoCandidate) -> AlgoCandidate {
        if self.config.traditional_chinese {
            cand.text = self.opencc_dict.convert_text(&cand.text);
        }
        if self.config.show_pinyin_comment && cand.comment.is_none() && !cand.pinyin.is_empty() {
            cand.comment = Some(format!("[{}]", cand.pinyin));
        }
        cand.tier = CandidateTier::PrefixPartial;
        cand
    }

    /// 跨词边界短语前缀合成检索（解决诸如 "womendeshij" 召回“我们的时间”、“我们的世界”等）
    pub(crate) fn synthesize_prefix_phrases(
        &self,
        syllables: &[String],
        remaining: &str,
    ) -> Vec<AlgoCandidate> {
        let n = syllables.len();
        if n == 0 || remaining.is_empty() {
            return Vec::new();
        }

        // 1. 动态向前回溯提取尾部声母链 initial_chain
        let mut initial_chain = vec![remaining.to_string()];
        let mut split_idx = n;

        while split_idx > 0 {
            let prev_syl = &syllables[split_idx - 1];
            if !is_valid_syllable(prev_syl) {
                initial_chain.insert(0, prev_syl.clone());
                split_idx -= 1;
            } else {
                break;
            }
        }

        let k = initial_chain.len();
        let head_syls = &syllables[..split_idx];

        // 2. 尾部展开候选词集合 (tail_cands)
        let tail_cands: Vec<AlgoCandidate> = if k == 1 {
            if split_idx == 0 {
                return Vec::new();
            }
            let last_syl_idx = split_idx - 1;
            let tail_syl = &syllables[last_syl_idx..split_idx];
            self.table.lookup_prefix_extension(tail_syl, remaining)
        } else {
            let acronym_key = initial_chain.concat();
            self.table.lookup_acronym(&acronym_key)
        };

        if tail_cands.is_empty() {
            return Vec::new();
        }

        // 3. 确定真正的前驱短语音节切片 (effective_head_syls)
        let effective_head_syls = if k == 1 {
            if split_idx == 0 {
                return Vec::new();
            }
            &syllables[..split_idx - 1]
        } else {
            head_syls
        };

        if effective_head_syls.is_empty() {
            return tail_cands;
        }

        // 4. 求解前驱段文本与得分 (如 wo'men'gong'tong'de -> 我们共同的)
        let head_key = effective_head_syls.join("'");
        let mut head_options: Vec<(String, String, u32)> = Vec::with_capacity(2);

        if let Some(sentence) = ViterbiDecoder::decode_sentence_with_context(
            effective_head_syls,
            "",
            &self.table,
            self.last_commit_context.as_deref(),
        ) {
            head_options.push((sentence.text, head_key.clone(), sentence.score));
        }

        let exact_matches = self.table.lookup_top_k(&head_key, 1);
        if let Some(best_exact) = exact_matches.first() {
            if !head_options.iter().any(|(t, _, _)| t == &best_exact.text) {
                head_options.push((best_exact.text.clone(), head_key, best_exact.score));
            }
        }

        // 5. 跨词短语笛卡尔积合成与打分
        let mut results = Vec::with_capacity(head_options.len() * 5);
        for (head_text, head_pinyin, head_score) in &head_options {
            let head_last_char = head_text
                .chars()
                .last()
                .map(|c| c.to_string())
                .unwrap_or_default();
            for tail in tail_cands.iter().take(5) {
                let phrase_text = format!("{}{}", head_text, tail.text);
                let phrase_pinyin = format!("{}'{}", head_pinyin, tail.pinyin);

                let char_len = (phrase_text.chars().count() as u32).min(64);
                let length_boost = char_len
                    .saturating_sub(1)
                    .saturating_mul(600_000)
                    .min(30_000_000);

                let bigram_bonus = self
                    .table
                    .get_bigram_affinity(head_text, &tail.text)
                    .unwrap_or_else(|| {
                        let rule_score =
                            BigramLanguageModel::estimate_bigram_affinity(head_text, &tail.text);
                        if rule_score > 0 {
                            rule_score
                        } else {
                            self.table
                                .get_bigram_affinity(&head_last_char, &tail.text)
                                .unwrap_or(0)
                        }
                    });

                let head_base = (*head_score % 600_000) / 8;
                let tail_base = (tail.score % 600_000) / 4;
                let final_score = head_base
                    .saturating_add(tail_base)
                    .saturating_add(length_boost)
                    .saturating_add((bigram_bonus.max(0) as u32).min(50_000) / 2);

                let cand = AlgoCandidate::new(phrase_text, phrase_pinyin, final_score)
                    .with_tier(CandidateTier::ExactWord);
                results.push(cand);
            }
        }

        results.sort_by_key(|c| std::cmp::Reverse(c.score));
        results.truncate(10);
        results
    }
}
