//! 首页扩展候选词流水线与动态注入
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供高度可扩展的候选词扩展能力，将自定义短语、动态时间卡片、
//!   英文单词联想、特殊符号表情等多样化扩展项从主算法解码中解耦，彻底遵循开闭原则 (OCP)。
//! - **核心优势**：插件化管道模式（Pipeline Pattern），新增扩展项仅需实现 [`CandidateExtensionProvider`]
//!   特型并在管道中注册，无需修改任何现有解码逻辑。
//! - **代价与局限**：扩展项在流水线收集后需要根据语义优先级进行统一编排，插入操作涉及少量切片重排。

use std::collections::HashSet;

use super::resources::LexiconResources;
use super::typo::SpatialTypoCorrector;
use super::AlgoEngine;
use crate::custom_phrase::{
    CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager,
};
use crate::evaluator::DateTimeEvaluator;
use crate::opencc::OpenCcDict;
use crate::prism::SegmentPath;
use crate::types::{AlgoCandidate, CandidateTier};

/// 候选扩展上下文
pub struct ExtensionContext<'a> {
    /// 原始按键输入文本
    pub raw_input: &'a str,
    /// 现有候选词列表引用（用于去重与互斥门禁检测）
    pub existing_candidates: &'a [AlgoCandidate],
    /// 是否开启繁体中文转换
    pub traditional_chinese: bool,
    /// 简繁转换词典
    pub opencc: &'a OpenCcDict,
}

/// 候选扩展项提供者特型 (Candidate Extension Provider)
///
/// 遵循开闭原则 (OCP)，支持动态插拔不同类型的候选词扩展源。
pub trait CandidateExtensionProvider {
    /// 根据当前上下文生成扩展候选词并追加至输出缓冲区（推荐零分配实现）
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        out.extend(self.provide(ctx));
    }

    /// 根据当前上下文生成扩展候选词（默认实现自动委托至 append_candidates）
    fn provide(&self, ctx: &ExtensionContext<'_>) -> Vec<AlgoCandidate> {
        let mut results = Vec::new();
        self.append_candidates(ctx, &mut results);
        results
    }
}

/// 用户自定义短语提供者（最高优先级，标记为 ProtectedEntity 置顶）
pub struct CustomPhraseProvider<'a> {
    pub custom_phrases: &'a CustomPhraseManager,
}

impl<'a> CandidateExtensionProvider for CustomPhraseProvider<'a> {
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        let phrase_cands = self.custom_phrases.lookup(ctx.raw_input);
        for mut phrase in phrase_cands {
            if ctx.traditional_chinese {
                phrase.text = ctx.opencc.convert_text(&phrase.text);
            }
            phrase.tier = CandidateTier::ProtectedEntity;
            phrase.consumed_len = Some(ctx.raw_input.len());
            out.push(phrase);
        }
    }
}

/// 动态扩展功能提供者（日期、时间、干支等强相关功能卡片）
pub struct DateTimeProvider;

impl CandidateExtensionProvider for DateTimeProvider {
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        if let Some(mut dynamic_cand) = DateTimeEvaluator::try_evaluate(ctx.raw_input) {
            if ctx.traditional_chinese {
                dynamic_cand.text = ctx.opencc.convert_text(&dynamic_cand.text);
                for sub in &mut dynamic_cand.sub_candidates {
                    *sub = ctx.opencc.convert_text(sub);
                }
            }
            dynamic_cand.consumed_len = Some(ctx.raw_input.len());
            out.push(dynamic_cand);
        }
    }
}

/// 英文单词联想建议提供者（次要辅助建议，至多取 1 项避免污染候选列表）
pub struct EnglishSuggesterProvider<'a> {
    pub english_suggester: &'a EnglishSuggestionManager,
}

impl<'a> CandidateExtensionProvider for EnglishSuggesterProvider<'a> {
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        let eng_cands = self.english_suggester.lookup(ctx.raw_input);
        for mut eng in eng_cands.into_iter().take(1) {
            if !ctx
                .existing_candidates
                .iter()
                .any(|c| c.text.eq_ignore_ascii_case(&eng.text))
            {
                eng.consumed_len = Some(ctx.raw_input.len());
                out.push(eng);
            }
        }
    }
}

/// 符号与表情联想建议提供者（次要辅助建议，至多取 1 项）
pub struct SymbolSuggesterProvider<'a> {
    pub symbol_suggester: &'a SymbolSuggestionManager,
}

impl<'a> CandidateExtensionProvider for SymbolSuggesterProvider<'a> {
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        let sym_cands = self.symbol_suggester.lookup(ctx.raw_input);
        for mut sym in sym_cands.into_iter().take(1) {
            if !ctx.existing_candidates.iter().any(|c| c.text == sym.text) {
                sym.consumed_len = Some(ctx.raw_input.len());
                out.push(sym);
            }
        }
    }
}

/// 扩展候选词聚合流水线 (Extension Pipeline)
pub struct ExtensionPipeline<'a> {
    providers: Vec<Box<dyn CandidateExtensionProvider + 'a>>,
}

impl<'a> ExtensionPipeline<'a> {
    /// 构造包含所有标准扩展源的默认管道
    pub fn default_pipeline(resources: &'a LexiconResources) -> Self {
        Self {
            providers: vec![
                Box::new(CustomPhraseProvider {
                    custom_phrases: &resources.custom_phrases,
                }),
                Box::new(DateTimeProvider),
                Box::new(EnglishSuggesterProvider {
                    english_suggester: &resources.english_suggester,
                }),
                Box::new(SymbolSuggesterProvider {
                    symbol_suggester: &resources.symbol_suggester,
                }),
            ],
        }
    }

    /// 静态无分配收集默认扩展源候选（热点路径优先调用，消除 Box 与动态分派堆开销）
    pub fn collect_default_extensions(
        resources: &'a LexiconResources,
        ctx: &ExtensionContext<'_>,
        out: &mut Vec<AlgoCandidate>,
    ) {
        CustomPhraseProvider {
            custom_phrases: &resources.custom_phrases,
        }
        .append_candidates(ctx, out);
        DateTimeProvider.append_candidates(ctx, out);
        EnglishSuggesterProvider {
            english_suggester: &resources.english_suggester,
        }
        .append_candidates(ctx, out);
        SymbolSuggesterProvider {
            symbol_suggester: &resources.symbol_suggester,
        }
        .append_candidates(ctx, out);
    }

    /// 注册自定义扩展提供者
    pub fn register(&mut self, provider: Box<dyn CandidateExtensionProvider + 'a>) {
        self.providers.push(provider);
    }

    /// 执行管道收集所有扩展候选项
    pub fn collect_extensions(&self, ctx: &ExtensionContext<'_>) -> Vec<AlgoCandidate> {
        let mut extensions = Vec::new();
        for provider in &self.providers {
            provider.append_candidates(ctx, &mut extensions);
        }
        extensions
    }
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
            traditional_chinese: self.config.traditional_chinese,
            opencc: &self.resources.opencc_dict,
        };
        let mut extensions = Vec::with_capacity(8);
        ExtensionPipeline::collect_default_extensions(&self.resources, &ctx, &mut extensions);

        // 4. 按照确定的语义层级编排扩展候选
        self.merge_extensions_by_hierarchy(&extensions, candidates);
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
            cand.text = self.resources.opencc_dict.convert_text(&cand.text);
        }
        if self.config.show_pinyin_comment && cand.comment.is_none() && !cand.pinyin.is_empty() {
            cand.comment = Some(format!("[{}]", cand.pinyin));
        }
        cand.tier = CandidateTier::PrefixPartial;
        cand
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockCustomProvider;
    impl CandidateExtensionProvider for MockCustomProvider {
        fn provide(&self, ctx: &ExtensionContext<'_>) -> Vec<AlgoCandidate> {
            if ctx.raw_input == "test" {
                vec![AlgoCandidate::new("测试扩展", "ce'shi", 100)
                    .with_tier(CandidateTier::ProtectedEntity)]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn test_extension_pipeline_custom_registration() {
        let opencc = OpenCcDict::global();
        let mut pipeline = ExtensionPipeline {
            providers: Vec::new(),
        };
        pipeline.register(Box::new(MockCustomProvider));

        let candidates = Vec::new();
        let ctx = ExtensionContext {
            raw_input: "test",
            existing_candidates: &candidates,
            traditional_chinese: false,
            opencc: &opencc,
        };

        let result = pipeline.collect_extensions(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "测试扩展");
        assert_eq!(result[0].tier, CandidateTier::ProtectedEntity);
    }
}
