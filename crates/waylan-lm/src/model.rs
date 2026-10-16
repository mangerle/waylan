use crate::config::WayLanLMConfig;
use crate::error::NeuralLMError;
use crate::weights::{LayerWeights, ModelWeights};

/// 微澜 4 层微型语言模型前向推理引擎
#[derive(Debug, Clone)]
pub struct WayLanLMModel {
    pub config: WayLanLMConfig,
    pub weights: ModelWeights,
    // 预计算的 RoPE 旋转位置编码正余弦缓存：[max_pos, head_dim / 2]
    cos_table: Vec<Vec<f32>>,
    sin_table: Vec<Vec<f32>>,
}

impl WayLanLMModel {
    pub fn new(config: WayLanLMConfig, weights: ModelWeights) -> Self {
        let (cos_table, sin_table) = Self::precompute_rope(&config);
        Self {
            config,
            weights,
            cos_table,
            sin_table,
        }
    }

    /// 预计算 RoPE 频率表，避免前向传播重复计算三角函数
    fn precompute_rope(config: &WayLanLMConfig) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let max_pos = config.max_position_embeddings;
        let half_dim = config.head_dim / 2;
        let mut cos_table = Vec::with_capacity(max_pos);
        let mut sin_table = Vec::with_capacity(max_pos);

        let inv_freq: Vec<f32> = (0..half_dim)
            .map(|i| {
                let exponent = (2 * i) as f64 / config.head_dim as f64;
                (1.0 / config.rope_theta.powf(exponent)) as f32
            })
            .collect();

        for pos in 0..max_pos {
            let mut cos_row = Vec::with_capacity(half_dim);
            let mut sin_row = Vec::with_capacity(half_dim);
            for &freq in &inv_freq {
                let val = (pos as f32) * freq;
                cos_row.push(val.cos());
                sin_row.push(val.sin());
            }
            cos_table.push(cos_row);
            sin_table.push(sin_row);
        }

        (cos_table, sin_table)
    }

    /// 计算给定 Token ID 序列的累积自回归对数似然（Log-Likelihood，用于整句 PPL 打分）
    pub fn compute_sequence_log_likelihood(&self, token_ids: &[u32]) -> Result<f32, NeuralLMError> {
        let seq_len = token_ids.len();
        if seq_len <= 1 {
            return Ok(0.0);
        }

        let logits = self.forward(token_ids)?;
        let vocab_size = self.config.vocab_size;
        let mut total_log_likelihood = 0.0f32;

        // 对每一个位置 t，使用其 logits 评估真实出现的下一个词 token_ids[t + 1] 的条件概率
        for t in 0..(seq_len - 1) {
            let start = t * vocab_size;
            let end = start + vocab_size;
            let step_logits = &logits[start..end];

            let target_token = token_ids[t + 1] as usize;
            if target_token >= vocab_size {
                continue;
            }

            let log_prob = Self::log_softmax_at(step_logits, target_token);
            total_log_likelihood += log_prob;
        }

        Ok(total_log_likelihood)
    }

    /// 基于输入序列预测下一个 Token 的概率分布，返回 Top-K 候选 Token ID 及其置信度概率
    pub fn predict_next_token(
        &self,
        token_ids: &[u32],
        top_k: usize,
    ) -> Result<Vec<(u32, f32)>, NeuralLMError> {
        if token_ids.is_empty() {
            return Ok(Vec::new());
        }

        let seq_len = token_ids.len();
        let logits = self.forward(token_ids)?;
        let vocab_size = self.config.vocab_size;

        // 提取最后一个位置的 logits
        let last_start = (seq_len - 1) * vocab_size;
        let last_logits = &logits[last_start..(last_start + vocab_size)];

        // Softmax 归一化
        let probs = Self::softmax(last_logits);

        // 选出前 Top-K 个最大的概率
        let mut indexed_probs: Vec<(u32, f32)> = probs
            .iter()
            .enumerate()
            .map(|(idx, &p)| (idx as u32, p))
            .collect();

        indexed_probs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        indexed_probs.truncate(top_k);

        Ok(indexed_probs)
    }

    /// 完整模型前向传播，输出形状为 [seq_len * vocab_size] 的未归一化 Logits
    pub fn forward(&self, token_ids: &[u32]) -> Result<Vec<f32>, NeuralLMError> {
        let seq_len = token_ids.len();
        let hidden_size = self.config.hidden_size;

        // 1. 词嵌入查表：[seq_len, hidden_size]
        let mut hidden_states = Vec::with_capacity(seq_len * hidden_size);
        for &token_id in token_ids {
            let id = (token_id as usize).min(self.config.vocab_size - 1);
            let row_start = id * hidden_size;
            let row = &self.weights.embed_tokens.data[row_start..(row_start + hidden_size)];
            hidden_states.extend_from_slice(row);
        }

        // 2. 逐层通过 4 个 Transformer Decoder 块
        for layer in &self.weights.layers {
            self.forward_layer(layer, &mut hidden_states, seq_len);
        }

        // 3. 最终层归一化 RMSNorm
        for t in 0..seq_len {
            let start = t * hidden_size;
            let end = start + hidden_size;
            Self::apply_rmsnorm(
                &mut hidden_states[start..end],
                &self.weights.final_norm,
                self.config.rms_norm_eps as f32,
            );
        }

        // 4. LM Head 分类投影
        let vocab_size = self.config.vocab_size;
        let mut logits = vec![0.0f32; seq_len * vocab_size];

        if let Some(ref head) = self.weights.lm_head {
            for t in 0..seq_len {
                let h_start = t * hidden_size;
                let h_step = &hidden_states[h_start..(h_start + hidden_size)];
                let l_start = t * vocab_size;
                let l_step = &mut logits[l_start..(l_start + vocab_size)];
                head.matvec(h_step, l_step);
            }
        } else {
            // 共享权重 (Weight Tying)：复用 embed_tokens 权重
            for t in 0..seq_len {
                let h_start = t * hidden_size;
                let h_step = &hidden_states[h_start..(h_start + hidden_size)];
                let l_start = t * vocab_size;
                let l_step = &mut logits[l_start..(l_start + vocab_size)];
                self.weights.embed_tokens.matvec(h_step, l_step);
            }
        }

        Ok(logits)
    }

    /// 单个 Decoder 层的前向传播（自注意力残差 + SwiGLU 前馈残差）
    fn forward_layer(&self, layer: &LayerWeights, hidden_states: &mut [f32], seq_len: usize) {
        let hidden_size = self.config.hidden_size;

        // 1. 自注意力前置归一化与注意力残差
        let mut attn_input = hidden_states.to_vec();
        for t in 0..seq_len {
            let start = t * hidden_size;
            let end = start + hidden_size;
            Self::apply_rmsnorm(
                &mut attn_input[start..end],
                &layer.input_norm,
                self.config.rms_norm_eps as f32,
            );
        }

        let attn_out = self.forward_gqa(layer, &attn_input, seq_len);
        for i in 0..hidden_states.len() {
            hidden_states[i] += attn_out[i];
        }

        // 2. SwiGLU 前馈前置归一化与前馈残差
        let mut ffn_input = hidden_states.to_vec();
        for t in 0..seq_len {
            let start = t * hidden_size;
            let end = start + hidden_size;
            Self::apply_rmsnorm(
                &mut ffn_input[start..end],
                &layer.post_attention_norm,
                self.config.rms_norm_eps as f32,
            );
        }

        let ffn_out = self.forward_swiglu(layer, &ffn_input, seq_len);
        for i in 0..hidden_states.len() {
            hidden_states[i] += ffn_out[i];
        }
    }

    /// 分组查询注意力 GQA 前向传播
    fn forward_gqa(&self, layer: &LayerWeights, x: &[f32], seq_len: usize) -> Vec<f32> {
        let hidden_size = self.config.hidden_size;
        let num_q_heads = self.config.num_attention_heads;
        let num_kv_heads = self.config.num_key_value_heads;
        let head_dim = self.config.head_dim;
        let group_size = num_q_heads / num_kv_heads; // 例如 8 / 4 = 2

        let mut q = vec![0.0f32; seq_len * num_q_heads * head_dim];
        let mut k = vec![0.0f32; seq_len * num_kv_heads * head_dim];
        let mut v = vec![0.0f32; seq_len * num_kv_heads * head_dim];

        // 投影 Q, K, V
        for t in 0..seq_len {
            let x_step = &x[(t * hidden_size)..((t + 1) * hidden_size)];
            layer.q_proj.matvec(
                x_step,
                &mut q[(t * num_q_heads * head_dim)..((t + 1) * num_q_heads * head_dim)],
            );
            layer.k_proj.matvec(
                x_step,
                &mut k[(t * num_kv_heads * head_dim)..((t + 1) * num_kv_heads * head_dim)],
            );
            layer.v_proj.matvec(
                x_step,
                &mut v[(t * num_kv_heads * head_dim)..((t + 1) * num_kv_heads * head_dim)],
            );
        }

        // 应用 RoPE 旋转位置编码
        for t in 0..seq_len {
            let pos = t.min(self.config.max_position_embeddings - 1);
            let cos_row = &self.cos_table[pos];
            let sin_row = &self.sin_table[pos];

            for h in 0..num_q_heads {
                let start = (t * num_q_heads + h) * head_dim;
                Self::apply_rope_to_slice(&mut q[start..(start + head_dim)], cos_row, sin_row);
            }
            for h in 0..num_kv_heads {
                let start = (t * num_kv_heads + h) * head_dim;
                Self::apply_rope_to_slice(&mut k[start..(start + head_dim)], cos_row, sin_row);
            }
        }

        // 计算带因果掩码的自注意力
        let scale = 1.0f32 / (head_dim as f32).sqrt();
        let mut attn_context = vec![0.0f32; seq_len * num_q_heads * head_dim];

        for h_q in 0..num_q_heads {
            let h_kv = h_q / group_size;

            for i in 0..seq_len {
                let q_start = (i * num_q_heads + h_q) * head_dim;
                let q_vec = &q[q_start..(q_start + head_dim)];

                let mut scores = Vec::with_capacity(i + 1);
                for j in 0..=i {
                    // 因果掩码：j <= i
                    let k_start = (j * num_kv_heads + h_kv) * head_dim;
                    let k_vec = &k[k_start..(k_start + head_dim)];

                    let mut dot = 0.0f32;
                    for d in 0..head_dim {
                        dot += q_vec[d] * k_vec[d];
                    }
                    scores.push(dot * scale);
                }

                let weights = Self::softmax_masked(&scores);

                let out_start = (i * num_q_heads + h_q) * head_dim;
                for (j, &w) in weights.iter().enumerate() {
                    let v_start = (j * num_kv_heads + h_kv) * head_dim;
                    let v_vec = &v[v_start..(v_start + head_dim)];
                    for d in 0..head_dim {
                        attn_context[out_start + d] += w * v_vec[d];
                    }
                }
            }
        }

        // 输出投影 O Proj
        let mut output = vec![0.0f32; seq_len * hidden_size];
        for t in 0..seq_len {
            let ctx_step =
                &attn_context[(t * num_q_heads * head_dim)..((t + 1) * num_q_heads * head_dim)];
            let out_step = &mut output[(t * hidden_size)..((t + 1) * hidden_size)];
            layer.o_proj.matvec(ctx_step, out_step);
        }

        output
    }

    /// SwiGLU 前馈网络前向计算：(SiLU(x * W_gate) * (x * W_up)) * W_down
    fn forward_swiglu(&self, layer: &LayerWeights, x: &[f32], seq_len: usize) -> Vec<f32> {
        let hidden_size = self.config.hidden_size;
        let inter_size = self.config.intermediate_size;

        let mut output = vec![0.0f32; seq_len * hidden_size];
        let mut gate = vec![0.0f32; inter_size];
        let mut up = vec![0.0f32; inter_size];
        let mut intermediate = vec![0.0f32; inter_size];

        for t in 0..seq_len {
            let x_step = &x[(t * hidden_size)..((t + 1) * hidden_size)];
            layer.gate_proj.matvec(x_step, &mut gate);
            layer.up_proj.matvec(x_step, &mut up);

            for i in 0..inter_size {
                let silu = gate[i] / (1.0 + (-gate[i]).exp());
                intermediate[i] = silu * up[i];
            }

            let out_step = &mut output[(t * hidden_size)..((t + 1) * hidden_size)];
            layer.down_proj.matvec(&intermediate, out_step);
        }

        output
    }

    #[inline]
    fn apply_rmsnorm(x: &mut [f32], weight: &[f32], eps: f32) {
        let dim = x.len();
        let mut sum_sq = 0.0f32;
        for &val in x.iter() {
            sum_sq += val * val;
        }
        let rms = (sum_sq / (dim as f32) + eps).sqrt();
        let inv_rms = 1.0f32 / rms;

        for (val, &w) in x.iter_mut().zip(weight.iter()) {
            *val = (*val * inv_rms) * w;
        }
    }

    #[inline]
    fn apply_rope_to_slice(slice: &mut [f32], cos: &[f32], sin: &[f32]) {
        let half = slice.len() / 2;
        for i in 0..half {
            let x0 = slice[i];
            let x1 = slice[i + half];
            let c = cos[i];
            let s = sin[i];
            slice[i] = x0 * c - x1 * s;
            slice[i + half] = x0 * s + x1 * c;
        }
    }

    #[inline]
    fn softmax(logits: &[f32]) -> Vec<f32> {
        let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut exps = Vec::with_capacity(logits.len());
        let mut sum_exp = 0.0f32;
        for &val in logits {
            let e = (val - max_val).exp();
            exps.push(e);
            sum_exp += e;
        }
        let inv_sum = 1.0f32 / sum_exp.max(1e-12);
        for val in &mut exps {
            *val *= inv_sum;
        }
        exps
    }

    #[inline]
    fn softmax_masked(scores: &[f32]) -> Vec<f32> {
        let max_val = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut exps = Vec::with_capacity(scores.len());
        let mut sum_exp = 0.0f32;
        for &val in scores {
            let e = (val - max_val).exp();
            exps.push(e);
            sum_exp += e;
        }
        let inv_sum = 1.0f32 / sum_exp.max(1e-12);
        for val in &mut exps {
            *val *= inv_sum;
        }
        exps
    }

    #[inline]
    fn log_softmax_at(logits: &[f32], target_idx: usize) -> f32 {
        let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut sum_exp = 0.0f32;
        for &val in logits {
            sum_exp += (val - max_val).exp();
        }
        let target_val = logits[target_idx];
        (target_val - max_val) - sum_exp.ln()
    }
}
