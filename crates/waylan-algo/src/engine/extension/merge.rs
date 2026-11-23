//! 算法引擎首页扩展候选与分段短词混排注入
//!
//! # 设计原理
//! - **实现初衷**：负责将扩展候选、前缀音节高频词与实时 Emoji 推荐安全编排入主候选列表。
//! - **核心优势**：确立严格的语义层级优先级，杜绝扩展项粗暴覆盖击中词，同时为长句分段选词提供前缀分段候选支持。
//! - **代价与局限**：前缀分段提取依赖最优解码拼音序列按音节切分。

use std::collections::HashSet;

use super::pipeline::ExtensionPipeline;
use super::providers::ExtensionContext;
use crate::engine::AlgoEngine;
use crate::engine::emoji::suggest_emoji_candidate;
use crate::types::{AlgoCandidate, CandidateTier};

impl AlgoEngine {
    /// 注入首页扩展候选（前缀分段短词、动态日期时间、英文联想、自定义短语与实时 Emoji）
    pub(crate) fn inject_page_zero_extensions(
        &self,
        raw_input: &str,
        best_units: &[&str],
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        // 1. 前缀高频短词混排注入（支持长句分段确认 Partial Commit）
        self.inject_prefix_segment_candidates(raw_input, best_units, candidates);

        // 2. 静态分派零分配收集扩展项
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

        // 3. 按照确定的语义层级编排扩展候选
        self.merge_extensions_by_hierarchy(&extensions, candidates);

        // 4. 实时 Emoji 推荐（在第三候选位插入匹配表情，受配置开关控制）
        if self.config.enable_emoji_suggest
            && candidates.len() >= 2
            && let Some(emoji_cand) = suggest_emoji_candidate(raw_input, candidates)
        {
            let insert_pos = 2.min(candidates.len());
            candidates.insert(insert_pos, emoji_cand);
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
        raw_input: &str,
        best_units: &[&str],
    ) -> Option<AlgoCandidate> {
        if best_units.len() < 4 {
            return None;
        }
        let key_3 = best_units[..3].join("'");
        let mut m = self
            .resources
            .table
            .lookup_top_k(&key_3, 1)
            .into_iter()
            .next()?;
        if m.pinyin == key_3 {
            m.consumed_len = Some(Self::calculate_consumed_len(raw_input, &key_3, best_units));
            Some(self.format_prefix_candidate(m))
        } else {
            None
        }
    }

    /// 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，回退查简拼召回 Top-2）
    fn extract_prefix_2_candidates(
        &self,
        raw_input: &str,
        best_units: &[&str],
    ) -> Vec<AlgoCandidate> {
        let mut prefix_items = Vec::new();
        if best_units.len() < 2 {
            return prefix_items;
        }
        let key_2 = best_units[..2].join("'");
        let mut found_exact_2 = false;
        if let Some(mut m) = self
            .resources
            .table
            .lookup_top_k(&key_2, 1)
            .into_iter()
            .next()
            && m.pinyin == key_2
        {
            m.consumed_len = Some(Self::calculate_consumed_len(raw_input, &key_2, best_units));
            prefix_items.push(self.format_prefix_candidate(m));
            found_exact_2 = true;
        }
        if !found_exact_2 && self.config.enable_acronym {
            let acronym_2 = best_units[..2].concat();
            for mut cand in self.resources.table.lookup_acronym_top_k(&acronym_2, 2) {
                cand.consumed_len =
                    Some(Self::calculate_consumed_len(raw_input, &key_2, best_units));
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
        let keep_count = full_len_count.clamp(3, 3).min(candidates.len());

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
        raw_input: &str,
        best_units: &[&str],
        candidates: &mut Vec<AlgoCandidate>,
    ) {
        if best_units.len() < 3 {
            return;
        }

        let mut prefix_items = Vec::new();
        if let Some(cand_3) = self.extract_prefix_3_candidate(raw_input, best_units) {
            prefix_items.push(cand_3);
        }
        prefix_items.extend(self.extract_prefix_2_candidates(raw_input, best_units));

        if !prefix_items.is_empty() {
            Self::blend_prefix_candidates(candidates, prefix_items, best_units.len());
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
