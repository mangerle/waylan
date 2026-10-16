//! 外部词库与字典数据导入模块
//!
//! # 设计原理
//! - **实现初衷**：为用户提供外部拼音词库、英文词库及特殊符号表的增量导入与合入机制。
//! - **核心优势**：
//!   1. 统一安全防御机制，校验 64MB 文件体积上限，杜绝内存爆满风险；
//!   2. 支持自动去重与增量合入，避免污染现有自适应词库与预设字典；
//!   3. 支持 WAL 日志紧凑化写入，确保断电或异常退出时数据一致性。
//! - **代价与局限**：
//!   一次性同步解析文本行，超大文件导入时存在短暂的 I/O 处理耗时。

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use waylan_algo::dict_importer::DictImporter;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::AppConfig;

/// 外部导入文件体积最大允许上限（64 MB），防止异常大文件导致内存暴涨
pub const MAX_IMPORT_FILE_BYTES: u64 = 64 * 1024 * 1024;

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

/// 解析并合入外部拼音词库
///
/// # Errors
/// 当文件不存在、体积超限、解析格式错误或写入磁盘失败时返回 [`ImportError`]。
pub fn import_pinyin_dict_file(path_str: &str) -> Result<String, ImportError> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err(ImportError::FileNotFound {
            path: path_str.to_string(),
        });
    }
    let meta = fs::metadata(path).map_err(|source| ImportError::IoError {
        path: path_str.to_string(),
        source,
    })?;
    if meta.len() > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge {
            path: path_str.to_string(),
            limit_bytes: MAX_IMPORT_FILE_BYTES,
            actual_bytes: meta.len(),
        });
    }

    let entries = DictImporter::import_file(path).map_err(|e| ImportError::ParseError {
        path: path_str.to_string(),
        message: e.to_string(),
    })?;
    let count = entries.len();
    let user_dict_path = AppConfig::user_dict_path();
    let mut user_model = UserAdaptiveModel::load_or_replay_wal(&user_dict_path).unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_000);
    for entry in &entries {
        user_model.record_commit(&entry.text, &entry.pinyin, now);
    }
    user_model
        .compact_wal(&user_dict_path)
        .map_err(|e| ImportError::SaveError {
            path: user_dict_path.to_string_lossy().to_string(),
            message: e.to_string(),
        })?;

    Ok(format!(
        "成功合入 {} 条词目，当前自适应词库总词量: {} 条",
        count,
        user_model.total_entries()
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
    if !path.is_file() {
        return Err(ImportError::FileNotFound {
            path: path_str.to_string(),
        });
    }
    let meta = fs::metadata(path).map_err(|source| ImportError::IoError {
        path: path_str.to_string(),
        source,
    })?;
    if meta.len() > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge {
            path: path_str.to_string(),
            limit_bytes: MAX_IMPORT_FILE_BYTES,
            actual_bytes: meta.len(),
        });
    }

    let content = fs::read_to_string(path).map_err(|source| ImportError::IoError {
        path: path_str.to_string(),
        source,
    })?;
    let mut existing: HashSet<String> = aux
        .english_words
        .iter()
        .map(|(w, _)| w.to_lowercase())
        .collect();
    let mut added = 0;
    let mut in_yaml_header = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            in_yaml_header = true;
            continue;
        }
        if trimmed == "..." {
            in_yaml_header = false;
            continue;
        }
        if in_yaml_header || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let word = trimmed.split('\t').next().unwrap_or("").trim();
        if !word.is_empty()
            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
            && existing.insert(word.to_lowercase())
        {
            aux.english_words.push((word.to_string(), 50_000));
            added += 1;
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| ImportError::SaveError {
            path: aux_path.to_string_lossy().to_string(),
            message: e.to_string(),
        })?;

    Ok((
        format!(
            "成功导入 {} 个英文单词，当前单词库总数: {}",
            added,
            aux.english_words.len()
        ),
        aux.english_words.len(),
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
    if !path.is_file() {
        return Err(ImportError::FileNotFound {
            path: path_str.to_string(),
        });
    }
    let meta = fs::metadata(path).map_err(|source| ImportError::IoError {
        path: path_str.to_string(),
        source,
    })?;
    if meta.len() > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge {
            path: path_str.to_string(),
            limit_bytes: MAX_IMPORT_FILE_BYTES,
            actual_bytes: meta.len(),
        });
    }

    let content = fs::read_to_string(path).map_err(|source| ImportError::IoError {
        path: path_str.to_string(),
        source,
    })?;
    let mut existing: HashSet<(String, String)> = aux
        .symbols
        .iter()
        .map(|(k, s, _)| (k.clone(), s.clone()))
        .collect();
    let mut added = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let key = parts[0].trim().to_string();
            let sym = parts[1].trim().to_string();
            let desc = if parts.len() >= 3 {
                parts[2].trim().to_string()
            } else {
                "外部符号".to_string()
            };
            if existing.insert((key.clone(), sym.clone())) {
                aux.symbols.push((key, sym, desc));
                added += 1;
            }
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| ImportError::SaveError {
            path: aux_path.to_string_lossy().to_string(),
            message: e.to_string(),
        })?;

    Ok((
        format!(
            "成功导入 {} 组符号联想项，当前符号总数: {}",
            added,
            aux.symbols.len()
        ),
        aux.symbols.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
