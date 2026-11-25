//! 第三方词库文件解码与词条提取器
//!
//! # 设计原理
//! - **实现初衷**：负责解析各类外部异构格式词库文件（Rime YAML、搜狗细胞词库 .scel 及纯文本 .txt），
//!   提取标准化拼音词条项并输出统一的领域对象 `ImportedDictEntry`。
//! - **核心优势**：
//!   1. 按格式拆分为高内聚子模块（`rime_yaml`、`plain_text`、`scel`），每份文件职责单一；
//!   2. 严格内存边界与魔数校验，防范畸变搜狗 SCEL 二进制扩展字段越界与奇数字节截断；
//!   3. 文本格式统一采用流式逐行解码，消除大文件全量读入内存峰值。
//! - **代价与局限**：
//!   仅负责单文件的语法与二进制解码，不直接负责自适应历史记录合入与去重。

mod plain_text;
mod rime_yaml;
mod scel;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use waylan_algo::syllables::normalize_pinyin_tones;
use waylan_algo::types::ImportedDictEntry;
use waylan_core::MAX_IMPORT_FILE_BYTES;

use crate::error::DictImportError;

/// 第三方词库导入与转换器 (支持 Rime YAML / 搜狗细胞词库 .scel / 纯文本 .txt)
pub struct DictImporter;

impl DictImporter {
    /// 打开并校验待导入文件体积不超过上限（单次 `metadata` 系统调用，避免重复 stat）
    pub(crate) fn open_checked_file(path: &Path) -> Result<File, DictImportError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DictImportError::FileNotFound {
                    path: path.to_path_buf(),
                }
            } else {
                DictImportError::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?;
        let meta = file.metadata().map_err(|e| DictImportError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        if !meta.is_file() {
            return Err(DictImportError::FileNotFound {
                path: path.to_path_buf(),
            });
        }
        if meta.len() > MAX_IMPORT_FILE_BYTES {
            return Err(DictImportError::FileTooLarge {
                path: path.to_path_buf(),
                limit_bytes: MAX_IMPORT_FILE_BYTES,
                actual_bytes: meta.len(),
            });
        }
        Ok(file)
    }

    /// 零分配解析单行文本词条（支持 `\t` 分隔或空白分隔）
    #[inline]
    pub(crate) fn parse_entry_columns<'a>(
        mut cols: impl Iterator<Item = &'a str>,
    ) -> Option<ImportedDictEntry> {
        let text = cols.next()?.trim();
        let raw_pinyin = cols.next()?.trim();
        let weight = cols
            .next()
            .and_then(|w| w.trim().parse::<u32>().ok())
            .unwrap_or(100);

        if text.is_empty() {
            return None;
        }
        let pinyin = normalize_pinyin_tones(raw_pinyin);
        if pinyin.is_empty() {
            return None;
        }
        Some(ImportedDictEntry {
            text: text.to_string(),
            pinyin,
            weight,
        })
    }

    /// 自动根据文件后缀解析导入词库文件
    ///
    /// # Errors
    /// 当文件不存在、读取异常、超出大小上限或格式损坏时返回 [`DictImportError`]。
    pub fn import_file(path: &Path) -> Result<Vec<ImportedDictEntry>, DictImportError> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();

        match extension.as_str() {
            "scel" => {
                let mut file = Self::open_checked_file(path)?;
                let cap = file
                    .metadata()
                    .map(|m| m.len() as usize)
                    .unwrap_or(0)
                    .min(MAX_IMPORT_FILE_BYTES as usize);
                let mut data = Vec::with_capacity(cap);
                file.read_to_end(&mut data)
                    .map_err(|e| DictImportError::Io {
                        path: path.to_path_buf(),
                        source: e,
                    })?;
                Self::parse_sogou_scel(&data)
            }
            "yaml" | "yml" => {
                let file = Self::open_checked_file(path)?;
                rime_yaml::parse_rime_yaml_reader(BufReader::new(file), path)
            }
            "txt" => {
                let file = Self::open_checked_file(path)?;
                plain_text::parse_plain_text_reader(BufReader::new(file), path)
            }
            _ => Err(DictImportError::UnsupportedFormat {
                path: path.to_path_buf(),
                format: extension,
            }),
        }
    }

    /// 解析 Rime 词典 YAML 格式（兼容标准 `...` 表头分隔与无 `...` 标记的纯 TSV 词表）
    ///
    /// # Errors
    /// 当内容读取发生底层 I/O 异常时返回 [`DictImportError`]。
    pub fn parse_rime_yaml(content: &str) -> Result<Vec<ImportedDictEntry>, DictImportError> {
        rime_yaml::parse_rime_yaml(content)
    }

    /// 解析纯文本格式（词条 拼音 [权重] 或 词条\t拼音）
    ///
    /// # Errors
    /// 当内容读取发生底层 I/O 异常时返回 [`DictImportError`]。
    pub fn parse_plain_text(content: &str) -> Result<Vec<ImportedDictEntry>, DictImportError> {
        plain_text::parse_plain_text(content)
    }

    /// 解析搜狗细胞词库二进制格式 (.scel)
    ///
    /// # Errors
    /// 当文件长度越界、魔数头不匹配或无任何有效词条可解析时返回 [`DictImportError`]。
    pub fn parse_sogou_scel(data: &[u8]) -> Result<Vec<ImportedDictEntry>, DictImportError> {
        scel::parse_sogou_scel(data)
    }
}
