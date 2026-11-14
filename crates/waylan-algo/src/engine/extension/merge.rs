//! 算法引擎首页扩展候选与分段短词混排注入
//!
//! # 设计原理
//! - **实现初衷**：负责将扩展候选、空间纠错候选与前缀音节高频词安全编排入主候选列表。
//! - **核心优势**：确立严格的语义层级优先级，杜绝扩展项粗暴覆盖击中词，同时为长句分段选词提供前缀分段候选支持。

use std::collections::HashSet;

use super::pipeline::ExtensionPipeline;
use super::providers::ExtensionContext;
use crate::engine::emoji::suggest_emoji_candidate;
use crate::engine::typo::SpatialTypoCorrector;
use crate::engine::AlgoEngine;
use crate::prism::SegmentPath;
use crate::types::{AlgoCandidate, CandidateTier};

/// 纠错候选插回主候选列表的参数对象
///
/// 承载纠错候选插回所需的消费长度、碎切容错标记与待插入的纠错候选集合。
struct TypoCandidateInsertParams {
    /// 原始输入串长度（作为纠错候选的消费长度）
    raw_input_len: usize,
    /// 当前最优路径是否为碎切容错回退路径（决定纠错项插入位置）
    is_broken_typo: bool,
    /// 待插入的纠错候选集合
    merged_typos: Vec<AlgoCandidate>,
}

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

        // 3. 静态分派零分配收集扩展项
        let ctx = ExtensionContext {
            raw_input,
            existing_candidates: candidates,
            opencc: self
                .config
                .traditional_chinese
                .then(|| self.resources.opencc_dict()),
        };
        let mut extensions = Vec::with_capacity(8);
        ExtensionPipeline::collect_default_extensions(&self.resources, &ctx, &mut extensions);

        // 4. 按照确定的语义层级编排扩展候选
        self.merge_extensions_by_hierarchy(&extensions, candidates);

        // 5. 实时 Emoji 推荐（在第三候选位插入匹配表情，受配置开关控制）
        if self.config.enable_emoji_suggest && candidates.len() >= 2 {
            if let Some(emoji_cand) = suggest_emoji_candidate(raw_input, candidates) {
                let insert_pos = 2.min(candidates.len());
                candidates.insert(insert_pos, emoji_cand);
            }
        }
    }

    /// 在无完整合法成词命中时注入物理空间键盘按键纠错候选
    fn inject_typo_candidates_if_needed(
        &self,
        raw_input: &str,
        is_broken_typo: bool,
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        if !self.config.typo_rules.enable {
            return;
        }
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

        self.merge_spatial_typo_candidates(raw_input, &mut merged_typos);

        self.insert_typo_candidates(
            TypoCandidateInsertParams {
                raw_input_len: raw_input.len(),
                is_broken_typo,
                merged_typos,
            },
            candidates,
        );
    }

    /// 合并物理空间键盘纠错候选到纠错候选集合，并执行简繁归一与文本去重
    fn merge_spatial_typo_candidates(
        &self,
        raw_input: &str,
        merged_typos: &mut Vec<AlgoCandidate>,
    ) {
        for mut spatial_cand in SpatialTypoCorrector::generate_candidates(
            raw_input,
            &self.resources.table,
            &self.config.fuzzy,
            &self.config.typo_rules,
        ) {
            if self.config.traditional_chinese {
                spatial_cand.text = self
                    .resources
                    .opencc_dict()
                    .convert_text(&spatial_cand.text);
            }
            if !merged_typos.iter().any(|c| c.text == spatial_cand.text) {
                merged_typos.push(spatial_cand);
            }
        }
    }

    /// 将纠错候选插回候选列表头部区域，保证纠错项优先于普通候选可见
    fn insert_typo_candidates(
        &self,
        params: TypoCandidateInsertParams,
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        let TypoCandidateInsertParams {
            raw_input_len,
            is_broken_typo,
            merged_typos,
        } = params;

        let mut inserted = 0usize;
        for mut typo_cand in merged_typos.into_iter().take(2) {
            // 若已有更可信层级（精确词/确定性归一/整句等）的同文本候选，保留原候选不降级覆盖
            if candidates
                .iter()
                .any(|c| c.text == typo_cand.text && c.tier > CandidateTier::TypoCorrection)
            {
                continue;
            }
            typo_cand.consumed_len = Some(raw_input_len);
            candidates.retain(|c| c.text != typo_cand.text);
            let target_pos = if is_broken_typo || candidates.is_empty() {
                inserted.min(candidates.len())
            } else {
                (1 + inserted).min(candidates.len())
            };
            candidates.insert(target_pos, typo_cand);
            inserted += 1;
        }
    }

    /// 按照确定的语义层级编排扩展候选，杜绝多个扩展项暴力覆盖挤出中文最优词
    fn merge_extensions_by_hierarchy(
        &self,
        extensions: &[AlgoCandidate],
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        for ext in extensions {
            if ext.tier == CandidateTier::ProtectedEntity {
                candidates.retain(|c| c.text != ext.text);
                candidates.insert(0, ext.clone());
            } else if ext.is_expandable {
                candidates.retain(|c| c.text != ext.text);
                let insert_pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(insert_pos, ext.clone());
            } else {
                candidates.retain(|c| c.text != ext.text);
                let insert_pos = if candidates.len() <= 1 {
                    candidates.len()
                } else {
                    2.min(candidates.len())
                };
                candidates.insert(insert_pos, ext.clone());
            }
        }
    }

    /// 提取前 3 音节全拼高频词（如 wo'men'de -> 我们的）
    fn extract_prefix_3_candidate(
        &self,
        best_path: &SegmentPath,
        total_syls: usize,
    ) -> Option<AlgoCandidate> {
        if total_syls < 4 || best_path.syllables.len() < 3 {
            return None;
        }
        let key_3 = best_path.syllables[..3].join("'");
        let mut m = self
            .resources
            .table
            .lookup_top_k(&key_3, 1)
            .into_iter()
            .next()?;
        if m.pinyin == key_3 {
            m.consumed_len = Some(Self::calculate_consumed_len(
                self.composition.as_str(),
                &key_3,
                &best_path.syllables,
            ));
            Some(self.format_prefix_candidate(m))
        } else {
            None
        }
    }

    /// 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，回退查简拼召回 Top-2）
    fn extract_prefix_2_candidates(&self, best_path: &SegmentPath) -> Vec<AlgoCandidate> {
        let mut prefix_items = Vec::new();
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
        if !found_exact_2 && self.config.enable_acronym {
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
        prefix_items
    }

    /// 将前缀候选词混排注入到全长候选词之后并执行文本去重
    fn blend_prefix_candidates(
        candidates: &mut Vec<AlgoCandidate>,
        prefix_items: Vec<AlgoCandidate>,
        total_syls: usize,
    ) {
        let full_len_count = candidates
            .iter()
            .take_while(|c| {
                c.tier != CandidateTier::PrefixPartial && c.text.chars().count() >= total_syls
            })
            .count();
        let keep_count = full_len_count.clamp(2, 3).min(candidates.len());

        let mut injected = Vec::with_capacity(candidates.len() + prefix_items.len());
        injected.extend(candidates.drain(..keep_count));
        injected.extend(prefix_items);
        injected.append(candidates);

        let mut seen = HashSet::with_capacity(injected.len());
        let mut keep = Vec::with_capacity(injected.len());
        for item in &injected {
            keep.push(seen.insert(item.text.as_str()));
        }
        drop(seen);

        let mut keep_iter = keep.into_iter();
        injected.retain(|_| keep_iter.next().unwrap_or(false));

        *candidates = injected;
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
        if let Some(cand_3) = self.extract_prefix_3_candidate(best_path, total_syls) {
            prefix_items.push(cand_3);
        }
        prefix_items.extend(self.extract_prefix_2_candidates(best_path));

        if !prefix_items.is_empty() {
            Self::blend_prefix_candidates(candidates, prefix_items, total_syls);
        }
    }

    /// 归一化前缀分段候选词的简繁文本、拼音注释与层级标签
    fn format_prefix_candidate(&self, mut cand: AlgoCandidate) -> AlgoCandidate {
        if self.config.traditional_chinese {
            cand.text = self.resources.opencc_dict().convert_text(&cand.text);
        }
        if self.config.show_pinyin_comment && cand.comment.is_none() && !cand.pinyin.is_empty() {
            cand.comment = Some(format!("[{}]", cand.pinyin));
        }
        cand.tier = CandidateTier::PrefixPartial;
        cand
    }
}
