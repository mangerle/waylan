//! 首页扩展候选词流水线与动态注入
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供高度可扩展的候选词扩展能力，将自定义短语、动态时间卡片、
//!   英文单词联想、特殊符号表情等多样化扩展项从主算法解码中解耦，彻底遵循开闭原则 (OCP)。
//! - **核心优势**：插件化管道模式（Pipeline Pattern），新增扩展项仅需实现 [`CandidateExtensionProvider`]
//!   特型并在管道中注册，无需修改任何现有解码逻辑。
//! - **代价与局限**：扩展项在流水线收集后需要根据语义优先级进行统一编排，插入操作涉及少量切片重排。

mod merge;
mod pipeline;
mod providers;

pub use pipeline::ExtensionPipeline;
pub use providers::{
    CandidateExtensionProvider, CustomPhraseProvider, DateTimeProvider, EnglishSuggesterProvider,
    ExtensionContext, SymbolSuggesterProvider,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencc::OpenCcDict;
    use crate::types::{AlgoCandidate, CandidateTier};

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
        let mut pipeline = ExtensionPipeline::new();
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
