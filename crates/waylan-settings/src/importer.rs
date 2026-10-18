//! 外部词库与字典数据导入前端适配模块
//!
//! # 设计原理
//! - **实现初衷**：为设置界面提供用户外部拼音词库、英文词库及特殊符号表的增量导入入口。
//! - **核心优势**：
//!   1. 彻底将词库校验、解析与合入逻辑下沉委托至 `waylan-algo::dict_importer::DictImportService`；
//!   2. 本模块仅保留 UI 交互所需的状态报告字符串拼装与界面级错误映射；
//!   3. 零技术债与逻辑复制，确保 CLI 与 GUI 导入行为 100% 严格一致。
//! - **代价与局限**：
//!   界面调用时同步执行文件解析与持久化，超大文件导入时建议通过后台工作线程触发。

use std::path::Path;
use thiserror::Error;
use waylan_algo::dict_importer::DictImportService;
use waylan_algo::error::AlgoError;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::AppConfig;
pub use waylan_core::MAX_IMPORT_FILE_BYTES;

/// 外部词库与字典数据导入错误枚举
#[derive(Debug, Error)]
pub enum ImportError {
    /// 词库文件不存在或路径无效
    #[error("指定的词库文件不存在或路径无效: {path}")]
    FileNotFound {
        /// 目标文件路径
        path: String,
    },

    /// 词库文件体积超出安全上限
    #[error(
        "词库文件体积超过安全上限（{limit_bytes} 字节）: {path}（当前体积: {actual_bytes} 字节）"
    )]
    FileTooLarge {
        /// 目标文件路径
        path: String,
        /// 允许的最大字节上限
        limit_bytes: u64,
        /// 实际检测到的文件字节数
        actual_bytes: u64,
    },

    /// 文件读取或写入 I/O 异常
    #[error("词库文件 I/O 操作失败，路径: {path}，原因: {source}")]
    IoError {
        /// 目标文件路径
        path: String,
        /// 底层 I/O 错误
        #[source]
        source: std::io::Error,
    },

    /// 词库解析失败
    #[error("解析词库文件失败，路径: {path}，原因: {message}")]
    ParseError {
        /// 目标文件路径
        path: String,
        /// 解析失败原因描述
        message: String,
    },

    /// 持久化保存失败
    #[error("保存用户词库失败，路径: {path}，原因: {message}")]
    SaveError {
        /// 目标文件路径
        path: String,
        /// 保存失败原因描述
        message: String,
    },
}

/// 将底层算法库领域错误映射为图形界面使用的导入错误
fn map_algo_error(err: AlgoError, fallback_path: &str) -> ImportError {
    match err {
        AlgoError::FileNotFound { path } => ImportError::FileNotFound {
            path: path.to_string_lossy().to_string(),
        },
        AlgoError::FileTooLarge {
            path,
            limit_bytes,
            actual_bytes,
        } => ImportError::FileTooLarge {
            path: path.to_string_lossy().to_string(),
            limit_bytes,
            actual_bytes,
        },
        AlgoError::DictionaryIo { path, source } => ImportError::IoError {
            path: path.to_string_lossy().to_string(),
            source,
        },
        AlgoError::UserDictPersistFailed { path, source } => ImportError::SaveError {
            path: path.to_string_lossy().to_string(),
            message: source.to_string(),
        },
        AlgoError::DictParseFailed { reason } | AlgoError::DeserializeFailed { reason, .. } => {
            ImportError::ParseError {
                path: fallback_path.to_string(),
                message: reason,
            }
        }
        other => ImportError::ParseError {
            path: fallback_path.to_string(),
            message: other.to_string(),
        },
    }
}

/// 解析并合入外部拼音词库
///
/// # Errors
/// 当文件不存在、体积超限、解析格式错误或写入磁盘失败时返回 [`ImportError`]。
pub fn import_pinyin_dict_file(path_str: &str) -> Result<String, ImportError> {
    let path = Path::new(path_str.trim());
    let user_dict_path = AppConfig::user_dict_path();
    let report = DictImportService::import_pinyin_dict(path, &user_dict_path)
        .map_err(|e| map_algo_error(e, path_str))?;

    Ok(format!(
        "成功合入 {} 条词目，当前自适应词库总词量: {} 条",
        report.imported_count, report.total_entries
    ))
}

/// 解析并合入外部英文单词库
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回 [`ImportError`]。
pub fn import_english_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), ImportError> {
    let path = Path::new(path_str.trim());
    let aux_path = AppConfig::user_aux_dict_path();
    let report = DictImportService::import_english_dict(path, aux, Some(&aux_path))
        .map_err(|e| map_algo_error(e, path_str))?;

    Ok((
        format!(
            "成功导入 {} 个英文单词，当前单词库总数: {}",
            report.added_count, report.total_count
        ),
        report.total_count,
    ))
}

/// 解析并合入外部符号映射表
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回 [`ImportError`]。
pub fn import_symbols_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), ImportError> {
    let path = Path::new(path_str.trim());
    let aux_path = AppConfig::user_aux_dict_path();
    let report = DictImportService::import_symbols_dict(path, aux, Some(&aux_path))
        .map_err(|e| map_algo_error(e, path_str))?;

    Ok((
        format!(
            "成功导入 {} 组符号联想项，当前符号总数: {}",
            report.added_count, report.total_count
        ),
        report.total_count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_import_nonexistent_file() {
        let res = import_pinyin_dict_file("/path/not/exist/foo.txt");
        assert!(matches!(res, Err(ImportError::FileNotFound { .. })));

        let mut aux = AuxiliaryDict::default();
        let res_en = import_english_dict_file("/path/not/exist/en.txt", &mut aux);
        assert!(matches!(res_en, Err(ImportError::FileNotFound { .. })));

        let res_sym = import_symbols_dict_file("/path/not/exist/sym.txt", &mut aux);
        assert!(matches!(res_sym, Err(ImportError::FileNotFound { .. })));
    }

    #[test]
    fn test_import_english_and_symbols_flow() {
        let mut aux = AuxiliaryDict::default();

        let temp_en = NamedTempFile::new().unwrap();
        fs::write(
            temp_en.path(),
            "---\nname: english\n...\nhello\t100\nworld\t200\nhello\t300\n# comment\n",
        )
        .unwrap();

        let (msg, count) =
            import_english_dict_file(&temp_en.path().to_string_lossy(), &mut aux).unwrap();
        assert_eq!(count, 2);
        assert!(msg.contains("成功导入 2 个英文单词"));

        let temp_sym = NamedTempFile::new().unwrap();
        fs::write(
            temp_sym.path(),
            "sj\t(｡･ω･｡)\t颜文字\nsj\t(｡･ω･｡)\t重复项\nhh\t哈哈\n",
        )
        .unwrap();

        let (msg_sym, count_sym) =
            import_symbols_dict_file(&temp_sym.path().to_string_lossy(), &mut aux).unwrap();
        assert_eq!(count_sym, 2);
        assert!(msg_sym.contains("成功导入 2 组符号联想项"));
    }
}
