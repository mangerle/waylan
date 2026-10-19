//! 扩展候选词聚合流水线
//!
//! # 设计原理
//! - **实现初衷**：统一调度已注册的候选扩展项，支持动态流水线注册与热路径零堆分配静态分派。
//! - **核心优势**：在热路径中提供 `collect_default_extensions`，消除击键敏感循环内的堆微分配。

use super::providers::{
    CandidateExtensionProvider, CustomPhraseProvider, DateTimeProvider, EnglishSuggesterProvider,
    ExtensionContext, SymbolSuggesterProvider,
};
use crate::engine::resources::LexiconResources;
use crate::types::AlgoCandidate;

/// 扩展候选词聚合流水线 (Extension Pipeline)
#[derive(Default)]
pub struct ExtensionPipeline<'a> {
    pub(crate) providers: Vec<Box<dyn CandidateExtensionProvider + 'a>>,
}

impl<'a> ExtensionPipeline<'a> {
    /// 构造空流水线
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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
