use crate::config::WayLanLMConfig;
use crate::error::NeuralLMError;
use safetensors::SafeTensors;
use std::fs::File;
use std::path::Path;

/// 二维浮点张量结构（行优先存储）
#[derive(Debug, Clone)]
pub struct Tensor2D {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

impl Tensor2D {
    pub fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self, NeuralLMError> {
        if data.len() != rows.saturating_mul(cols) {
            return Err(NeuralLMError::TensorShapeMismatch {
                name: "Tensor2D".to_string(),
                expected: vec![rows, cols],
                actual: vec![data.len()],
            });
        }
        Ok(Self { rows, cols, data })
    }

    /// 执行通用矩阵向量乘法：y = W * x，其中 W 形状为 [rows, cols]，x 长度为 cols，y 长度为 rows
    #[inline]
    pub fn matvec(&self, x: &[f32], y: &mut [f32]) {
        assert_eq!(x.len(), self.cols);
        assert_eq!(y.len(), self.rows);

        for (row_idx, row_chunk) in self.data.chunks_exact(self.cols).enumerate() {
            let mut dot = 0.0f32;
            for (w, &val) in row_chunk.iter().zip(x.iter()) {
                dot += w * val;
            }
            y[row_idx] = dot;
        }
    }
}

/// 单个 Transformer 解码器块的所有权重集合
#[derive(Debug, Clone)]
pub struct LayerWeights {
    pub input_norm: Vec<f32>,
    pub post_attention_norm: Vec<f32>,
    pub q_proj: Tensor2D,
    pub k_proj: Tensor2D,
    pub v_proj: Tensor2D,
    pub o_proj: Tensor2D,
    pub gate_proj: Tensor2D,
    pub up_proj: Tensor2D,
    pub down_proj: Tensor2D,
}

/// 完整模型的预加载权重集合
#[derive(Debug, Clone)]
pub struct ModelWeights {
    pub embed_tokens: Tensor2D,
    pub final_norm: Vec<f32>,
    pub lm_head: Option<Tensor2D>,
    pub layers: Vec<LayerWeights>,
}

impl ModelWeights {
    /// 从本地 Safetensors 权重文件中解析加载全量参数
    pub fn from_safetensors_file(
        path: impl AsRef<Path>,
        config: &WayLanLMConfig,
    ) -> Result<Self, NeuralLMError> {
        let p = path.as_ref();
        let file = File::open(p).map_err(|e| NeuralLMError::IoError {
            path: p.display().to_string(),
            source: e,
        })?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| NeuralLMError::IoError {
            path: p.display().to_string(),
            source: e,
        })?;
        let tensors = SafeTensors::deserialize(&mmap)
            .map_err(|e| NeuralLMError::SafeTensorsParseError(e.to_string()))?;

        Self::from_safetensors(&tensors, config)
    }

    /// 从已反序列化的 SafeTensors 视图中提取各层权重
    pub fn from_safetensors(
        tensors: &SafeTensors<'_>,
        config: &WayLanLMConfig,
    ) -> Result<Self, NeuralLMError> {
        let embed_tokens = Self::load_tensor_2d(
            tensors,
            "embed_tokens.weight",
            config.vocab_size,
            config.hidden_size,
        )?;

        let final_norm = Self::load_tensor_1d(tensors, "norm.weight", config.hidden_size)?;

        let lm_head = if tensors.names().contains(&"lm_head.weight") {
            Some(Self::load_tensor_2d(
                tensors,
                "lm_head.weight",
                config.vocab_size,
                config.hidden_size,
            )?)
        } else {
            None
        };

        let mut layers = Vec::with_capacity(config.num_hidden_layers);
        let q_dim = config.num_attention_heads * config.head_dim;
        let kv_dim = config.num_key_value_heads * config.head_dim;

        for i in 0..config.num_hidden_layers {
            let input_norm = Self::load_tensor_1d(
                tensors,
                &format!("layers.{}.input_layernorm.weight", i),
                config.hidden_size,
            )?;
            let post_attention_norm = Self::load_tensor_1d(
                tensors,
                &format!("layers.{}.post_attention_layernorm.weight", i),
                config.hidden_size,
            )?;

            let q_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.self_attn.q_proj.weight", i),
                q_dim,
                config.hidden_size,
            )?;
            let k_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.self_attn.k_proj.weight", i),
                kv_dim,
                config.hidden_size,
            )?;
            let v_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.self_attn.v_proj.weight", i),
                kv_dim,
                config.hidden_size,
            )?;
            let o_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.self_attn.o_proj.weight", i),
                config.hidden_size,
                q_dim,
            )?;

            let gate_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.mlp.gate_proj.weight", i),
                config.intermediate_size,
                config.hidden_size,
            )?;
            let up_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.mlp.up_proj.weight", i),
                config.intermediate_size,
                config.hidden_size,
            )?;
            let down_proj = Self::load_tensor_2d(
                tensors,
                &format!("layers.{}.mlp.down_proj.weight", i),
                config.hidden_size,
                config.intermediate_size,
            )?;

            layers.push(LayerWeights {
                input_norm,
                post_attention_norm,
                q_proj,
                k_proj,
                v_proj,
                o_proj,
                gate_proj,
                up_proj,
                down_proj,
            });
        }

        Ok(Self {
            embed_tokens,
            final_norm,
            lm_head,
            layers,
        })
    }

    fn load_tensor_1d(
        tensors: &SafeTensors<'_>,
        name: &str,
        expected_len: usize,
    ) -> Result<Vec<f32>, NeuralLMError> {
        let view = tensors
            .tensor(name)
            .map_err(|_| NeuralLMError::MissingTensor {
                tensor_name: name.to_string(),
            })?;
        let shape = view.shape();
        if shape != [expected_len] {
            return Err(NeuralLMError::TensorShapeMismatch {
                name: name.to_string(),
                expected: vec![expected_len],
                actual: shape.to_vec(),
            });
        }
        let data = bytemuck::cast_slice(view.data()).to_vec();
        Ok(data)
    }

    fn load_tensor_2d(
        tensors: &SafeTensors<'_>,
        name: &str,
        rows: usize,
        cols: usize,
    ) -> Result<Tensor2D, NeuralLMError> {
        let view = tensors
            .tensor(name)
            .map_err(|_| NeuralLMError::MissingTensor {
                tensor_name: name.to_string(),
            })?;
        let shape = view.shape();
        if shape != [rows, cols] {
            return Err(NeuralLMError::TensorShapeMismatch {
                name: name.to_string(),
                expected: vec![rows, cols],
                actual: shape.to_vec(),
            });
        }
        let data = bytemuck::cast_slice(view.data()).to_vec();
        Tensor2D::new(rows, cols, data)
    }
}
