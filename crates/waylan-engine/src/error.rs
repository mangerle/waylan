//! 微澜交互运行时与领域服务强类型错误定义
//!
//! # 设计原理
//! - **实现初衷**：根据 AGENTS.md 分层错误处理准则，将外部词库文件校验、解码与合入过程中的错误
//!   从底层算法库中彻底剥离，在引擎与领域服务层建立专属、高内聚的强类型错误枚举。
//! - **核心优势**：
//!   1. 错误变体携带完整上下文（路径、文件大小、具体原因等），排查定位清晰；
//!   2. 解除上层 UI/CLI 与底层算法错误之间的越级耦合，防止算法层错误类型被业务污染；
//!   3. 零技术债与逻辑复制，上层消费者（GUI、CLI）可直接模式匹配或友好展示。
//! - **代价与局限**：跨模块向上传播时需在边界处进行领域错误包装。

use std::path::PathBuf;
use thiserror::Error;

/// 外部词库解析与导入领域强类型错误枚举
#[derive(Debug, Error)]
pub enum DictImportError {
    /// 词库文件不存在或路径无效
    #[error("指定的词库文件不存在或路径无效: {path}")]
    FileNotFound {
        /// 目标文件路径
        path: PathBuf,
    },

    /// 词库文件体积超出安全上限
    #[error(
        "词库文件体积超过安全上限（{limit_bytes} 字节）: {path}（当前体积: {actual_bytes} 字节）"
    )]
    FileTooLarge {
        /// 目标文件路径
        path: PathBuf,
        /// 允许的最大字节上限
        limit_bytes: u64,
        /// 实际检测到的文件字节数
        actual_bytes: u64,
    },

    /// 词库文件读取或写入 I/O 异常
    #[error("词库文件 I/O 操作失败，路径: {path}，原因: {source}")]
    Io {
        /// 目标文件路径
        path: PathBuf,
        /// 底层 I/O 错误
        #[source]
        source: std::io::Error,
    },

    /// 不支持的第三方词库文件格式
    #[error("不支持的词库文件格式，路径: {path}，格式扩展名: {format}")]
    UnsupportedFormat {
        /// 目标文件路径
        path: PathBuf,
        /// 未知或不受支持的格式
        format: String,
    },

    /// 词库内容损坏或解析格式错误
    #[error("解析第三方词库失败，详情: {reason}")]
    ParseFailed {
        /// 解析失败详细原因
        reason: String,
    },

    /// 读取现有用户词库失败，为避免覆盖历史数据而中止导入
    #[error("读取现有用户词库失败，路径: {path}，原因: {reason}；已中止导入以保护历史数据")]
    UserDictLoadFailed {
        /// 用户词库路径
        path: PathBuf,
        /// 读取或解析失败原因
        reason: String,
    },

    /// 自适应词库模型更新或持久化保存失败
    #[error("保存用户词库失败，路径: {path}，原因: {reason}")]
    SaveFailed {
        /// 目标文件路径
        path: PathBuf,
        /// 保存失败原因描述
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dict_import_error_display() {
        let err = DictImportError::FileNotFound {
            path: PathBuf::from("/path/to/dict.txt"),
        };
        assert!(err.to_string().contains("指定的词库文件不存在"));
        assert!(err.to_string().contains("/path/to/dict.txt"));

        let limit_err = DictImportError::FileTooLarge {
            path: PathBuf::from("/path/to/huge.scel"),
            limit_bytes: 1024,
            actual_bytes: 2048,
        };
        assert!(limit_err.to_string().contains("词库文件体积超过安全上限"));

        let format_err = DictImportError::UnsupportedFormat {
            path: PathBuf::from("/path/to/doc.pdf"),
            format: "pdf".to_string(),
        };
        assert!(format_err.to_string().contains("不支持的词库文件格式"));

        let parse_err = DictImportError::ParseFailed {
            reason: "魔数不匹配".to_string(),
        };
        assert!(parse_err.to_string().contains("解析第三方词库失败"));

        let save_err = DictImportError::SaveFailed {
            path: PathBuf::from("/path/to/wal.bin"),
            reason: "磁盘写满".to_string(),
        };
        assert!(save_err.to_string().contains("保存用户词库失败"));
    }
}
