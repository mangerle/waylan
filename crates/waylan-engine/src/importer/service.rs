//! 统一外部词库导入领域服务流水线
//!
//! # 设计原理
//! - **实现初衷**：根据架构规范，将原散落在 `waylan-settings` 与 `waylan-daemon` CLI 中的词库校验、
//!   去重过滤、模型写入与磁盘持久化流水线统一下沉至底层领域服务层。
//! - **核心优势**：
//!   1. 统一 64MB 安全文件体积与合法路径预检；
//!   2. 统一 CLI 与图形化设置中心的词库合入行为，消除逻辑分歧；
//!   3. 全中文领域强类型错误闭环，杜绝松散字符串报错。
//! - **代价与局限**：
//!   合入自适应词库时会触发 WAL 文件的压实（Compaction），超大词库首次合入需要毫秒级磁盘 I/O。

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use waylan_algo::custom_phrase::AuxiliaryDict;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_core::MAX_IMPORT_FILE_BYTES;

use super::parser::DictImporter;
use crate::error::DictImportError;

/// 拼音词典导入结果报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictImportReport {
    /// 本次解析并成功合入的新词目数量
    pub imported_count: usize,
    /// 合入后当前自适应词库的条目总数
    pub total_entries: usize,
}

/// 辅助词典（英文/符号）导入结果报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxImportReport {
    /// 本次成功新增的条目数量
    pub added_count: usize,
    /// 新增后当前词典的总条目数量
    pub total_count: usize,
}

/// 统一词库导入领域服务
pub struct DictImportService;

impl DictImportService {
    /// 校验源词库文件是否存在且未超过 64MB 体积安全上限
    fn validate_source_file(path: &Path) -> Result<(), DictImportError> {
        if !path.is_file() {
            return Err(DictImportError::FileNotFound {
                path: path.to_path_buf(),
            });
        }
        let meta = fs::metadata(path).map_err(|e| DictImportError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        if meta.len() > MAX_IMPORT_FILE_BYTES {
            return Err(DictImportError::FileTooLarge {
                path: path.to_path_buf(),
                limit_bytes: MAX_IMPORT_FILE_BYTES,
                actual_bytes: meta.len(),
            });
        }
        Ok(())
    }

    /// 解析外部拼音词库文件并增量合入用户自适应模型 WAL
    ///
    /// # Errors
    /// 当文件不存在、体积超出 64MB、格式无法解析或 WAL 写入失败时返回 [`DictImportError`]。
    pub fn import_pinyin_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        source_path: P,
        user_dict_path: Q,
    ) -> Result<DictImportReport, DictImportError> {
        let src = source_path.as_ref();
        Self::validate_source_file(src)?;

        let entries = DictImporter::import_file(src)?;
        let imported_count = entries.len();

        let dst = user_dict_path.as_ref();
        let mut user_model = UserAdaptiveModel::load_or_replay_wal(dst).map_err(|e| {
            DictImportError::UserDictLoadFailed {
                path: dst.to_path_buf(),
                reason: e.to_string(),
            }
        })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_700_000_000);

        for entry in &entries {
            user_model.record_commit(&entry.text, &entry.pinyin, now);
        }

        user_model
            .compact_wal(dst)
            .map_err(|e| DictImportError::SaveFailed {
                path: dst.to_path_buf(),
                reason: e.to_string(),
            })?;

        Ok(DictImportReport {
            imported_count,
            total_entries: user_model.total_entries(),
        })
    }

    /// 解析外部英文词表并去重合入辅助词库
    ///
    /// # Errors
    /// 当文件不存在、体积超出上限或保存失败时返回 [`DictImportError`]。
    pub fn import_english_dict<P: AsRef<Path>>(
        source_path: P,
        aux: &mut AuxiliaryDict,
        aux_save_path: Option<&Path>,
    ) -> Result<AuxImportReport, DictImportError> {
        let src = source_path.as_ref();
        Self::validate_source_file(src)?;

        let content = fs::read_to_string(src).map_err(|e| DictImportError::Io {
            path: src.to_path_buf(),
            source: e,
        })?;

        let mut existing: HashSet<String> = aux
            .english_words
            .iter()
            .map(|(w, _)| w.to_lowercase())
            .collect();

        let added_count = Self::parse_english_words(&content, &mut existing, aux);

        Self::save_aux_dict(aux, aux_save_path)?;

        Ok(AuxImportReport {
            added_count,
            total_count: aux.english_words.len(),
        })
    }

    /// 解析外部符号映射表并去重合入辅助词库
    ///
    /// # Errors
    /// 当文件不存在、体积超出上限或保存失败时返回 [`DictImportError`]。
    pub fn import_symbols_dict<P: AsRef<Path>>(
        source_path: P,
        aux: &mut AuxiliaryDict,
        aux_save_path: Option<&Path>,
    ) -> Result<AuxImportReport, DictImportError> {
        let src = source_path.as_ref();
        Self::validate_source_file(src)?;

        let content = fs::read_to_string(src).map_err(|e| DictImportError::Io {
            path: src.to_path_buf(),
            source: e,
        })?;

        let mut existing: HashSet<(String, String)> = aux
            .symbols
            .iter()
            .map(|(k, s, _)| (k.clone(), s.clone()))
            .collect();

        let added_count = Self::parse_symbols(&content, &mut existing, aux);

        Self::save_aux_dict(aux, aux_save_path)?;

        Ok(AuxImportReport {
            added_count,
            total_count: aux.symbols.len(),
        })
    }

    /// 解析英文词表文本并去重合入辅助词库，返回本次新增条目数
    ///
    /// # 设计原理
    /// - 词表可能带有 YAML 头部（`---` 与 `...` 之间），该区间一律跳过；
    ///   仅接受纯 ASCII 字母与连字符构成的单词，杜绝符号与数字污染英文词库。
    fn parse_english_words(
        content: &str,
        existing: &mut HashSet<String>,
        aux: &mut AuxiliaryDict,
    ) -> usize {
        let mut added_count = 0;
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
                added_count += 1;
            }
        }

        added_count
    }

    /// 解析符号映射表文本并去重合入辅助词库，返回本次新增条目数
    ///
    /// # 设计原理
    /// - 符号表以制表符分隔"快捷码 / 符号 / 描述"三列，缺省描述时统一回退为"外部符号"；
    ///   以"快捷码 + 符号"二元组作为去重键，允许同一快捷码对应多个不同符号。
    fn parse_symbols(
        content: &str,
        existing: &mut HashSet<(String, String)>,
        aux: &mut AuxiliaryDict,
    ) -> usize {
        let mut added_count = 0;

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
                    added_count += 1;
                }
            }
        }

        added_count
    }

    /// 将辅助词库落盘到指定路径（未指定路径时仅在内存中生效）
    fn save_aux_dict(
        aux: &AuxiliaryDict,
        aux_save_path: Option<&Path>,
    ) -> Result<(), DictImportError> {
        if let Some(save_path) = aux_save_path {
            aux.save_to_file(save_path)
                .map_err(|e| DictImportError::SaveFailed {
                    path: save_path.to_path_buf(),
                    reason: e.to_string(),
                })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_import_nonexistent_file() {
        let res =
            DictImportService::import_pinyin_dict("/nonexistent/path/pinyin.txt", "/tmp/wal.bin");
        assert!(matches!(res, Err(DictImportError::FileNotFound { .. })));

        let mut aux = AuxiliaryDict::default();
        let res_en =
            DictImportService::import_english_dict("/nonexistent/path/en.txt", &mut aux, None);
        assert!(matches!(res_en, Err(DictImportError::FileNotFound { .. })));

        let res_sym =
            DictImportService::import_symbols_dict("/nonexistent/path/sym.txt", &mut aux, None);
        assert!(matches!(res_sym, Err(DictImportError::FileNotFound { .. })));
    }

    #[test]
    fn test_import_english_and_symbols_service() {
        let mut aux = AuxiliaryDict::default();

        let temp_en = NamedTempFile::new().unwrap();
        fs::write(
            temp_en.path(),
            "---\nname: english\n...\nhello\t100\nworld\t200\nhello\t300\n# comment\n",
        )
        .unwrap();

        let res_en =
            DictImportService::import_english_dict(temp_en.path(), &mut aux, None).unwrap();
        assert_eq!(res_en.added_count, 2);
        assert_eq!(res_en.total_count, 2);

        let temp_sym = NamedTempFile::new().unwrap();
        fs::write(
            temp_sym.path(),
            "sj\t(｡･ω･｡)\t颜文字\nsj\t(｡･ω･｡)\t重复项\nhh\t哈哈\n",
        )
        .unwrap();

        let res_sym =
            DictImportService::import_symbols_dict(temp_sym.path(), &mut aux, None).unwrap();
        assert_eq!(res_sym.added_count, 2);
        assert_eq!(res_sym.total_count, 2);
    }
}

#[cfg(test)]
mod data_safety_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_import_does_not_overwrite_corrupt_user_dict() {
        let dir = tempdir().expect("创建临时目录失败");
        let src = dir.path().join("input.txt");
        fs::write(&src, "测试\tce'shi\t100\n").expect("写入测试词库失败");
        let user_dict = dir.path().join("user_dict.bin");
        let corrupt = b"not-a-valid-waylan-wal-file";
        fs::write(&user_dict, corrupt).expect("写入损坏词库失败");

        let result = DictImportService::import_pinyin_dict(&src, &user_dict);
        assert!(
            matches!(result, Err(DictImportError::UserDictLoadFailed { .. })),
            "损坏的用户词库必须中止导入并返回明确错误"
        );
        assert_eq!(
            fs::read(&user_dict).expect("读取原文件失败"),
            corrupt,
            "导入失败时不得覆盖原始用户词库"
        );
    }
}
