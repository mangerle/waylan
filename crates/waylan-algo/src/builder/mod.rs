//! 基础词典与辅助词库离线二进制压缩构建工具
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供离线编译原始文本词表为超紧凑二进制二分查找词典（`base.dict.bin` / `aux.dict.bin`）的编译工具。
//! - **核心优势**：通过 `dict-builder` 特性独立门控，在打字引擎运行时完全不参与编译，将繁重构建逻辑与只读运行时彻底隔离。
//! - **代价与局限**：仅在 CLI 词库管理或离线预构建时按需启用。

use crate::custom_phrase::AuxiliaryDict;
use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub mod compact;
pub mod compact_writer;
pub mod dat_builder;
pub use compact::CompactDictBuilder;
pub use compact_writer::{write_compact_file, CompactWritePayload};
pub use dat_builder::build_dat;

/// 词库编译统计指标
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStats {
    /// 处理的原始文本总行数
    pub total_lines: usize,
    /// 有效词条记录总数
    pub total_words: usize,
    /// 唯一拼音键总数
    pub total_keys: usize,
    /// 打包编译耗时（毫秒）
    pub elapsed_ms: u64,
}

pub use crate::syllables::normalize_pinyin_tones;

#[derive(Debug, Default)]
pub struct DictBuilder;

impl DictBuilder {
    /// 从多个文本词库源编译生成向后兼容的传统 bincode 二进制词库文件
    ///
    /// # Errors
    /// 当读取源文件或创建输出文件失败，或序列化输出失败时返回 [`AlgoError`]。
    pub fn build_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        output_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        let start = Instant::now();
        let mut dict_map: HashMap<String, Vec<(String, u32)>> = HashMap::new();
        let mut total_lines: usize = 0;
        let mut total_words: usize = 0;

        for in_path in input_paths {
            let path_ref = in_path.as_ref();
            let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
            let reader = BufReader::new(file);

            for line_res in reader.lines() {
                total_lines = total_lines.saturating_add(1);
                let line = match line_res {
                    Ok(l) => l,
                    Err(_) => continue,
                };

                let trimmed = line.trim();
                if trimmed.is_empty()
                    || trimmed.starts_with('#')
                    || trimmed.starts_with("---")
                    || trimmed.starts_with("...")
                    || trimmed.starts_with("name:")
                    || trimmed.starts_with("version:")
                    || trimmed.starts_with("sort:")
                {
                    continue;
                }

                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.len() >= 2 {
                    let word = parts[0].trim().to_string();
                    let pinyin = normalize_pinyin_tones(parts[1].trim());
                    let score: u32 = if parts.len() >= 3 {
                        parts[2].trim().parse().unwrap_or(1000)
                    } else {
                        1000
                    };

                    dict_map.entry(pinyin).or_default().push((word, score));
                    total_words = total_words.saturating_add(1);
                }
            }
        }

        for entries in dict_map.values_mut() {
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        }

        let out_ref = output_path.as_ref();
        let out_file = File::create(out_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(out_ref),
            source: e,
        })?;
        let writer = BufWriter::new(out_file);
        bincode::serialize_into(writer, &dict_map).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(out_ref),
            source: std::io::Error::other(e.to_string()),
        })?;

        let total_keys = dict_map.len();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(BuildStats {
            total_lines,
            total_words,
            total_keys,
            elapsed_ms,
        })
    }

    /// 构建包含英文补全与表情符号的辅助词典二进制文件
    ///
    /// # Errors
    /// 当写出辅助词典持久化文件失败时返回 [`AlgoError`]。
    pub fn build_auxiliary_dict<P: AsRef<Path>, Q: AsRef<Path>, R: AsRef<Path>>(
        en_paths: &[P],
        sym_paths: &[Q],
        output_path: R,
    ) -> Result<AuxiliaryDict, AlgoError> {
        let mut dict = AuxiliaryDict::with_default_data();
        let mut seen_en: HashSet<String> = dict
            .english_words
            .iter()
            .map(|(w, _)| w.to_lowercase())
            .collect();

        // 若提供了外部英文词库，增量解析合并
        for path in en_paths {
            let p_ref = path.as_ref();
            if p_ref.exists() {
                if let Ok(file) = File::open(p_ref) {
                    let reader = BufReader::new(file);
                    for line in reader.lines().map_while(Result::ok) {
                        let trimmed = line.trim();
                        if trimmed.is_empty()
                            || trimmed.starts_with('#')
                            || trimmed.starts_with("---")
                            || trimmed.starts_with("...")
                            || trimmed.starts_with("name:")
                            || trimmed.starts_with("version:")
                            || trimmed.starts_with("sort:")
                        {
                            continue;
                        }
                        let parts: Vec<&str> = trimmed.split('\t').collect();
                        if !parts.is_empty() {
                            let word = parts[0].trim();
                            if word.is_empty() {
                                continue;
                            }
                            let score: u32 = if parts.len() >= 3 {
                                parts[2].trim().parse().unwrap_or(50_000)
                            } else if parts.len() >= 2 {
                                parts[1].trim().parse().unwrap_or(50_000)
                            } else {
                                50_000
                            };
                            if seen_en.insert(word.to_lowercase()) {
                                dict.english_words.push((word.to_string(), score));
                            }
                        }
                    }
                }
            }
        }

        // 若提供了外部符号词库（Emoji/颜文字/特殊符号），增量解析合并
        for path in sym_paths {
            let p_ref = path.as_ref();
            if p_ref.exists() {
                if let Ok(file) = File::open(p_ref) {
                    let reader = BufReader::new(file);
                    for line in reader.lines().map_while(Result::ok) {
                        let trimmed = line.trim();
                        if trimmed.is_empty()
                            || trimmed.starts_with('#')
                            || trimmed.starts_with("---")
                            || trimmed.starts_with("...")
                            || trimmed.starts_with("name:")
                            || trimmed.starts_with("version:")
                            || trimmed.starts_with("sort:")
                        {
                            continue;
                        }
                        let parts: Vec<&str> = trimmed.split('\t').collect();
                        if parts.len() >= 2 {
                            let (key, sym, desc) = if parts[0]
                                .chars()
                                .any(|c| !c.is_ascii_alphanumeric() && c != '\'' && c != '_')
                            {
                                let py = parts[1].trim().to_lowercase().replace(' ', "");
                                let emoji_or_sym = parts[0].trim().to_string();
                                let d = if py == "vv" {
                                    "颜文字".to_string()
                                } else {
                                    "表情".to_string()
                                };
                                (py, emoji_or_sym, d)
                            } else {
                                let k = parts[0].trim().to_lowercase();
                                let s = parts[1].trim().to_string();
                                let d = if parts.len() >= 3 {
                                    parts[2].trim().to_string()
                                } else {
                                    "符号".to_string()
                                };
                                (k, s, d)
                            };

                            if !key.is_empty() && !sym.is_empty() {
                                dict.symbols.push((key, sym, desc));
                            }
                        }
                    }
                }
            }
        }

        dict.save_to_file(output_path.as_ref())?;
        Ok(dict)
    }

    /// 用户导入个人自定义词库文本
    pub fn import_user_text_dict<P: AsRef<Path>>(
        file_path: P,
        user_model: &mut UserAdaptiveModel,
        current_timestamp: u64,
    ) -> Result<usize, AlgoError> {
        let path_ref = file_path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let reader = BufReader::new(file);
        let mut imported_count: usize = 0;

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = trimmed.split('\t').collect();
            if parts.len() >= 2 {
                let word = parts[0].trim();
                let pinyin = normalize_pinyin_tones(parts[1].trim());
                let repeat: usize = if parts.len() >= 3 {
                    parts[2].trim().parse().unwrap_or(1).min(10)
                } else {
                    1
                };

                for _ in 0..repeat {
                    user_model.record_commit(word, &pinyin, current_timestamp);
                }
                imported_count = imported_count.saturating_add(1);
            }
        }

        Ok(imported_count)
    }
}

impl DictBuilder {
    #[inline]
    pub fn build_compact_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        bin_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        self::compact::CompactDictBuilder::build_compact_binary_dict(input_paths, bin_path)
    }
}

#[cfg(test)]
mod tests;
