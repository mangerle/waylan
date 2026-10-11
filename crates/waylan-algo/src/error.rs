use std::path::PathBuf;
use thiserror::Error;

/// 微澜算法库领域强类型错误定义
///
/// # 设计原理
/// - **实现初衷**：根据 AGENTS.md 规范，底层算法库必须使用 `thiserror` 定义强类型、语义详尽的领域错误，
///   严禁抛出无类型的通用字符串或泄漏不稳定的底层第三方错误。
/// - **核心优势**：调用方可针对具体错误变体进行模式匹配，且所有错误变体均包含动态上下文定位参数。
/// - **代价与局限**：跨模块传递时需显式进行错误变体转换，较通用动态错误稍增类型声明成本。
#[derive(Debug, Error)]
pub enum AlgoError {
    /// 词库文件访问或 I/O 错误
    #[error("词库文件访问失败，路径: {path}，原因: {source}")]
    DictionaryIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 二进制反序列化解析错误
    #[error("二进制词库反序列化解析失败，路径: {path}，原因: {source}")]
    DeserializeFailed {
        path: PathBuf,
        #[source]
        source: bincode::Error,
    },

    /// 算术表达式求值语法错误（V 模式）
    #[error("表达式求值失败，表达式: {expression}，详情: {reason}")]
    ExpressionEvalFailed { expression: String, reason: String },

    /// 用户词典持久化失败
    #[error("用户词典持久化失败，路径: {path}，原因: {source}")]
    UserDictPersistFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 状态机非法操作（如光标越界）
    #[error("状态机操作越界或非法，当前长度: {current_len}，目标位置: {target_pos}")]
    InvalidCursorPosition {
        current_len: usize,
        target_pos: usize,
    },

    /// 不支持的第三方词库文件格式
    #[error("不支持的词库文件格式，路径: {path}，格式: {format}")]
    UnsupportedDictFormat { path: PathBuf, format: String },

    /// 词库解析格式损坏或内容非法
    #[error("解析第三方词库失败，详情: {reason}")]
    DictParseFailed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = AlgoError::ExpressionEvalFailed {
            expression: "1/0".to_string(),
            reason: "除数不能为零".to_string(),
        };
        assert!(err.to_string().contains("除数不能为零"));
        assert!(err.to_string().contains("1/0"));

        let format_err = AlgoError::UnsupportedDictFormat {
            path: PathBuf::from("test.doc"),
            format: "doc".to_string(),
        };
        assert!(format_err.to_string().contains("不支持的词库文件格式"));

        let parse_err = AlgoError::DictParseFailed {
            reason: "数据长度不符合头结构".to_string(),
        };
        assert!(parse_err.to_string().contains("数据长度不符合头结构"));

        let cursor_err = AlgoError::InvalidCursorPosition {
            current_len: 5,
            target_pos: 10,
        };
        assert!(cursor_err.to_string().contains("状态机操作越界"));
    }
}
