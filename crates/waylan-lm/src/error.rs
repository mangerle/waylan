use thiserror::Error;

/// 微澜本地语言模型核心错误类型
#[derive(Debug, Error)]
pub enum NeuralLMError {
    /// 配置文件或权重文件 IO 读取失败
    #[error("读取模型文件失败，路径: {path}，原因: {source}")]
    IoError {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Safetensors 格式解析失败
    #[error("解析 Safetensors 权重容器失败，原因: {0}")]
    SafeTensorsParseError(String),

    /// 缺少必需的权重张量
    #[error("模型缺少必需的权重张量: {tensor_name}")]
    MissingTensor { tensor_name: String },

    /// 权重张量形状与模型配置不匹配
    #[error("张量形状不匹配，张量名: {name}，预期: {expected:?}，实际: {actual:?}")]
    TensorShapeMismatch {
        name: String,
        expected: Vec<usize>,
        actual: Vec<usize>,
    },

    /// 分词器加载或分词编码失败
    #[error("分词器处理失败，原因: {0}")]
    TokenizerError(String),

    /// 模型配置反序列化失败
    #[error("模型配置文件反序列化失败，原因: {0}")]
    ConfigDeserializeError(String),
}
