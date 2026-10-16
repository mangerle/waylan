use crate::config::WayLanLMConfig;
use crate::error::NeuralLMError;
use crate::model::WayLanLMModel;
use crate::tokenizer::LMTokenizer;
use crate::weights::ModelWeights;
use std::path::Path;
use waylan_algo::{AlgoCandidate, SentenceReranker};

/// 神经语言模型整句重排序与联想引擎实现
#[derive(Debug)]
pub struct NeuralReranker {
    pub model: WayLanLMModel,
    pub tokenizer: LMTokenizer,
    /// 神经网络对数困惑度得分在综合排序中的插值权重（推荐 0.35）
    pub alpha: f32,
}

impl NeuralReranker {
    /// 从权重文件与分词器文件初始化加载神经重排序器
    pub fn load_from_files(
        weights_path: impl AsRef<Path>,
        tokenizer_path: impl AsRef<Path>,
        config: Option<WayLanLMConfig>,
    ) -> Result<Self, NeuralLMError> {
        let cfg = config.unwrap_or_default();
        let weights = ModelWeights::from_safetensors_file(weights_path, &cfg)?;
        let tokenizer = LMTokenizer::from_file(tokenizer_path)?;
        let model = WayLanLMModel::new(cfg, weights);

        Ok(Self {
            model,
            tokenizer,
            alpha: 0.35,
        })
    }

    /// 设置重排序插值权重
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }
}

impl SentenceReranker for NeuralReranker {
    fn rerank(&self, candidates: &mut [AlgoCandidate], context: Option<&str>) {
        if candidates.is_empty() {
            return;
        }

        let ctx_str = context.unwrap_or("");

        for cand in candidates.iter_mut() {
            let eval_text = if ctx_str.is_empty() {
                cand.text.clone()
            } else {
                format!("{}{}", ctx_str, cand.text)
            };

            if let Ok(token_ids) = self.tokenizer.encode(&eval_text, true) {
                if token_ids.len() > 1 {
                    if let Ok(total_ll) = self.model.compute_sequence_log_likelihood(&token_ids) {
                        // 字符级平均对数似然度，消除长短句子因 Token 长度累加产生的系统性偏差
                        let token_count = (token_ids.len() - 1) as f32;
                        let avg_ll = total_ll / token_count.max(1.0);

                        // 将对数似然（通常在 -1.0 到 -5.0）平移映射至输入法整数评分维度
                        // 经验刻度：每 1.0 的对数概率差异对应约 10,000 分转移权重
                        let neural_bonus = (avg_ll * 10_000.0 * self.alpha) as i64;
                        let orig_score = cand.score as i64;
                        let final_score = orig_score
                            .saturating_add(neural_bonus)
                            .clamp(1, u32::MAX as i64)
                            as u32;
                        cand.score = final_score;
                    }
                }
            }
        }

        // 依据融合后的综合得分重新降序排序
        candidates.sort_by_key(|a| std::cmp::Reverse(a.score));
    }

    fn predict_next(&self, context: &str, top_k: usize) -> Vec<String> {
        if context.is_empty() || top_k == 0 {
            return Vec::new();
        }

        let token_ids = match self.tokenizer.encode(context, true) {
            Ok(ids) if !ids.is_empty() => ids,
            _ => return Vec::new(),
        };

        // 仅保留最近 32 个历史 Token，避免过长上下文产生无谓推理耗时
        let eval_ids = if token_ids.len() > 32 {
            &token_ids[(token_ids.len() - 32)..]
        } else {
            &token_ids[..]
        };

        // 提取概率最高的 Top 候选
        let top_candidates = match self.model.predict_next_token(eval_ids, top_k * 2) {
            Ok(cands) => cands,
            Err(_) => return Vec::new(),
        };

        let mut results = Vec::with_capacity(top_k);
        let mut seen = std::collections::HashSet::with_capacity(top_k);

        for (token_id, _prob) in top_candidates {
            if let Ok(text) = self.tokenizer.decode(&[token_id], true) {
                let trimmed = text.trim();
                if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                    results.push(trimmed.to_string());
                    if results.len() >= top_k {
                        break;
                    }
                }
            }
        }

        results
    }
}
