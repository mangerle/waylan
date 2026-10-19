use std::collections::HashSet;

use super::typo::SpatialTypoCorrector;
use super::AlgoEngine;
use crate::evaluator::DateTimeEvaluator;
use crate::prism::SegmentPath;
use crate::syllables::is_valid_syllable;
use crate::table::{calculate_length_boost, extract_raw_score};
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
        let is_broken_typo = paths
            .first()
            .is_some_and(|p| p.is_broken_typo_fallback(&self.config.fuzzy));

        // 1. 容错门禁控制：在无合法成词命中时注入物理空间纠错
        self.inject_typo_candidates_if_needed(raw_input, is_broken_typo, candidates);

        // 2. 前缀高频短词混排注入（支持长句分段确认 Partial Commit，仅在正常切分路径上启用）
        if let Some(best_path) = paths.first() {
            if !is_broken_typo {
                self.inject_prefix_segment_candidates(best_path, candidates);
            }
        }

        // 3. 收集并编排扩展项（自定义短语、动态时间、英文联想、符号表情）
        let extensions = self.collect_page_zero_extensions(raw_input, candidates);
        self.merge_extensions_by_hierarchy(extensions, candidates);
    }

    /// 在无完整合法成词命中时注入物理空间键盘按键纠错候选
    fn inject_typo_candidates_if_needed(
        &self,
        raw_input: &str,
        is_broken_typo: bool,
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        let has_exact_or_prefix_words = candidates.iter().any(|c| {
            c.tier == CandidateTier::ExactWord || c.tier == CandidateTier::ProtectedEntity
        });
        if has_exact_or_prefix_words {
            return;
        }

        let mut merged_typos: Vec<AlgoCandidate> = candidates
            .iter()
            .filter(|c| c.tier == CandidateTier::TypoCorrection)
            .take(2)
            .cloned()
            .collect();

        for mut spatial_cand in
            SpatialTypoCorrector::generate_candidates(raw_input, &self.resources.table)
        {
            if self.config.traditional_chinese {
                spatial_cand.text = self.resources.opencc_dict.convert_text(&spatial_cand.text);
            }
            if !merged_typos.iter().any(|c| c.text == spatial_cand.text) {
                merged_typos.push(spatial_cand);
            }
        }

        for (i, mut typo_cand) in merged_typos.into_iter().take(2).enumerate() {
            typo_cand.consumed_len = Some(raw_input.len());
            candidates.retain(|c| c.text != typo_cand.text);
            let target_pos = if is_broken_typo || candidates.is_empty() {
                i.min(candidates.len())
            } else {
                (1 + i).min(candidates.len())
            };
            candidates.insert(target_pos, typo_cand);
        }
    }

    /// 收集首页扩展建议（自定义短语、动态日期时间、英文词汇与特殊符号）
    fn collect_page_zero_extensions(
        &self,
        raw_input: &str,
        existing_candidates: &[AlgoCandidate],
    ) -> Vec<AlgoCandidate> {
        let mut extensions: Vec<AlgoCandidate> = Vec::new();

        // 1. 用户自定义短语（最高优先级，标记为 ProtectedEntity 置顶）
        let phrase_cands = self.resources.custom_phrases.lookup(raw_input);
        for mut phrase in phrase_cands {
            if self.config.traditional_chinese {
                phrase.text = self.resources.opencc_dict.convert_text(&phrase.text);
            }
            phrase.tier = CandidateTier::ProtectedEntity;
            phrase.consumed_len = Some(raw_input.len());
            extensions.push(phrase);
        }

        // 2. 动态扩展功能（日期、时间、干支等强相关功能卡片）
        if let Some(mut dynamic_cand) = DateTimeEvaluator::try_evaluate(raw_input) {
            if self.config.traditional_chinese {
                dynamic_cand.text = self.resources.opencc_dict.convert_text(&dynamic_cand.text);
                for sub in &mut dynamic_cand.sub_candidates {
                    *sub = self.resources.opencc_dict.convert_text(sub);
                }
            }
            dynamic_cand.consumed_len = Some(raw_input.len());
            extensions.push(dynamic_cand);
        }

        // 3. 英文单词联想（次要辅助建议，至多取 1 项避免污染候选列表）
        let eng_cands = self.resources.english_suggester.lookup(raw_input);
        for mut eng in eng_cands.into_iter().take(1) {
            if !existing_candidates
                .iter()
                .any(|c| c.text.eq_ignore_ascii_case(&eng.text))
            {
                eng.consumed_len = Some(raw_input.len());
                extensions.push(eng);
            }
        }

        // 4. 符号与表情联想（次要辅助建议，至多取 1 项）
        let sym_cands = self.resources.symbol_suggester.lookup(raw_input);
        for mut sym in sym_cands.into_iter().take(1) {
            if !existing_candidates.iter().any(|c| c.text == sym.text) {
                sym.consumed_len = Some(raw_input.len());
                extensions.push(sym);
            }
        }

        extensions
    }

    /// 按照确定的语义层级编排扩展候选，杜绝多个扩展项暴力覆盖挤出中文最优词
    fn merge_extensions_by_hierarchy(
        &self,
        extensions: Vec<AlgoCandidate>,
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        for ext in extensions {
            if ext.tier == CandidateTier::ProtectedEntity {
                candidates.retain(|c| c.text != ext.text);
                candidates.insert(0, ext);
            } else if ext.is_expandable {
                candidates.retain(|c| c.text != ext.text);
                let insert_pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(insert_pos, ext);
            } else {
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
            if let Some(mut m) = self
                .resources
                .table
                .lookup_top_k(&key_3, 1)
                .into_iter()
                .next()
            {
                if m.pinyin == key_3 {
                    m.consumed_len = Some(Self::calculate_consumed_len(
                        self.composition.as_str(),
                        &key_3,
                        &best_path.syllables,
                    ));
                    prefix_items.push(self.format_prefix_candidate(m));
                }
            }
        }

        // 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，回退查简拼召回 Top-2 二字高频词）
        let key_2 = best_path.syllables[..2].join("'");
        let mut found_exact_2 = false;
        if let Some(mut m) = self
            .resources
            .table
            .lookup_top_k(&key_2, 1)
            .into_iter()
            .next()
        {
            if m.pinyin == key_2 {
                m.consumed_len = Some(Self::calculate_consumed_len(
                    self.composition.as_str(),
                    &key_2,
                    &best_path.syllables,
                ));
                prefix_items.push(self.format_prefix_candidate(m));
                found_exact_2 = true;
            }
        }
        if !found_exact_2 {
            let acronym_2 = best_path.syllables[..2].concat();
            for mut cand in self.resources.table.lookup_acronym_top_k(&acronym_2, 2) {
                cand.consumed_len = Some(Self::calculate_consumed_len(
                    self.composition.as_str(),
                    &key_2,
                    &best_path.syllables,
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
            cand.text = self.resources.opencc_dict.convert_text(&cand.text);
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
            let acronym_key = initial_chain.concat();
            self.resources.table.lookup_acronym(&acronym_key)
        }
    }

    /// 求解前驱段文本与得分
    fn decode_head_candidates(&self, effective_head_syls: &[String]) -> Vec<(String, String, u32)> {
        let head_key = effective_head_syls.join("'");
        let mut head_options: Vec<(String, String, u32)> = Vec::with_capacity(2);

        if let Some(sentence) = ViterbiDecoder::decode_sentence_with_context(
            effective_head_syls,
            "",
            &self.resources.table,
            self.composition.last_commit_context(),
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
            let head_last_char = head_text
                .chars()
                .last()
                .map(|c| c.to_string())
                .unwrap_or_default();
            for tail in tail_cands.iter().take(5) {
                let phrase_text = format!("{}{}", head_text, tail.text);
                let phrase_pinyin = format!("{}'{}", head_pinyin, tail.pinyin);

                let char_len = (phrase_text.chars().count() as u32).min(64);
                let length_boost = calculate_length_boost(char_len).min(30_000_000);

                let bigram_bonus = self
                    .resources
                    .table
                    .get_bigram_affinity(head_text, &tail.text)
                    .unwrap_or_else(|| {
                        let rule_score =
                            BigramLanguageModel::estimate_bigram_affinity(head_text, &tail.text);
                        if rule_score > 0 {
                            rule_score
                        } else {
                            self.resources
                                .table
                                .get_bigram_affinity(&head_last_char, &tail.text)
                                .unwrap_or(0)
                        }
                    });

                let head_base = extract_raw_score(*head_score) / 8;
                let tail_base = extract_raw_score(tail.score) / 4;
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
