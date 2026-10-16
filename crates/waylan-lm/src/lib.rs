pub mod config;
pub mod error;
pub mod model;
pub mod reranker;
pub mod tokenizer;
pub mod weights;

pub use config::WayLanLMConfig;
pub use error::NeuralLMError;
pub use model::WayLanLMModel;
pub use reranker::NeuralReranker;
pub use tokenizer::LMTokenizer;
pub use weights::{LayerWeights, ModelWeights, Tensor2D};

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造合成极微型模型用于测试前向推理、对数似然打分与重排序流程
    fn create_mock_model() -> WayLanLMModel {
        let config = WayLanLMConfig {
            hidden_size: 64,
            num_hidden_layers: 2,
            num_attention_heads: 4,
            num_key_value_heads: 2,
            head_dim: 16,
            intermediate_size: 128,
            vocab_size: 100,
            max_position_embeddings: 64,
            ..Default::default()
        };

        let embed_tokens = Tensor2D::new(
            config.vocab_size,
            config.hidden_size,
            vec![0.01; config.vocab_size * config.hidden_size],
        )
        .unwrap();

        let final_norm = vec![1.0; config.hidden_size];

        let mut layers = Vec::new();
        for _ in 0..config.num_hidden_layers {
            let q_dim = config.num_attention_heads * config.head_dim;
            let kv_dim = config.num_key_value_heads * config.head_dim;

            layers.push(LayerWeights {
                input_norm: vec![1.0; config.hidden_size],
                post_attention_norm: vec![1.0; config.hidden_size],
                q_proj: Tensor2D::new(
                    q_dim,
                    config.hidden_size,
                    vec![0.01; q_dim * config.hidden_size],
                )
                .unwrap(),
                k_proj: Tensor2D::new(
                    kv_dim,
                    config.hidden_size,
                    vec![0.01; kv_dim * config.hidden_size],
                )
                .unwrap(),
                v_proj: Tensor2D::new(
                    kv_dim,
                    config.hidden_size,
                    vec![0.01; kv_dim * config.hidden_size],
                )
                .unwrap(),
                o_proj: Tensor2D::new(
                    config.hidden_size,
                    q_dim,
                    vec![0.01; config.hidden_size * q_dim],
                )
                .unwrap(),
                gate_proj: Tensor2D::new(
                    config.intermediate_size,
                    config.hidden_size,
                    vec![0.01; config.intermediate_size * config.hidden_size],
                )
                .unwrap(),
                up_proj: Tensor2D::new(
                    config.intermediate_size,
                    config.hidden_size,
                    vec![0.01; config.intermediate_size * config.hidden_size],
                )
                .unwrap(),
                down_proj: Tensor2D::new(
                    config.hidden_size,
                    config.intermediate_size,
                    vec![0.01; config.hidden_size * config.intermediate_size],
                )
                .unwrap(),
            });
        }

        let weights = ModelWeights {
            embed_tokens,
            final_norm,
            lm_head: None,
            layers,
        };

        WayLanLMModel::new(config, weights)
    }

    #[test]
    fn test_forward_and_log_likelihood() {
        let model = create_mock_model();
        let token_ids = vec![1, 10, 20, 30];

        // 验证完整前向
        let logits = model.forward(&token_ids).expect("前向计算应成功");
        assert_eq!(logits.len(), token_ids.len() * model.config.vocab_size);

        // 验证序列对数似然计算
        let ll = model
            .compute_sequence_log_likelihood(&token_ids)
            .expect("对数似然计算应成功");
        assert!(ll.is_finite());

        // 验证 Next-Token 预测
        let next_tokens = model.predict_next_token(&token_ids, 5).expect("预测应成功");
        assert_eq!(next_tokens.len(), 5);
        for i in 0..4 {
            assert!(
                next_tokens[i].1 >= next_tokens[i + 1].1,
                "预测概率应单调降序"
            );
        }
    }
}
