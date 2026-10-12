use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::pipeline::CandidatePipeline;
use crate::prism::PrismEngine;
use crate::prism_fuzzy::PrismFuzzy;
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

impl AlgoEngine {
    pub(crate) fn recompute_candidates(&mut self) {
        let raw_input = self.composition.as_str();
        if raw_input.is_empty() {
            self.selected_index = 0;
            self.is_expanded = false;
            self.expanded_selected_index = 0;
            self.all_candidates_cache.clear();
            self.current_candidates.clear();
            self.cached_preedit.clear();
            return;
        }

        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;

        // 1. 尝试前缀模式分流 (V计算器、U拆字、I特殊符号)
        if let Some(dispatched) = ModeDispatcher::try_dispatch(raw_input, &self.config) {
            self.cached_preedit = raw_input.to_string();
            self.all_candidates_cache = CandidatePipeline::process_all_with_opencc(
                dispatched,
                &self.config,
                Some(&self.opencc_dict),
            );
            self.update_page_candidates();
            return;
        }

        // 2. 拼音模式：利用 Prism 构建 DAG 音节路径
        let paths = PrismEngine::build_paths_with_entities(
            raw_input,
            &self.config.fuzzy,
            Some(&self.entity_manager),
        );
        self.cached_preedit = paths
            .first()
            .map(|p| p.to_key_string())
            .unwrap_or_else(|| raw_input.to_string());

        let mut raw_candidates = Vec::new();

        // 2.0 专有名词与实体保护机制 (NER 原型)：精确命中受保护实体时注入置顶候选
        if let Some(entity) = self.entity_manager.match_exact(raw_input) {
            let mut cand = AlgoCandidate::new(entity, raw_input, 999_999);
            cand.tier = CandidateTier::ProtectedEntity;
            cand.comment = Some("专名".to_string());
            raw_candidates.push(cand);
        }

        let eval_ts = self.user_model.latest_timestamp().max(self.last_timestamp);

        for path in &paths {
            let key = path.to_key_string();

            // 词典精确及倒排检索
            let mut matches = self.table.lookup(&key);
            if path.is_typo {
                for m in &mut matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            }
            raw_candidates.append(&mut matches);

            // 用户自造词检索
            let mut user_matches = self.user_model.lookup_user_words(&key, eval_ts);
            if path.is_typo {
                for m in &mut user_matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            }
            raw_candidates.append(&mut user_matches);

            // 模糊音等价查询键联合检索（设置候选词显式 consumed_len 以保证选词消费长度一致，同时保留标准拼音）
            if self.config.fuzzy.has_active_rules() && !path.is_typo {
                for fuzzy_key in PrismEngine::expand_fuzzy_keys(path, &self.config.fuzzy) {
                    let mut fuzzy_matches = self.table.lookup(&fuzzy_key);
                    for fm in &mut fuzzy_matches {
                        fm.consumed_len = Some(key.len());
                        fm.score = fm.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_matches);

                    let mut fuzzy_user_matches =
                        self.user_model.lookup_user_words(&fuzzy_key, eval_ts);
                    for fum in &mut fuzzy_user_matches {
                        fum.consumed_len = Some(key.len());
                        fum.score = fum.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_user_matches);
                }
            }
        }

        // 2.1 Viterbi 长句动态规划解码（遵循 Best Path Only 原则）
        let min_units = if self.last_commit_context.is_some() {
            1
        } else {
            2
        };
        if let Some(path) = paths.first() {
            let total_units = path.syllables.len() + if path.remaining.is_empty() { 0 } else { 1 };
            if total_units >= min_units {
                if let Some(mut sentence) = ViterbiDecoder::decode_sentence_with_context(
                    &path.syllables,
                    &path.remaining,
                    &self.table,
                    self.last_commit_context.as_deref(),
                ) {
                    sentence.score = sentence.score.saturating_sub(path.cost.saturating_mul(100));
                    raw_candidates.push(sentence);
                }
            }
        }

        if let Some(best_path) = paths.first() {
            // 2.2 前缀残存拼音与声母补全检索（解决诸如 "shux" 召回“熟悉”、“数学”、“属性”等）
            if !best_path.remaining.is_empty() {
                let mut prefix_matches = self
                    .table
                    .lookup_prefix_extension(&best_path.syllables, &best_path.remaining);
                raw_candidates.append(&mut prefix_matches);

                if self.config.fuzzy.has_active_rules() && !best_path.syllables.is_empty() {
                    for fuzzy_syls in PrismFuzzy::expand_fuzzy_syllable_lists(
                        &best_path.syllables,
                        &self.config.fuzzy,
                    ) {
                        let mut fuzzy_prefix = self
                            .table
                            .lookup_prefix_extension(&fuzzy_syls, &best_path.remaining);
                        for fp in &mut fuzzy_prefix {
                            fp.score = fp.score.saturating_sub(50);
                        }
                        raw_candidates.append(&mut fuzzy_prefix);
                    }
                }

                // 2.2.1 跨词边界短语前缀合成检索（解决诸如 "womendeshij" 召回“我们的时间”、“我们的世界”等）
                let mut phrase_matches =
                    self.synthesize_prefix_phrases(&best_path.syllables, &best_path.remaining);
                raw_candidates.append(&mut phrase_matches);

                // 若前序已完成音节非空，同时将首段已完成音节的候选词作为降级备选（如“数”、“书”、“树”）
                if !best_path.syllables.is_empty() {
                    let base_key = best_path.syllables.join("'");
                    let mut base_matches = self.table.lookup_top_k(&base_key, 32);
                    for bm in &mut base_matches {
                        bm.tier = CandidateTier::PrefixPartial;
                    }
                    raw_candidates.append(&mut base_matches);
                }
            }
        }

        // 2.2 检索首字母缩写简拼（支持用户输入单引号隔开的简拼，如 bj / b'j -> 北京，yyds -> 永远的神）
        let acronym_query = if raw_input.contains('\'') {
            raw_input.replace('\'', "")
        } else {
            raw_input.to_string()
        };
        let mut acronym_matches = self.table.lookup_acronym_top_k(&acronym_query, 64);
        if raw_input.contains('\'') {
            for am in &mut acronym_matches {
                am.consumed_len = Some(raw_input.len());
            }
        }
        raw_candidates.append(&mut acronym_matches);

        // 2.3 将首音节已完成音节或首声母单字作为降级备选（如 "wo" -> "我", "握", "卧"...）
        // 赋予 PrefixPartial 优先级，确保整词始终排在最前，同时单字可排在后续行供网格面板展开与翻页
        if let Some(first_path) = paths.first() {
            if !first_path.syllables.is_empty() {
                let first_syl = &first_path.syllables[0];
                let mut first_matches = self.table.lookup_top_k(first_syl, 32);
                if self.config.fuzzy.has_active_rules() {
                    for alt_syl in
                        PrismEngine::expand_fuzzy_syllables(first_syl, &self.config.fuzzy)
                    {
                        let mut alt_matches = self.table.lookup_top_k(&alt_syl, 32);
                        for am in &mut alt_matches {
                            am.consumed_len = Some(first_syl.len());
                            am.score = am.score.saturating_sub(50);
                        }
                        first_matches.append(&mut alt_matches);
                    }
                }
                if first_matches.is_empty() {
                    if let Some(initial_entries) = self.table.get_initial_raw_entries(first_syl) {
                        for (text, score, word_id) in initial_entries {
                            first_matches.push(
                                AlgoCandidate::new(text, first_syl.clone(), score)
                                    .with_tier(CandidateTier::PrefixPartial)
                                    .with_word_id(word_id),
                            );
                        }
                    }
                } else {
                    for fm in &mut first_matches {
                        fm.tier = CandidateTier::PrefixPartial;
                    }
                }
                raw_candidates.append(&mut first_matches);
            }
        }

        // 3. 动态应用用户习惯提权与上屏历史上下文亲和度加权
        let prev_context_info = self
            .last_commit_context
            .as_deref()
            .map(|prev| (prev, self.table.get_word_id(prev)));

        for c in &mut raw_candidates {
            if c.tier != CandidateTier::UserHistory {
                self.user_model.apply_boost(c, eval_ts);
            }
            if let Some((prev, prev_id_opt)) = prev_context_info {
                let affinity = if let Some(prev_id) = prev_id_opt {
                    let curr_id = c.word_id.or_else(|| self.table.get_word_id(&c.text));
                    if let Some(cid) = curr_id {
                        self.table
                            .get_bigram_affinity_by_ids(prev_id, cid)
                            .unwrap_or_else(|| {
                                BigramLanguageModel::estimate_bigram_affinity(prev, &c.text)
                            })
                    } else {
                        BigramLanguageModel::estimate_bigram_affinity(prev, &c.text)
                    }
                } else {
                    BigramLanguageModel::estimate_bigram_affinity(prev, &c.text)
                };

                if affinity > 0 {
                    c.score = c.score.saturating_add((affinity as u32).min(50_000));
                }
            }
        }

        // 4. 送入后处理流水线（去重、简繁、字符集、注释），保留全量已排序去重候选
        let mut candidates = CandidatePipeline::process_all_with_opencc(
            raw_candidates,
            &self.config,
            Some(&self.opencc_dict),
        );

        self.inject_page_zero_extensions(raw_input, &paths, &mut candidates);

        self.all_candidates_cache = candidates;
        self.update_page_candidates();
    }
}
