use crate::error::NeuralLMError;
use std::path::Path;
use tokenizers::Tokenizer;

/// 微澜本地语言模型分词器封装
#[derive(Debug)]
pub struct LMTokenizer {
    inner: Tokenizer,
}

impl LMTokenizer {
    /// 从本地 `tokenizer.json` 文件加载分词器
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, NeuralLMError> {
        let p = path.as_ref();
        let inner = Tokenizer::from_file(p)
            .map_err(|e| NeuralLMError::TokenizerError(format!("加载分词器失败: {}", e)))?;
        Ok(Self { inner })
    }

    /// 从内存字节切片中解析加载分词器
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NeuralLMError> {
        let inner = Tokenizer::from_bytes(bytes)
            .map_err(|e| NeuralLMError::TokenizerError(format!("解析分词器字节失败: {}", e)))?;
        Ok(Self { inner })
    }

    /// 对输入文本进行分词编码，返回 Token ID 序列（包含可选的 BOS 前缀）
    pub fn encode(&self, text: &str, add_bos: bool) -> Result<Vec<u32>, NeuralLMError> {
        let encoding = self
            .inner
            .encode(text, false)
            .map_err(|e| NeuralLMError::TokenizerError(format!("分词编码失败: {}", e)))?;

        let ids = encoding.get_ids();
        if add_bos {
            let mut result = Vec::with_capacity(ids.len() + 1);
            result.push(1); // 默认 <bos> id 为 1
            result.extend_from_slice(ids);
            Ok(result)
        } else {
            Ok(ids.to_vec())
        }
    }

    /// 将 Token ID 序列解码还原为可读中文文本
    pub fn decode(
        &self,
        token_ids: &[u32],
        skip_special_tokens: bool,
    ) -> Result<String, NeuralLMError> {
        self.inner
            .decode(token_ids, skip_special_tokens)
            .map_err(|e| NeuralLMError::TokenizerError(format!("分词解码失败: {}", e)))
    }

    /// 获取分词器当前词表大小
    pub fn vocab_size(&self) -> usize {
        self.inner.get_vocab_size(true)
    }
}
