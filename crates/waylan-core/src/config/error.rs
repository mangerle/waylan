//! 微澜输入法配置系统领域错误枚举

use std::path::PathBuf;
use thiserror::Error;

/// 微澜输入法配置系统领域错误枚举
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 无法解析或定位当前用户的配置目录
    #[error("无法确定用户配置文件存储路径，请检查 HOME 或 XDG_CONFIG_HOME 环境变量")]
    ConfigDirectoryNotFound,

    /// 配置文件底层 I/O 错误
    #[error("配置文件 I/O 读写失败，路径: {path}，原因: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// TOML 配置反序列化失败（不暴露第三方 toml crate 内部错误类型）
    #[error("TOML 配置解析失败: {0}")]
    TomlParse(String),

    /// TOML 配置序列化失败（不暴露第三方 toml crate 内部错误类型）
    #[error("TOML 配置序列化失败: {0}")]
    TomlSerialize(String),

    /// 容错解析与合并失败
    #[error("TOML 容错解析失败，严格错误: {strict_err}，合并错误: {merge_err}")]
    ParseFailed {
        strict_err: String,
        merge_err: String,
    },

    /// 语法损坏错误
    #[error("TOML 语法损坏无法解析: {0}")]
    SyntaxCorrupted(String),
}
