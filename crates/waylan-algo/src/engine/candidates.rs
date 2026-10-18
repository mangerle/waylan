use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::pipeline::CandidatePipeline;
use crate::prism::{PrismEngine, PrismFuzzy, SegmentPath};
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

impl AlgoEngine {
    /// 重新计算当前输入序列的全量候选词列表并分页刷新
    pub(crate) fn recompute_candidates(&mut self) {
        if self.composition.is_empty() {
            self.clear_candidates_and_preedit();
            return;
        }

        self.viewport.reset_selection();

        // 1. 尝试前缀模式分流 (V计算器、U拆字、I特殊符号)
        if self.try_dispatch_special_mode() {
            return;
        }

        // 2. 拼音模式：利用 Prism 构建 DAG 音节路径与预编辑字符串
        let (paths, preedit_str) = self.build_pinyin_paths_and_preedit();
        self.composition.set_cached_preedit(preedit_str);

        // 3. 聚合各层级候选词 (专名、DAG路径、长句合成、简拼与用户习惯)
        let raw_candidates = self.collect_all_raw_candidates(&paths);

        // 4. 流水线后处理、零页扩展注入与分页刷新
        self.finalize_and_publish_candidates(&paths, raw_candidates);
    }

    /// 清空所有候选词缓存与预编辑内容
    fn clear_candidates_and_preedit(&mut self) {
        self.viewport.reset_selection();
        self.viewport.all_candidates_cache.clear();
        self.viewport.current_candidates.clear();
        self.composition.clear_cached_preedit();
    }

    /// 尝试前缀特殊模式分流 (V计算器、U拆字、I特殊符号)，若命中则更新候选并返回 true
    fn try_dispatch_special_mode(&mut self) -> bool {
        let Some(dispatched) =
            ModeDispatcher::try_dispatch(self.composition.as_str(), &self.config)
        else {
            return false;
        };

        let input_len = self.composition.len();
        let raw_str = self.composition.as_str().to_string();
        self.composition.set_cached_preedit(raw_str);
        let mut cands = CandidatePipeline::process_all_with_opencc(
            dispatched,
            &self.config,
            Some(&self.resources.opencc_dict),
        );
        for c in &mut cands {
            if c.consumed_len.is_none() {
                c.consumed_len = Some(input_len);
            }
        }
        self.viewport.all_candidates_cache = cands;
        self.update_page_candidates();
        true
    }

    /// 基于 Prism 构建音节切分 DAG 路径与预编辑字符串
    fn build_pinyin_paths_and_preedit(&self) -> (Vec<SegmentPath>, String) {
        let raw = self.composition.as_str();
        let paths = PrismEngine::build_paths_with_entities(
            raw,
            &self.config.fuzzy,
            Some(&self.resources.entity_manager),
        );
        let preedit_str = paths
            .first()
            .map(|p| {
                if p.is_broken_typo_fallback(&self.config.fuzzy) {
                    paths
                        .iter()
                        .find(|tp| tp.is_typo && tp.remaining.is_empty())
                        .and_then(|tp| tp.format_raw_with_spans(raw))
                        .unwrap_or_else(|| p.to_key_string())
                } else {
                    p.to_key_string()
                }
            })
            .unwrap_or_else(|| raw.to_string());
        (paths, preedit_str)
    }

    /// 聚合各维度候选词（专名实体、DAG 路径匹配、长句前缀合成、简拼降级与习惯加权）
    fn collect_all_raw_candidates(&mut self, paths: &[SegmentPath]) -> Vec<AlgoCandidate> {
        let raw_input = self.composition.as_str();
        let mut raw_candidates = std::mem::take(&mut self.scratch_candidates);
        raw_candidates.clear();

        // 1. 专名保护机制精确匹配
        if let Some(entity) = self.resources.entity_manager.match_exact(raw_input) {
            let mut cand = AlgoCandidate::new(entity, raw_input, 999_999);
            cand.tier = CandidateTier::ProtectedEntity;
            cand.comment = Some("专名".to_string());
            cand.consumed_len = Some(raw_input.len());
            raw_candidates.push(cand);
        }

        let eval_ts = self
            .adaptive
            .model
            .latest_timestamp()
            .max(self.adaptive.last_timestamp);

        // 2. 收集各级候选
        self.collect_path_matches(paths, eval_ts, &mut raw_candidates);
        self.collect_sentence_and_prefix_matches(paths, &mut raw_candidates);
        self.collect_acronym_and_fallback_matches(raw_input, paths, &mut raw_candidates);

        // 3. 应用用户词库提权与上下文亲和度加权
        self.apply_user_and_context_boost(&mut raw_candidates, eval_ts);

        raw_candidates
    }

    /// 候选词流水线最终处理（去重、简繁、零页扩展、标记 consumed_len）并分页生效
    fn finalize_and_publish_candidates(
        &mut self,
        paths: &[SegmentPath],
        raw_candidates: Vec<AlgoCandidate>,
    ) {
        let raw_input = self.composition.as_str();
        let mut best_units: Vec<String> = Vec::new();
        if let Some(best) = paths.first() {
            best_units.extend(best.syllables.iter().cloned());
            if !best.remaining.is_empty() {
                best_units.push(best.remaining.clone());
            }
        }

        let mut candidates = CandidatePipeline::process_all_with_opencc(
            raw_candidates,
            &self.config,
            Some(&self.resources.opencc_dict),
        );

        self.inject_page_zero_extensions(raw_input, paths, &mut candidates);

        for cand in &mut candidates {
            if cand.consumed_len.is_none() {
                cand.consumed_len = Some(Self::calculate_consumed_len(
                    raw_input,
                    &cand.pinyin,
                    &best_units,
                ));
            }
        }

        let old_cache = std::mem::replace(&mut self.viewport.all_candidates_cache, candidates);
        self.scratch_candidates = old_cache;
        self.update_page_candidates();
    }

    /// 遍历 DAG 音节切分路径，收集词库精确、用户自造词及模糊音匹配候选
    fn collect_path_matches(
        &self,
        paths: &[SegmentPath],
        eval_ts: u64,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let Some(best_path) = paths.first() else {
            return;
        };
        let best_syl_count = (best_path.syllables.len() as u32).max(1);
        let best_cost = best_path.cost;

        for path in paths {
            let key = path.to_key_string();
            let cost_penalty = path.cost.saturating_sub(best_cost).saturating_mul(100);
            let is_suboptimal = !path.is_typo
                && (cost_penalty > 0 || (path.syllables.len() as u32) > best_syl_count);

            // 词典精确及倒排检索
            let mut matches = self.resources.table.lookup(&key);
            if path.is_typo {
                for m in &mut matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            } else if is_suboptimal {
                for m in &mut matches {
                    m.score = Self::calibrate_lexicon_path_score(
                        m.score,
                        &m.text,
                        best_syl_count,
                        cost_penalty,
                    );
                }
            }
            results.append(&mut matches);

            // 用户自造词检索
            let mut user_matches = self.adaptive.model.lookup_user_words(&key, eval_ts);
            if path.is_typo {
                for m in &mut user_matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            } else if is_suboptimal {
                for m in &mut user_matches {
                    m.score = m.score.saturating_sub(cost_penalty).max(1);
                }
            }
            results.append(&mut user_matches);

            // 模糊音等价查询键联合检索
            if self.config.fuzzy.has_active_rules() && !path.is_typo {
                self.collect_fuzzy_path_matches(
                    path,
                    eval_ts,
                    best_syl_count,
                    cost_penalty,
                    results,
                );
            }
        }
    }

    /// 收集单条非容错切分路径在模糊音扩展下的词库与用户自造词候选
    fn collect_fuzzy_path_matches(
        &self,
        path: &SegmentPath,
        eval_ts: u64,
        best_syl_count: u32,
        cost_penalty: u32,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let key_len = path.to_key_string().len();
        let is_suboptimal = cost_penalty > 0 || (path.syllables.len() as u32) > best_syl_count;

        for fuzzy_key in PrismEngine::expand_fuzzy_keys(path, &self.config.fuzzy) {
            let mut fuzzy_matches = self.resources.table.lookup(&fuzzy_key);
            for fm in &mut fuzzy_matches {
                fm.consumed_len = Some(key_len);
                let base = if is_suboptimal {
                    Self::calibrate_lexicon_path_score(
                        fm.score,
                        &fm.text,
                        best_syl_count,
                        cost_penalty,
                    )
                } else {
                    fm.score
                };
                fm.score = base.saturating_sub(50);
            }
            results.append(&mut fuzzy_matches);

            let mut fuzzy_user_matches = self.adaptive.model.lookup_user_words(&fuzzy_key, eval_ts);
            for fum in &mut fuzzy_user_matches {
                fum.consumed_len = Some(key_len);
                fum.score = fum
                    .score
                    .saturating_sub(cost_penalty)
                    .saturating_sub(50)
                    .max(1);
            }
            results.append(&mut fuzzy_user_matches);
        }
    }

    /// 校准次优音节切分路径的词库候选得分：
    /// 剥离因零声母借音碎切导致字数增加而虚增的字长加成（600,000/字），并扣除路径切分代价差值惩罚。
    #[inline]
    fn calibrate_lexicon_path_score(
        score: u32,
        text: &str,
        best_syl_count: u32,
        cost_penalty: u32,
    ) -> u32 {
        let char_len = text.chars().count() as u32;
        let extra_boost = char_len
            .saturating_sub(best_syl_count)
            .saturating_mul(600_000);
        let deduction = extra_boost.saturating_add(cost_penalty);
        score.saturating_sub(deduction).max(1)
    }

    /// 求解 Viterbi 最佳长句路径并合成前缀扩展短语候选
    fn collect_sentence_and_prefix_matches(
        &self,
        paths: &[SegmentPath],
        results: &mut Vec<AlgoCandidate>,
    ) {
        let Some(best_path) = paths.first() else {
            return;
        };

        // 若最优非容错路径属于拼写误触导致的断流碎切路径（如 ying'x'ai'n'g），
        // 禁止在该残破路径上运行 Viterbi 拼句与跨词前缀合成，避免产出“应先爱能够”等生造词挤占纠错候选
        if best_path.is_broken_typo_fallback(&self.config.fuzzy) {
            return;
        }

        let min_units = if self.composition.last_commit_context().is_some() {
            1
        } else {
            2
        };
        let total_units =
            best_path.syllables.len() + if best_path.remaining.is_empty() { 0 } else { 1 };
        if total_units >= min_units {
            if let Some(mut sentence) = ViterbiDecoder::decode_sentence_with_context(
                &best_path.syllables,
                &best_path.remaining,
                &self.resources.table,
                self.composition.last_commit_context(),
            ) {
                sentence.score = sentence
                    .score
                    .saturating_sub(best_path.cost.saturating_mul(100));
                results.push(sentence);
            }
        }

        // 前缀残存拼音补全与跨词短语合成
        if !best_path.remaining.is_empty() {
            let mut prefix_matches = self
                .resources
                .table
                .lookup_prefix_extension(&best_path.syllables, &best_path.remaining);
            results.append(&mut prefix_matches);

            if self.config.fuzzy.has_active_rules() && !best_path.syllables.is_empty() {
                for fuzzy_syls in PrismFuzzy::expand_fuzzy_syllable_lists(
                    &best_path.syllables,
                    &self.config.fuzzy,
                ) {
                    let mut fuzzy_prefix = self
                        .resources
                        .table
                        .lookup_prefix_extension(&fuzzy_syls, &best_path.remaining);
                    for fp in &mut fuzzy_prefix {
                        fp.score = fp.score.saturating_sub(50);
                    }
                    results.append(&mut fuzzy_prefix);
                }
            }

            let mut phrase_matches =
                self.synthesize_prefix_phrases(&best_path.syllables, &best_path.remaining);
            results.append(&mut phrase_matches);

            if !best_path.syllables.is_empty() {
                let base_key = best_path.syllables.join("'");
                let mut base_matches = self.resources.table.lookup_top_k(&base_key, 32);
                for bm in &mut base_matches {
                    bm.tier = CandidateTier::PrefixPartial;
                }
                results.append(&mut base_matches);
            }
        }
    }

    /// 检索首字母缩写简拼与首音节单字降级备选候选
    fn collect_acronym_and_fallback_matches(
        &self,
        raw_input: &str,
        paths: &[SegmentPath],
        results: &mut Vec<AlgoCandidate>,
    ) {
        // 简拼检索
        let acronym_query = if raw_input.contains('\'') {
            raw_input.replace('\'', "")
        } else {
            raw_input.to_string()
        };
        let mut acronym_matches = self
            .resources
            .table
            .lookup_acronym_top_k(&acronym_query, 64);
        if raw_input.contains('\'') {
            for am in &mut acronym_matches {
                am.consumed_len = Some(raw_input.len());
            }
        }
        results.append(&mut acronym_matches);

        // 首音节单字降级备选
        if let Some(first_path) = paths.first() {
            if !first_path.syllables.is_empty() {
                let first_syl = &first_path.syllables[0];
                let mut first_matches = self.resources.table.lookup_top_k(first_syl, 32);
                if self.config.fuzzy.has_active_rules() {
                    for alt_syl in
                        PrismEngine::expand_fuzzy_syllables(first_syl, &self.config.fuzzy)
                    {
                        let mut alt_matches = self.resources.table.lookup_top_k(&alt_syl, 32);
                        for am in &mut alt_matches {
                            am.consumed_len = Some(first_syl.len());
                            am.score = am.score.saturating_sub(50);
                        }
                        first_matches.append(&mut alt_matches);
                    }
                }
                if first_matches.is_empty() {
                    if let Some(initial_entries) =
                        self.resources.table.get_initial_raw_entries(first_syl)
                    {
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
                results.append(&mut first_matches);
            }
        }
    }

    /// 应用自适应词频提权与上屏历史二元语言模型亲和度加权
    fn apply_user_and_context_boost(&self, candidates: &mut [AlgoCandidate], eval_ts: u64) {
        let prev_context_info = self
            .composition
            .last_commit_context()
            .map(|prev| (prev, self.resources.table.get_word_id(prev)));

        for c in candidates {
            if c.tier != CandidateTier::UserHistory {
                self.adaptive.model.apply_boost(c, eval_ts);
            }
            if let Some((prev, prev_id_opt)) = prev_context_info {
                let affinity = if let Some(prev_id) = prev_id_opt {
                    let curr_id = c
                        .word_id
                        .or_else(|| self.resources.table.get_word_id(&c.text));
                    if let Some(cid) = curr_id {
                        self.resources
                            .table
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
    }

    /// 结合当前最优切分路径单元计算候选词所消费的原始按键字节长度
    ///
    /// # 设计原理
    /// - **实现初衷**：在候选词生成阶段，利用已经求解出的音节路径 `best_units` 预先精确计算
    ///   每个候选词所消费的按键长度，并在候选词中记录 `consumed_len`。
    /// - **核心优势**：彻底消除用户选词上屏时重新调用 `PrismEngine::build_paths_with_entities`
    ///   求解音节图路径的微秒级延迟，将按键上屏关键交互路径耗时降为纯内存字段读取。
    pub(crate) fn calculate_consumed_len(raw: &str, pinyin: &str, best_units: &[String]) -> usize {
        if raw.is_empty() {
            return 0;
        }

        if raw.starts_with(pinyin) && !pinyin.is_empty() {
            let mut len = pinyin.len();
            if raw[len..].starts_with('\'') {
                len = len.saturating_add(1);
            }
            return len;
        }

        // 全拼与简拼混合模式：优先按候选音节逐段对齐原始输入或最优切分路径单元
        let cand_syls: Vec<&str> = pinyin.split('\'').filter(|s| !s.is_empty()).collect();
        let mut pos = 0usize;
        for (i, &cs) in cand_syls.iter().enumerate() {
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
            if pos >= raw.len() {
                break;
            }
            if raw[pos..].starts_with(cs) {
                pos += cs.len();
            } else if i < best_units.len() && raw[pos..].starts_with(best_units[i].as_str()) {
                pos += best_units[i].len();
            } else {
                let first_len = cs.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if raw[pos..].starts_with(&cs[..first_len]) {
                    pos += first_len;
                } else {
                    return Self::compute_consumed_len(raw, pinyin);
                }
            }
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
        }

        if pos > 0 {
            pos.min(raw.len())
        } else {
            Self::compute_consumed_len(raw, pinyin)
        }
    }
}
