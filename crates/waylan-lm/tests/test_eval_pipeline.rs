//! 神经网络语言模型评测管线与重排序端到端集成测试

use waylan_algo::{AlgoCandidate, SentenceReranker};
use waylan_lm::weights::{LayerWeights, ModelWeights, Tensor2D};
use waylan_lm::{LMTokenizer, NeuralReranker, WayLanLMConfig, WayLanLMModel};

/// 构造微型合成测试模型
fn build_test_reranker() -> NeuralReranker {
    let config = WayLanLMConfig {
        vocab_size: 100,
        hidden_size: 32,
        num_hidden_layers: 2,
        num_attention_heads: 2,
        num_key_value_heads: 1,
        head_dim: 16,
        intermediate_size: 64,
        max_position_embeddings: 64,
        ..Default::default()
    };

    let embed = Tensor2D::new(
        config.vocab_size,
        config.hidden_size,
        vec![0.02; config.vocab_size * config.hidden_size],
    )
    .unwrap();

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
                vec![0.02; q_dim * config.hidden_size],
            )
            .unwrap(),
            k_proj: Tensor2D::new(
                kv_dim,
                config.hidden_size,
                vec![0.02; kv_dim * config.hidden_size],
            )
            .unwrap(),
            v_proj: Tensor2D::new(
                kv_dim,
                config.hidden_size,
                vec![0.02; kv_dim * config.hidden_size],
            )
            .unwrap(),
            o_proj: Tensor2D::new(
                config.hidden_size,
                q_dim,
                vec![0.02; config.hidden_size * q_dim],
            )
            .unwrap(),
            gate_proj: Tensor2D::new(
                config.intermediate_size,
                config.hidden_size,
                vec![0.02; config.intermediate_size * config.hidden_size],
            )
            .unwrap(),
            up_proj: Tensor2D::new(
                config.intermediate_size,
                config.hidden_size,
                vec![0.02; config.intermediate_size * config.hidden_size],
            )
            .unwrap(),
            down_proj: Tensor2D::new(
                config.hidden_size,
                config.intermediate_size,
                vec![0.02; config.hidden_size * config.intermediate_size],
            )
            .unwrap(),
        });
    }

    let weights = ModelWeights {
        embed_tokens: embed,
        layers,
        final_norm: vec![1.0; config.hidden_size],
        lm_head: None,
    };
    let model = WayLanLMModel::new(config, weights);

    let minimal_json = r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[{"id":0,"content":"<pad>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":1,"content":"<bos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":2,"content":"<eos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true}],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,"vocab":{"<pad>":0,"<bos>":1,"<eos>":2},"merges":[]}}"#;
    let tokenizer = LMTokenizer::from_bytes(minimal_json.as_bytes()).unwrap();

    NeuralReranker {
        model,
        tokenizer,
        alpha: 0.12,
        margin_threshold: 0.15,
    }
}

#[test]
fn test_reranker_handles_empty_and_single_candidate() {
    let reranker = build_test_reranker();
    let mut empty_cands: Vec<AlgoCandidate> = Vec::new();
    reranker.rerank(&mut empty_cands, None);
    assert!(empty_cands.is_empty());

    let mut single_cand = vec![AlgoCandidate::new("测试", "ce'shi", 100)];
    reranker.rerank(&mut single_cand, Some("前置文本"));
    assert_eq!(single_cand.len(), 1);
    assert_eq!(single_cand[0].text, "测试");
}

#[test]
fn test_reranker_maintains_deterministic_sorting() {
    let reranker = build_test_reranker();
    let mut candidates = vec![
        AlgoCandidate::new("先", "xian", 5000),
        AlgoCandidate::new("西安", "xi'an", 4000),
        AlgoCandidate::new("熄安", "xi'an", 3000),
    ];

    // 重排序应能够执行并保持候选数量完整
    reranker.rerank(&mut candidates, Some("去古都旅游"));
    assert_eq!(candidates.len(), 3);
    assert!(candidates[0].score >= candidates[1].score);
    assert!(candidates[1].score >= candidates[2].score);
}

#[test]
fn test_next_token_prediction_probabilities_normalized() {
    let reranker = build_test_reranker();
    let token_ids = vec![1, 2];
    let top_k = reranker.model.predict_next_token(&token_ids, 5).unwrap();
    assert_eq!(top_k.len(), 5);

    // 概率应处于 [0.0, 1.0] 区间内且按从高到低单调递减
    for i in 0..top_k.len() {
        assert!(top_k[i].1 >= 0.0 && top_k[i].1 <= 1.0);
        if i > 0 {
            assert!(top_k[i - 1].1 >= top_k[i].1);
        }
    }
}
