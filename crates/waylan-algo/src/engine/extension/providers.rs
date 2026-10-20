//! 首页候选词扩展项提供者特型与默认实现
//!
//! # 设计原理
//! - **实现初衷**：为自定义短语、动态时间卡片、英文联想与特殊符号提供统一特型契约，
//!   实现与主算法解码逻辑的解耦。
//! - **核心优势**：遵循开闭原则 (OCP)，支持各扩展源独立演进与单元测试。

use crate::custom_phrase::{
    CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager,
};
use crate::evaluator::DateTimeEvaluator;
use crate::opencc::OpenCcDict;
use crate::types::{AlgoCandidate, CandidateTier};

/// 候选扩展上下文
pub struct ExtensionContext<'a> {
    /// 原始按键输入文本
    pub raw_input: &'a str,
    /// 现有候选词列表引用（用于去重与互斥门禁检测）
    pub existing_candidates: &'a [AlgoCandidate],
    /// 简繁转换词典（仅在启用繁体输出时存在，缺席即表示无需转换）
    pub opencc: Option<&'a OpenCcDict>,
}

/// 候选扩展项提供者特型 (Candidate Extension Provider)
///
/// 遵循开闭原则 (OCP)，支持动态插拔不同类型的候选词扩展源。
pub trait CandidateExtensionProvider {
    /// 根据当前上下文生成扩展候选词并追加至输出缓冲区（推荐零分配实现）
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>);
}

/// 用户自定义短语提供者（最高优先级，标记为 ProtectedEntity 置顶）
pub struct CustomPhraseProvider<'a> {
    pub custom_phrases: &'a CustomPhraseManager,
}

impl<'a> CandidateExtensionProvider for CustomPhraseProvider<'a> {
    fn append_candidates(&self, ctx: &ExtensionContext<'_>, out: &mut Vec<AlgoCandidate>) {
        let phrase_cands = self.custom_phrases.lookup(ctx.raw_input);
        for mut phrase in phrase_cands {
            if let Some(opencc) = ctx.opencc {
                phrase.text = opencc.convert_text(&phrase.text);
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
            if let Some(opencc) = ctx.opencc {
                dynamic_cand.text = opencc.convert_text(&dynamic_cand.text);
                for sub in &mut dynamic_cand.sub_candidates {
                    *sub = opencc.convert_text(sub);
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
