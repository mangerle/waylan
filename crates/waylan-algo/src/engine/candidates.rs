use super::typo::SpatialTypoCorrector;
use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::evaluator::DateTimeEvaluator;
use crate::pipeline::CandidatePipeline;
use crate::prism::PrismEngine;
use crate::types::{AlgoCandidate, CandidateTier};
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

impl AlgoEngine {
    pub(crate) fn recompute_candidates(&mut self) {
        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;

        let raw_input = self.composition.as_str();
        if raw_input.is_empty() {
            self.current_candidates.clear();
            self.cached_preedit.clear();
            return;
        }

        // 1. 尝试前缀模式分流 (V计算器、U拆字、I特殊符号)
        if let Some(dispatched) = ModeDispatcher::try_dispatch(raw_input, &self.config) {
            self.cached_preedit = raw_input.to_string();
            self.current_candidates = CandidatePipeline::process_with_opencc(
                dispatched,
                &self.config,
                self.page_index,
                Some(&self.opencc_dict),
            );
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

            // 模糊音等价查询键联合检索（将候选词 pinyin 映射回原始输入切分键以保证选词消费长度一致）
            if self.config.fuzzy.has_active_rules() && !path.is_typo {
                for fuzzy_key in PrismEngine::expand_fuzzy_keys(path, &self.config.fuzzy) {
                    let mut fuzzy_matches = self.table.lookup(&fuzzy_key);
                    for fm in &mut fuzzy_matches {
                        fm.pinyin = key.clone();
                        fm.score = fm.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_matches);

                    let mut fuzzy_user_matches =
                        self.user_model.lookup_user_words(&fuzzy_key, eval_ts);
                    for fum in &mut fuzzy_user_matches {
                        fum.pinyin = key.clone();
                        fum.score = fum.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_user_matches);
                }
            }
        }

        // 2.1 Viterbi 长句动态规划解码（遵循 Best Path Only 原则）：
        // 仅针对 Prism 中代价最低的最优主路径执行整句求解，彻底杜绝次级碎切路径产生劣质假整句；
        // 同时传入末尾残存拼音 remaining 与上屏历史上下文，支持跨句二元转移预测
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
                    // 融合切分路径拓扑代价值：扣除对应的拓扑惩罚
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
                    for fuzzy_syls in crate::prism_fuzzy::PrismFuzzy::expand_fuzzy_syllable_lists(
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
                    let mut base_matches = self.table.lookup(&base_key);
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
        let mut acronym_matches = self.table.lookup_acronym(&acronym_query);
        if raw_input.contains('\'') {
            for am in &mut acronym_matches {
                am.pinyin = raw_input.to_string();
            }
        }
        raw_candidates.append(&mut acronym_matches);

        // 2.3 若多音节切分未能命中整词，将首段已完成音节或首声母单字作为保真降级备选（如 "ni" -> "你", "r" -> "人"）
        let clean_raw_len = raw_input.chars().filter(|c| *c != '\'').count();
        let has_full_exact_match = raw_candidates.iter().any(|c| {
            c.tier == CandidateTier::ExactWord
                && c.pinyin.chars().filter(|ch| *ch != '\'').count() == clean_raw_len
        });
        if !has_full_exact_match {
            if let Some(first_path) = paths.first() {
                if !first_path.syllables.is_empty() {
                    let first_syl = &first_path.syllables[0];
                    let mut first_matches = self.table.lookup(first_syl);
                    if self.config.fuzzy.has_active_rules() {
                        for alt_syl in
                            PrismEngine::expand_fuzzy_syllables(first_syl, &self.config.fuzzy)
                        {
                            let mut alt_matches = self.table.lookup(&alt_syl);
                            for am in &mut alt_matches {
                                am.pinyin = first_syl.clone();
                                am.score = am.score.saturating_sub(50);
                            }
                            first_matches.append(&mut alt_matches);
                        }
                    }
                    if first_matches.is_empty() {
                        if let Some(initial_entries) = self.table.get_initial_raw_entries(first_syl)
                        {
                            for (text, score) in initial_entries {
                                first_matches.push(
                                    AlgoCandidate::new(text, first_syl.clone(), score)
                                        .with_tier(CandidateTier::PrefixPartial),
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
        }

        // 3. 动态应用用户习惯提权与上屏历史上下文亲和度加权
        for c in &mut raw_candidates {
            self.user_model.apply_boost(c, eval_ts);
            if let Some(ref prev) = self.last_commit_context {
                let affinity = self
                    .table
                    .get_bigram_affinity(prev, &c.text)
                    .unwrap_or_else(|| {
                        BigramLanguageModel::estimate_bigram_affinity(prev, &c.text)
                    });
                if affinity > 0 {
                    // 将语言模型亲和度映射为候选词得分增益（大幅提高首位命中率）
                    c.score = c.score.saturating_add((affinity as u32).min(50_000));
                }
            }
        }

        // 4. 送入后处理流水线（去重、简繁、字符集、注释、分页）
        let mut candidates = CandidatePipeline::process_with_opencc(
            raw_candidates,
            &self.config,
            self.page_index,
            Some(&self.opencc_dict),
        );

        if self.page_index == 0 {
            // 5. 状态机感知与容错门禁控制：
            // 若当前输入处于正常的合法前缀打字态（已成功检索出 ExactWord 级别的完整成词或前缀展开短语），
            // 绝对免受激进物理空间容错打扰；仅在无任何合法成词/前缀展开命中时（如输入 nihso, cehng），才激活次位容错。
            let has_exact_or_prefix_words = candidates.iter().any(|c| {
                c.tier == CandidateTier::ExactWord || c.tier == CandidateTier::ProtectedEntity
            });

            if !has_exact_or_prefix_words {
                let typo_cands = SpatialTypoCorrector::generate_candidates(raw_input, &self.table);
                for (i, mut typo_cand) in typo_cands.into_iter().take(2).enumerate() {
                    if self.config.traditional_chinese {
                        typo_cand.text = self.opencc_dict.convert_text(&typo_cand.text);
                    }
                    // 若列表中已有同名候选但非容错词（例如 Viterbi 产生的不完全前缀预测），先将其剔除
                    candidates.retain(|c| c.text != typo_cand.text);
                    let target_pos = if candidates.is_empty() {
                        0
                    } else {
                        (1 + i).min(candidates.len())
                    };
                    candidates.insert(target_pos, typo_cand);
                }
            }

            // 6. 动态扩展功能（日期、时间、干支等）注入：
            // 若命中动态日期/时间指令，将动态功能项插入到第 2 位（仅次于首选的常规成词）
            if let Some(mut dynamic_cand) = DateTimeEvaluator::try_evaluate(raw_input) {
                if self.config.traditional_chinese {
                    dynamic_cand.text = self.opencc_dict.convert_text(&dynamic_cand.text);
                    for sub in &mut dynamic_cand.sub_candidates {
                        *sub = self.opencc_dict.convert_text(sub);
                    }
                }
                let insert_pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(insert_pos, dynamic_cand);
            }

            // 7. 英文单词联想注入（如输入 rust、github、apple 等在次位提供英文候选，若已存在相同专名则避免重复）
            let eng_cands = self.english_suggester.lookup(raw_input);
            for eng in eng_cands.into_iter().rev() {
                if !candidates
                    .iter()
                    .any(|c| c.text.eq_ignore_ascii_case(&eng.text))
                {
                    let pos = if candidates.is_empty() { 0 } else { 1 };
                    candidates.insert(pos, eng);
                }
            }

            // 8. 符号与表情联想注入（如输入 dianzan、daxiao 等在次位提供符号候选）
            let sym_cands = self.symbol_suggester.lookup(raw_input);
            for sym in sym_cands.into_iter().rev() {
                let pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(pos, sym);
            }

            // 9. 用户自定义短语注入（如输入 mymail、addr 等置顶首位）
            let phrase_cands = self.custom_phrases.lookup(raw_input);
            for phrase in phrase_cands.into_iter().rev() {
                candidates.insert(0, phrase);
            }

            // 10. 前缀高频短词混排注入 (Prefix Segment Candidate Injection)：
            // 当处于首未翻页状态 (page_index == 0) 且有效拼音达到 3 个及以上音节时，
            // 自动提取前 3 个音节（如 wo'men'de -> 我们的）与前 2 个音节（如 wo'men -> 我们，或简拼 r'g -> 如果/入股）的高频成词，
            // 紧随全长成词之后注入，支持分段确认 (Partial Commit)
            if let Some(best_path) = paths.first() {
                let total_syls =
                    best_path.syllables.len() + if best_path.remaining.is_empty() { 0 } else { 1 };
                if total_syls >= 3 && best_path.syllables.len() >= 2 {
                    let mut prefix_items = Vec::new();

                    // 11.1 若总音节 >= 4 且已完成音节 >= 3，优先提取前 3 音节全拼高频词（如 wo'men'de -> 我们的）
                    if total_syls >= 4 && best_path.syllables.len() >= 3 {
                        let key_3 = best_path.syllables[..3].join("'");
                        let matches_3 = self.table.lookup(&key_3);
                        for m in matches_3 {
                            if m.pinyin == key_3 {
                                let mut cand = m.clone();
                                if self.config.traditional_chinese {
                                    cand.text = self.opencc_dict.convert_text(&cand.text);
                                }
                                if self.config.show_pinyin_comment
                                    && cand.comment.is_none()
                                    && !cand.pinyin.is_empty()
                                {
                                    cand.comment = Some(format!("[{}]", cand.pinyin));
                                }
                                cand.tier = CandidateTier::PrefixPartial;
                                prefix_items.push(cand);
                                break;
                            }
                        }
                    }

                    // 11.2 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，则回退查简拼召回 Top-2 二字高频词如 如果、入股）
                    let key_2 = best_path.syllables[..2].join("'");
                    let matches_2 = self.table.lookup(&key_2);
                    let mut found_exact_2 = false;
                    for m in matches_2 {
                        if m.pinyin == key_2 {
                            let mut cand = m.clone();
                            if self.config.traditional_chinese {
                                cand.text = self.opencc_dict.convert_text(&cand.text);
                            }
                            if self.config.show_pinyin_comment
                                && cand.comment.is_none()
                                && !cand.pinyin.is_empty()
                            {
                                cand.comment = Some(format!("[{}]", cand.pinyin));
                            }
                            cand.tier = CandidateTier::PrefixPartial;
                            prefix_items.push(cand);
                            found_exact_2 = true;
                            break;
                        }
                    }
                    if !found_exact_2 {
                        let acronym_2 = best_path.syllables[..2].concat();
                        let acro_matches_2 = self.table.lookup_acronym(&acronym_2);
                        for m in acro_matches_2.into_iter().take(2) {
                            let mut cand = m;
                            if self.config.traditional_chinese {
                                cand.text = self.opencc_dict.convert_text(&cand.text);
                            }
                            cand.pinyin = key_2.clone();
                            if self.config.show_pinyin_comment && cand.comment.is_none() {
                                cand.comment = Some(format!("[{}]", cand.pinyin));
                            }
                            cand.tier = CandidateTier::PrefixPartial;
                            prefix_items.push(cand);
                        }
                    }

                    // 11.3 混排注入：动态保留前序全长成词（2 至 3 个），随后注入前缀短词并保持去重
                    if !prefix_items.is_empty() {
                        let mut injected =
                            Vec::with_capacity(candidates.len() + prefix_items.len());
                        let mut seen = std::collections::HashSet::new();

                        // 统计排在列表头部的全长匹配成词数量（如 rgzn 命中的 人工智能、人过中年、弱冠之年）
                        let full_len_count = candidates
                            .iter()
                            .take_while(|c| {
                                c.tier != CandidateTier::PrefixPartial
                                    && c.text.chars().count() >= total_syls
                            })
                            .count();
                        let keep_count = full_len_count.clamp(2, 3).min(candidates.len());

                        for c in candidates.drain(..keep_count) {
                            if seen.insert(c.text.clone()) {
                                injected.push(c);
                            }
                        }

                        // 注入前缀短词
                        for pi in prefix_items {
                            if seen.insert(pi.text.clone()) {
                                injected.push(pi);
                            }
                        }

                        // 拼接剩余备选词
                        for c in candidates.drain(..) {
                            if seen.insert(c.text.clone()) {
                                injected.push(c);
                            }
                        }

                        candidates = injected;
                    }
                }
            }
        }

        // 统一强制截断至单页容量上限，防止扩展项注入导致单页候选溢出
        candidates.truncate(self.config.page_size.max(1));

        self.current_candidates = candidates;
    }
    fn synthesize_prefix_phrases(
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
            // 若倒数音节不是合法的完整标准音节（即为单字母声母，如 g, w, r, z），吸收进声母链
            if !crate::syllables::is_valid_syllable(prev_syl) {
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
            // k = 1 场景：单声母未完成态，与前驱最后 1 个完整音节结合展开全拼词 (如 shi + j* -> 时间/世界)
            if split_idx == 0 {
                return Vec::new();
            }
            let last_syl_idx = split_idx - 1;
            let tail_syl = &syllables[last_syl_idx..split_idx];
            self.table.lookup_prefix_extension(tail_syl, remaining)
        } else {
            // k >= 2 场景：连续声母简拼链，直接在简拼索引树上遍历 (如 gj -> 感觉, gwy -> 国务院, rgzn -> 人工智能)
            let acronym_key = initial_chain.concat();
            self.table.lookup_acronym(&acronym_key)
        };

        if tail_cands.is_empty() {
            return Vec::new();
        }

        // 3. 确定真正的前驱短语音节切片 (effective_head_syls)
        let effective_head_syls = if k == 1 {
            // k = 1 时，最后 1 个完整音节已经被 tail 消耗，前驱为 split_idx - 1
            if split_idx == 0 {
                return Vec::new();
            }
            &syllables[..split_idx - 1]
        } else {
            // k >= 2 时，所有声母都被 tail 消耗，完整音节全部留作前驱
            head_syls
        };

        if effective_head_syls.is_empty() {
            return tail_cands;
        }

        // 4. 求解前驱段文本与得分 (如 wo'men'gong'tong'de -> 我们共同的)
        let head_key = effective_head_syls.join("'");
        let mut head_options: Vec<(String, String, u32)> = Vec::new();

        if let Some(sentence) = ViterbiDecoder::decode_sentence_with_context(
            effective_head_syls,
            "",
            &self.table,
            self.last_commit_context.as_deref(),
        ) {
            head_options.push((sentence.text, head_key.clone(), sentence.score));
        }

        let exact_matches = self.table.lookup(&head_key);
        if let Some(best_exact) = exact_matches.first() {
            if !head_options.iter().any(|(t, _, _)| t == &best_exact.text) {
                head_options.push((best_exact.text.clone(), head_key.clone(), best_exact.score));
            }
        }

        // 5. 跨词短语笛卡尔积合成与打分
        let mut results = Vec::new();
        for (head_text, head_pinyin, head_score) in &head_options {
            let head_last_char = head_text
                .chars()
                .last()
                .map(|c| c.to_string())
                .unwrap_or_default();
            for tail in tail_cands.iter().take(5) {
                let phrase_text = format!("{}{}", head_text, tail.text);
                let phrase_pinyin = format!("{}'{}", head_pinyin, tail.pinyin);

                let char_len = phrase_text.chars().count() as u32;
                let length_boost = char_len.saturating_sub(1).saturating_mul(600_000);

                // 获取二元搭配亲和度奖励（优先全词转移，次选显式高阶语义规则，再回退末字助词泛化转移）
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
