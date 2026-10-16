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
    /// 神经网络对数困惑度得分在综合排序中的插值权重（推荐 0.10 ~ 0.15）
    pub alpha: f32,
    /// 挑战者推翻基线首选所需的最小神经平均对数似然度优势门槛（置信度门控）
    pub margin_threshold: f32,
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
            alpha: 0.12,
            margin_threshold: 0.15,
        })
    }

    /// 设置重排序插值权重
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// 设置挑战者推翻基线首选的置信度门槛
    pub fn with_margin_threshold(mut self, threshold: f32) -> Self {
        self.margin_threshold = threshold.max(0.0);
        self
    }
}

impl SentenceReranker for NeuralReranker {
    fn rerank(&self, candidates: &mut [AlgoCandidate], context: Option<&str>) {
        if candidates.len() <= 1 {
            return;
        }

        let ctx_str = context.unwrap_or("");

        // 1. 逐条计算每个候选整句在神经模型下的序列平均对数似然度
        let mut ll_scores: Vec<Option<f32>> = Vec::with_capacity(candidates.len());
        for cand in candidates.iter() {
            let eval_text = if ctx_str.is_empty() {
                cand.text.clone()
            } else {
                format!("{}{}", ctx_str, cand.text)
            };

            let ll = if let Ok(token_ids) = self.tokenizer.encode(&eval_text, true) {
                if token_ids.len() > 1 {
                    self.model
                        .compute_sequence_log_likelihood(&token_ids)
                        .ok()
                        .map(|total_ll| {
                            // 依据汉字字符数归一化，消除因生僻字被 BPE 切碎为多个 Byte Token 导致的序列长度失真偏差
                            let char_count = cand.text.chars().count() as f32;
                            total_ll / char_count.max(1.0)
                        })
                } else {
                    None
                }
            } else {
                None
            };
            ll_scores.push(ll);
        }

        // 2. 提取基线首选 (Index 0) 作为基准锚点
        let Some(base_ll) = ll_scores[0] else {
            return;
        };

        // 3. 遍历后续挑战者，应用置信度差值门控 (Margin Gating)
        for (i, cand) in candidates.iter_mut().enumerate() {
            if i == 0 {
                continue;
            }
            if let Some(cand_ll) = ll_scores[i] {
                let delta_ll = cand_ll - base_ll;
                // 置信度门控保护：仅当挑战者的神经模型对数概率明显超越基线首选 (>= margin_threshold) 时，
                // 才赋予综合提权，杜绝低容量模型在细粒度高频词上的微弱随机扰动误伤基线词库首选决策。
                if delta_ll >= self.margin_threshold {
                    // 基准刻度：每 1.0 的对数优势对应约 25,000 分转移权重
                    let bonus = (delta_ll * 25_000.0 * self.alpha) as i64;
                    cand.score = cand.score.saturating_add(bonus.clamp(0, i64::MAX) as u32);
                } else if delta_ll < -self.margin_threshold {
                    // 挑战者流利度明显劣化时施加适度惩罚
                    let penalty = ((-delta_ll) * 25_000.0 * self.alpha) as i64;
                    cand.score = cand.score.saturating_sub(penalty.clamp(0, i64::MAX) as u32);
                }
            }
        }

        // 4. 依据融合后的综合得分重新降序排序
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
