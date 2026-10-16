use crate::types::AlgoCandidate;

/// 整句重排序与自回归联想接口契约 (Sentence Reranker Trait)
///
/// # 设计原理
/// - **实现初衷**：解耦输入法底层解码器与高阶语言模型（如统计模型、紧凑因果神经模型），
///   实现可插拔的整句困惑度重排序与上屏后联想预测能力。
/// - **核心优势**：接口保持无锁与绝对并发安全（`Send + Sync`），支持根据运行平台与配置灵活挂载。
/// - **代价与局限**：打分过程涉及文本遍历与前向推理，实现方需严格保证在时间预算（通常 < 10ms）内返回。
pub trait SentenceReranker: std::fmt::Debug + Send + Sync {
    /// 对传入的 K-Best 候选整句进行语言模型打分与原位重排序
    ///
    /// # 参数
    /// - `candidates`: Viterbi 算法解码出的候选整句切片，按初始声学/转移得分降序排列；
    /// - `context`: 上屏的历史上下文文本（可选）。
    fn rerank(&self, candidates: &mut [AlgoCandidate], context: Option<&str>);

    /// 基于已提交的历史上下文预测下一个最可能输入的词或字（下一词智能联想）
    ///
    /// # 参数
    /// - `context`: 最近上屏的历史文本；
    /// - `top_k`: 期望返回的高置信度建议条数上限。
    fn predict_next(&self, context: &str, top_k: usize) -> Vec<String>;
}

/// 默认空操作重排序器 (No-Op Reranker)
///
/// 当系统未加载神经网络权重或运行于极低功耗嵌入式环境时作为安全保底，
/// 完全保持原始候选顺序不变，零内存与零计算开销。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopReranker;

impl SentenceReranker for NoopReranker {
    #[inline]
    fn rerank(&self, _candidates: &mut [AlgoCandidate], _context: Option<&str>) {
        // 透传模式：保持传统 Viterbi + Bigram 原始排序不变
    }

    #[inline]
    fn predict_next(&self, _context: &str, _top_k: usize) -> Vec<String> {
        // 空操作：未挂载模型时不提供联想建议
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_reranker_preserves_order() {
        let reranker = NoopReranker;
        let mut candidates = vec![
            AlgoCandidate::new("中国人民", "zhong'guo'ren'min", 100_000),
            AlgoCandidate::new("钟表仁民", "zhong'guo'ren'min", 50_000),
        ];

        reranker.rerank(&mut candidates, None);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].text, "中国人民");
        assert_eq!(candidates[1].text, "钟表仁民");

        let predictions = reranker.predict_next("今天天气", 3);
        assert!(predictions.is_empty());
    }
}
