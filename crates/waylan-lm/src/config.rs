use crate::error::NeuralLMError;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// 微澜本地轻量语言模型超参数配置
///
/// 严格对齐 MiniMind 中定义的 `MiniMindConfig::waylan_16m` 规格：
/// - 4 层 Transformer 解码器；
/// - 512 隐藏层特征维度；
/// - 8 查询头、4 键值头（GQA 2:1 分组共享）；
/// - 1536 维度 SwiGLU 紧凑 FFN；
/// - 6400 紧凑词表与权重绑定（Weight Tying）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WayLanLMConfig {
    /// 隐藏层特征维度 d_model（默认 512）
    pub hidden_size: usize,
    /// 解码器堆叠层数（默认 4）
    pub num_hidden_layers: usize,
    /// 查询头注意力头数 H_Q（默认 8）
    pub num_attention_heads: usize,
    /// 键值头注意力头数 H_KV（默认 4）
    pub num_key_value_heads: usize,
    /// 单个注意力头维度（默认 64）
    pub head_dim: usize,
    /// 前馈层中间维度（默认 1536）
    pub intermediate_size: usize,
    /// 词表大小（默认 6400）
    pub vocab_size: usize,
    /// 序列起始标记 ID
    pub bos_token_id: usize,
    /// 序列结束标记 ID
    pub eos_token_id: usize,
    /// 批处理填充标记 ID
    pub pad_token_id: usize,
    /// 最大上下文位置嵌入长度（默认 512）
    pub max_position_embeddings: usize,
    /// RMSNorm 防除零微小常数
    pub rms_norm_eps: f64,
    /// RoPE 基础波长频率常数
    pub rope_theta: f64,
    /// 是否共享词嵌入层与输出投影层权重
    pub tie_word_embeddings: bool,
}

impl Default for WayLanLMConfig {
    fn default() -> Self {
        let hidden_size = 512;
        let num_attention_heads = 8;
        let num_key_value_heads = 4;
        let head_dim = hidden_size / num_attention_heads;

        Self {
            hidden_size,
            num_hidden_layers: 4,
            num_attention_heads,
            num_key_value_heads,
            head_dim,
            intermediate_size: 1536,
            vocab_size: 6400,
            bos_token_id: 1,
            eos_token_id: 2,
            pad_token_id: 0,
            max_position_embeddings: 512,
            rms_norm_eps: 1e-5,
            rope_theta: 10000.0,
            tie_word_embeddings: true,
        }
    }
}

impl WayLanLMConfig {
    /// 从本地 JSON 配置文件反序列化加载配置
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, NeuralLMError> {
        let p = path.as_ref();
        let file = File::open(p).map_err(|e| NeuralLMError::IoError {
            path: p.display().to_string(),
            source: e,
        })?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader)
            .map_err(|e| NeuralLMError::ConfigDeserializeError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_and_alignment() {
        let config = WayLanLMConfig::default();
        assert_eq!(config.hidden_size, 512);
        assert_eq!(config.num_hidden_layers, 4);
        assert_eq!(config.num_attention_heads, 8);
        assert_eq!(config.num_key_value_heads, 4);
        assert_eq!(config.head_dim, 64);
        assert_eq!(config.intermediate_size, 1536);
        assert_eq!(config.vocab_size, 6400);
        assert!(config.tie_word_embeddings);
    }
}
