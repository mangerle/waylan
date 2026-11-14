//! 多源候选词并行与分层聚合器
//!
//! # 设计原理
//! - **实现初衷**：将专名保护、词库精确倒排、模糊音等价匹配、Viterbi 长句解码、前缀扩展与首字母简拼等
//!   多层级候选召回逻辑收敛至独立模块，从主候选流水线中解耦。
//! - **核心优势**：职责单一且高内聚；单文件严格控制在 350 行以内，消除巨型单体文件膨胀隐患。
//! - **代价与局限**：方法间需共享 [`SegmentPath`] 音节切分上下文。

use super::super::AlgoEngine;
use crate::prism::{
    calibrate_habitual_alias_score, is_habitual_alias_key, PathOrigin, PrismEngine, PrismFuzzy,
    SegmentPath, STANDARD_INITIALS,
};
use crate::table::LENGTH_BOOST_MULTIPLIER;
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, SentenceDecodeParams, ViterbiDecoder};

/// 单条音节切分路径的容错状态与次优代价惩罚上下文参数对象
///
/// 用途：向词库检索与用户自造词检索两个子过程传递同一份路径判定结果，避免重复计算与参数膨胀。
struct PathMatchContext {
    /// 路径对应的词典查询键（如 "ng'hao"）
    key: String,
    /// 路径对应用户原始击键构成的展示与自适应拼音键（如 "en'hao"）
    raw_key: String,
    /// 路径所完全覆盖消费的原始击键字符总数
    consumed_len: usize,
    /// 路径来源性质（严格切分 / 确定性归一 / 概率性容错）
    origin: PathOrigin,
    /// 路径是否为相对最优路径的次优切分
    is_suboptimal: bool,
    /// 最优路径的音节数（用于剥离虚增的字长加成）
    best_syl_count: u32,
    /// 路径切分代价差值惩罚
    cost_penalty: u32,
}

/// 单条非容错切分路径的模糊音扩展匹配参数对象
///
/// 用途：承载模糊音扩展召回所需的切分路径、评估时间戳与该路径相对最优路径的次优判定依据。
struct FuzzyPathMatchParams<'a> {
    /// 待扩展的切分路径
    path: &'a SegmentPath,
    /// 候选评估基准时间戳
    eval_ts: u64,
    /// 最优路径的音节数（用于剥离虚增的字长加成）
    best_syl_count: u32,
    /// 路径切分代价差值惩罚
    cost_penalty: u32,
}

impl AlgoEngine {
    /// 聚合各维度候选词（专名实体、DAG 路径匹配、长句前缀合成、简拼降级与习惯加权）
    pub(crate) fn collect_all_raw_candidates(
        &mut self,
        paths: &[SegmentPath],
    ) -> Vec<AlgoCandidate> {
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
            let raw_key = path.to_raw_key_string();
            let consumed_len = path.consumed_raw_len();
            let cost_penalty = path.cost.saturating_sub(best_cost).saturating_mul(100);
            let is_suboptimal = path.origin == PathOrigin::Exact
                && (cost_penalty > 0 || (path.syllables.len() as u32) > best_syl_count);
            let path_ctx = PathMatchContext {
                key,
                raw_key,
                consumed_len,
                origin: path.origin,
                is_suboptimal,
                best_syl_count,
                cost_penalty,
            };

            // 词典精确及倒排检索（原地填充至复用容器）
            self.collect_lexicon_path_matches(&path_ctx, results);

            // 用户自造词检索（原地填充至复用容器）
            self.collect_user_path_matches(&path_ctx, eval_ts, results);

            // 模糊音等价查询键联合检索
            if self.config.fuzzy.has_active_rules() && path.origin == PathOrigin::Exact {
                self.collect_fuzzy_path_matches(
                    &FuzzyPathMatchParams {
                        path,
                        eval_ts,
                        best_syl_count,
                        cost_penalty,
                    },
                    results,
                );
            }
        }
    }

    /// 收集单条切分路径在词典精确及倒排索引上的候选，并按容错/次优状态修正层级与得分
    fn collect_lexicon_path_matches(
        &self,
        ctx: &PathMatchContext,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let start_lex = results.len();
        self.resources.table.lookup_into(&ctx.key, results);
        if is_habitual_alias_key(&ctx.key) {
            for m in &mut results[start_lex..] {
                let is_single = m.text.chars().count() == 1;
                m.tier = CandidateTier::ExactWord;
                m.score = calibrate_habitual_alias_score(m.score, is_single);
                m.consumed_len = Some(ctx.consumed_len);
                m.pinyin = ctx.raw_key.clone();
                m.comment = Some("[习惯音]".to_string());
            }
        } else if let Some(tier) = Self::typo_tier(ctx.origin) {
            for m in &mut results[start_lex..] {
                m.tier = tier;
                m.comment = Some(format!("[纠错: {}]", ctx.key));
            }
        } else if ctx.is_suboptimal {
            for m in &mut results[start_lex..] {
                m.score = Self::calibrate_lexicon_path_score(
                    m.score,
                    &m.text,
                    ctx.best_syl_count,
                    ctx.cost_penalty,
                );
            }
        }
    }

    /// 收集单条切分路径在用户自造词库上的候选，并按容错/次优状态修正层级与得分
    fn collect_user_path_matches(
        &self,
        ctx: &PathMatchContext,
        eval_ts: u64,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let start_usr = results.len();
        self.adaptive
            .model
            .lookup_user_words_into(&ctx.key, eval_ts, results);
        if is_habitual_alias_key(&ctx.key) {
            for m in &mut results[start_usr..] {
                m.tier = CandidateTier::UserHistory;
                m.consumed_len = Some(ctx.consumed_len);
                m.pinyin = ctx.raw_key.clone();
            }
        } else if let Some(tier) = Self::typo_tier(ctx.origin) {
            for m in &mut results[start_usr..] {
                m.tier = tier;
                m.comment = Some(format!("[纠错: {}]", ctx.key));
            }
        } else if ctx.is_suboptimal {
            for m in &mut results[start_usr..] {
                m.score = m.score.saturating_sub(ctx.cost_penalty).max(1);
            }
        }
    }

    /// 收集单条非容错切分路径在模糊音扩展下的词库与用户自造词候选
    fn collect_fuzzy_path_matches(
        &self,
        params: &FuzzyPathMatchParams<'_>,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let key_len = params.path.to_key_string().len();
        let is_suboptimal =
            params.cost_penalty > 0 || (params.path.syllables.len() as u32) > params.best_syl_count;

        for fuzzy_key in PrismEngine::expand_fuzzy_keys(params.path, &self.config.fuzzy) {
            let start_fz = results.len();
            self.resources.table.lookup_into(&fuzzy_key, results);
            for fm in &mut results[start_fz..] {
                fm.consumed_len = Some(key_len);
                let base = if is_suboptimal {
                    Self::calibrate_lexicon_path_score(
                        fm.score,
                        &fm.text,
                        params.best_syl_count,
                        params.cost_penalty,
                    )
                } else {
                    fm.score
                };
                fm.score = base.saturating_sub(50);
            }

            let start_fzu = results.len();
            self.adaptive
                .model
                .lookup_user_words_into(&fuzzy_key, params.eval_ts, results);
            for fum in &mut results[start_fzu..] {
                fum.consumed_len = Some(key_len);
                fum.score = fum
                    .score
                    .saturating_sub(params.cost_penalty)
                    .saturating_sub(50)
                    .max(1);
            }
        }
    }

    /// 将容错路径来源映射为候选排序层级；严格切分路径返回 `None` 表示沿用词库原生成词层级
    ///
    /// 确定性归一与概率性邻键猜测的可信度不对等，故分置于 [`CandidateTier`] 的两档：
    /// 归一候选压过两音节生造整句，邻键猜测仍保持次位。
    #[inline]
    fn typo_tier(origin: PathOrigin) -> Option<CandidateTier> {
        match origin {
            PathOrigin::RuleNormalized => Some(CandidateTier::RuleNormalization),
            PathOrigin::SpatialTypo => Some(CandidateTier::TypoCorrection),
            PathOrigin::Exact => None,
        }
    }

    /// 校准次优音节切分路径的词库候选得分：
    /// 剥离因零声母借音碎切导致字数增加而虚增的字长加成（LENGTH_BOOST_MULTIPLIER/字），并扣除路径切分代价差值惩罚。
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
            .saturating_mul(LENGTH_BOOST_MULTIPLIER);
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
            self.collect_viterbi_sentence_match(best_path, results);
        }

        // 前缀残存拼音补全与跨词短语合成
        if !best_path.remaining.is_empty() {
            self.collect_prefix_extension_matches(best_path, results);
            self.collect_prefix_phrase_matches(best_path, results);
            self.collect_prefix_top_k_matches(best_path, results);
        } else if best_path.origin == PathOrigin::Exact && best_path.syllables.len() >= 2 {
            // 即使末尾已构成完整音节（如 `niha` -> ni+ha、`zhonggu` -> zhong+gu、`beiji` -> bei+ji），
            // 仍将末音节视作未打完的前缀进行半成品补全召回，防止打字过程中候选突然消失。
            self.collect_complete_tail_extension_matches(best_path, results);
        }
    }

    /// 基于 Viterbi 整句动态规划合成最佳长句候选，并附加用于局部上屏的句首词段候选
    fn collect_viterbi_sentence_match(
        &self,
        best_path: &SegmentPath,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let (Some(mut sentence), heads) =
            ViterbiDecoder::decode_sentence_with_heads(&SentenceDecodeParams {
                syllables: &best_path.syllables,
                remaining: &best_path.remaining,
                table: &self.resources.table,
                prev_context: self.composition.last_commit_context(),
                enable_acronym: self.config.enable_acronym,
                user_model: Some(&self.adaptive.model),
                eval_ts: self
                    .adaptive
                    .model
                    .latest_timestamp()
                    .max(self.adaptive.last_timestamp),
            })
        else {
            return;
        };

        sentence.score = sentence
            .score
            .saturating_sub(best_path.cost.saturating_mul(100));
        let base_score = sentence.score;
        results.push(sentence);

        for (idx, head) in heads.into_iter().enumerate() {
            let consumed = best_path.consumed_prefix_raw_len(head.syllable_count);
            if consumed == 0 || consumed >= best_path.consumed_raw_len() {
                continue;
            }
            let mut head_cand = AlgoCandidate::new(
                head.text,
                head.pinyin,
                base_score.saturating_sub(idx as u32 + 1),
            )
            .with_tier(CandidateTier::SentenceViterbi);
            head_cand.consumed_len = Some(consumed);
            results.push(head_cand);
        }
    }

    /// 对已成音节但可能是未打完前缀的末音节执行半成品补全召回
    ///
    /// 优先级控制：标记为 `PrefixPartial`（或低于 `ExactWord` 的次级层级），
    /// 确保存在精确全拼整词（如 `zhongdi` -> 「种地」）时精确词稳居第一，不被高频补全词抢占。
    fn collect_complete_tail_extension_matches(
        &self,
        best_path: &SegmentPath,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let len = best_path.syllables.len();
        let prefix_syls = &best_path.syllables[..len - 1];
        let Some(tail_syl) = best_path.syllables.last() else {
            return;
        };
        let start = results.len();
        self.resources
            .table
            .lookup_prefix_extension_into(prefix_syls, tail_syl, results);
        for cand in &mut results[start..] {
            cand.tier = CandidateTier::PrefixPartial;
        }
    }

    /// 收集前缀残存拼音补全候选，并对模糊音扩展召回项统一降权
    fn collect_prefix_extension_matches(
        &self,
        best_path: &SegmentPath,
        results: &mut Vec<AlgoCandidate>,
    ) {
        self.resources.table.lookup_prefix_extension_into(
            &best_path.syllables,
            &best_path.remaining,
            results,
        );

        if self.config.fuzzy.has_active_rules() && !best_path.syllables.is_empty() {
            for fuzzy_syls in
                PrismFuzzy::expand_fuzzy_syllable_lists(&best_path.syllables, &self.config.fuzzy)
            {
                let start_fzp = results.len();
                self.resources.table.lookup_prefix_extension_into(
                    &fuzzy_syls,
                    &best_path.remaining,
                    results,
                );
                for fp in &mut results[start_fzp..] {
                    fp.score = fp.score.saturating_sub(50);
                }
            }
        }
    }

    /// 合成跨词边界短语候选并追加到候选列表
    fn collect_prefix_phrase_matches(
        &self,
        best_path: &SegmentPath,
        results: &mut Vec<AlgoCandidate>,
    ) {
        let mut phrase_matches =
            self.synthesize_prefix_phrases(&best_path.syllables, &best_path.remaining);
        results.append(&mut phrase_matches);
    }

    /// 收集首音节完整键对应的高频词候选，并统一标记为前缀部分命中层级
    fn collect_prefix_top_k_matches(
        &self,
        best_path: &SegmentPath,
        results: &mut Vec<AlgoCandidate>,
    ) {
        if best_path.syllables.is_empty() {
            return;
        }
        let base_key = best_path.syllables.join("'");
        let start_top = results.len();
        self.resources
            .table
            .lookup_top_k_into(&base_key, 32, results);
        for bm in &mut results[start_top..] {
            bm.tier = CandidateTier::PrefixPartial;
        }
    }

    /// 检索首字母缩写简拼、全拼混简拼与首音节单字降级备选候选
    fn collect_acronym_and_fallback_matches(
        &self,
        raw_input: &str,
        paths: &[SegmentPath],
        results: &mut Vec<AlgoCandidate>,
    ) {
        // 混合全拼+简拼检索（如 wanqky -> 完全可以，yongzwen -> 用中文）
        self.collect_mixed_jianpin_matches(paths, results);

        // 简拼检索（受用户配置 enable_acronym 控制）
        self.collect_acronym_matches(raw_input, results);

        // 首音节单字降级备选
        self.collect_first_syllable_fallback(paths, results);
    }

    /// 基于音节切分路径检索全拼与声母混合简拼候选（如 `wan'q'k'y` -> 「完全可以」）
    fn collect_mixed_jianpin_matches(
        &self,
        paths: &[SegmentPath],
        results: &mut Vec<AlgoCandidate>,
    ) {
        if !self.config.enable_acronym {
            return;
        }
        let mut queried_acronyms = Vec::with_capacity(4);
        for path in paths.iter().take(12) {
            if path.origin != PathOrigin::Exact {
                continue;
            }
            // 若末尾残存片段为合法声母，将其纳入混合简拼序列末尾
            let mut full_syls: Vec<String> = path.syllables.clone();
            let mut consumed_len = path.consumed_raw_len();
            if !path.remaining.is_empty() {
                if STANDARD_INITIALS.contains(&path.remaining.as_str()) {
                    consumed_len += path.remaining.len();
                    full_syls.push(path.remaining.clone());
                } else {
                    continue;
                }
            }

            let syl_count = full_syls.len();
            if !(2..=6).contains(&syl_count) {
                continue;
            }
            let initial_count = full_syls
                .iter()
                .filter(|s| STANDARD_INITIALS.contains(&s.as_str()))
                .count();
            // 仅当同时包含完整音节与声母简拼时触发混合匹配
            if initial_count == 0 || initial_count == syl_count {
                continue;
            }
            let acronym_key: String = full_syls.iter().filter_map(|s| s.chars().next()).collect();
            if acronym_key.len() != syl_count || queried_acronyms.contains(&acronym_key) {
                continue;
            }
            queried_acronyms.push(acronym_key.clone());

            let full_count = (syl_count - initial_count) as u32;
            for mut cand in self.resources.table.lookup_acronym_top_k(&acronym_key, 48) {
                if !self.matches_mixed_jianpin_text(&full_syls, &cand.text) {
                    continue;
                }
                cand.tier = CandidateTier::Acronym;
                cand.score = cand.score.saturating_add(full_count.saturating_mul(15_000));
                cand.consumed_len = Some(consumed_len);
                cand.comment = Some("[混拼]".to_string());
                results.push(cand);
            }
        }
    }

    /// 校验候选词各位置汉字是否与混合全拼+简拼路径中的完整音节读音相符
    fn matches_mixed_jianpin_text(&self, path_syls: &[String], cand_text: &str) -> bool {
        let chars: Vec<char> = cand_text.chars().collect();
        if chars.len() != path_syls.len() {
            return false;
        }
        for (idx, path_syl) in path_syls.iter().enumerate() {
            if STANDARD_INITIALS.contains(&path_syl.as_str()) {
                continue;
            }
            let Some(entries) = self.resources.table.get_raw_entries(path_syl) else {
                return false;
            };
            let target_ch = chars[idx];
            let mut target_buf = [0u8; 4];
            let target_str = target_ch.encode_utf8(&mut target_buf);
            if !entries.iter().any(|(t, _)| t == target_str) {
                return false;
            }
        }
        true
    }

    /// 检索首字母缩写简拼候选，并对含分隔符输入标注完整消耗长度
    fn collect_acronym_matches(&self, raw_input: &str, results: &mut Vec<AlgoCandidate>) {
        if !self.config.enable_acronym {
            return;
        }
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
    }

    /// 检索首音节单字降级备选候选（含模糊音扩展召回与首字母兜底条目）
    fn collect_first_syllable_fallback(
        &self,
        paths: &[SegmentPath],
        results: &mut Vec<AlgoCandidate>,
    ) {
        let Some(first_path) = paths.first() else {
            return;
        };
        if first_path.syllables.is_empty() {
            return;
        }
        let first_syl = &first_path.syllables[0];
        let mut first_matches = self.resources.table.lookup_top_k(first_syl, 32);
        if self.config.fuzzy.has_active_rules() {
            for alt_syl in PrismEngine::expand_fuzzy_syllables(first_syl, &self.config.fuzzy) {
                let mut alt_matches = self.resources.table.lookup_top_k(&alt_syl, 32);
                for am in &mut alt_matches {
                    am.consumed_len = Some(first_syl.len());
                    am.score = am.score.saturating_sub(50);
                }
                first_matches.append(&mut alt_matches);
            }
        }
        if first_matches.is_empty() {
            if let Some(initial_entries) = self.resources.table.get_initial_raw_entries(first_syl) {
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

    /// 应用自适应词频提权与上屏历史二元语言模型亲和度加权
    fn apply_user_and_context_boost(&self, candidates: &mut [AlgoCandidate], eval_ts: u64) {
        let prev_context_info = self
            .composition
            .last_commit_context()
            .map(|prev| (prev, self.resources.table.get_word_id(prev)));

        for c in candidates {
            if self.config.dynamic_adjust_freq && c.tier != CandidateTier::UserHistory {
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
}
