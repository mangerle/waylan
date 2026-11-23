//! 基础词典与辅助词库离线二进制压缩构建工具
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供离线编译原始文本词表为超紧凑二进制二分查找词典（`base.dict.bin` / `aux.dict.bin`）的编译工具。
//! - **核心优势**：通过 `dict-builder` 特性独立门控，在打字引擎运行时完全不参与编译，将繁重构建逻辑与只读运行时彻底隔离。
//! - **代价与局限**：仅在 CLI 词库管理或离线预构建时按需启用。

use crate::custom_phrase::AuxiliaryDict;
use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub mod compact;
pub use crate::compact_writer::{CompactWritePayload, write_compact_file};
pub use crate::dat_builder::build_dat;
pub use compact::CompactDictBuilder;

/// 词条缺失词频或词频字段损坏时的保守默认值（避免误抬高至高频区）
pub(crate) const DEFAULT_MISSING_SCORE: u32 = 20;

/// 解析文本词典条目中的词频字段
///
/// - 缺失第 3 列或第 3 列为空/非数字时，返回保守低分 [`DEFAULT_MISSING_SCORE`]；
/// - 显式为负数（如 `-1`）时返回 `None`，表示该词条应当跳过，避免被误解析成高分。
pub(crate) fn parse_dict_entry_score(parts: &[&str]) -> Option<u32> {
    let Some(raw) = parts.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return Some(DEFAULT_MISSING_SCORE);
    };
    match raw.parse::<i64>() {
        Ok(val) if val < 0 => None,
        Ok(val) => Some(val.min(i64::from(u32::MAX)) as u32),
        Err(_) => Some(DEFAULT_MISSING_SCORE),
    }
}

/// 词库编译过滤与索引瘦身配置
#[derive(Debug, Clone, Copy, Default)]
pub struct DictBuildOptions {
    /// 2~3 字词条最低保留词频（0 表示不限制）
    pub min_phrase_score: u32,
    /// 4 字及以上长词条最低保留词频（0 表示不限制）
    pub min_long_phrase_score: u32,
    /// 是否剔除词频为 0 的无效条目
    pub drop_zero_score: bool,
    /// 简拼缩写索引最大字母长度（0 表示不限制）
    pub max_acronym_len: usize,
    /// 简拼缩写索引最低保留词频（0 表示沿用默认阈值）
    pub min_acronym_score: u32,
}

impl DictBuildOptions {
    /// 返回面向生产主词库的清洗与瘦身配置
    ///
    /// 过滤零频词、低频碎片拼接词以及 4 字以上低频长词，并收缩冗余长简拼键空间。
    pub const fn production() -> Self {
        Self {
            min_phrase_score: 10,
            min_long_phrase_score: 20,
            drop_zero_score: true,
            max_acronym_len: 4,
            min_acronym_score: 500,
        }
    }

    /// 判定给定词条及词频是否满足保留条件
    #[inline]
    pub(crate) fn should_keep_entry(&self, word: &str, score: u32) -> bool {
        if self.drop_zero_score && (score == 0 || has_noise_punctuation(word)) {
            return false;
        }
        let char_len = word.chars().count();
        if char_len >= 4 && self.min_long_phrase_score > 0 && score < self.min_long_phrase_score {
            return false;
        }
        if (2..=3).contains(&char_len) && self.min_phrase_score > 0 && score < self.min_phrase_score
        {
            return false;
        }
        true
    }
}

/// 检测词条中是否混入破折号、标点等语料噪声符号
#[inline]
fn has_noise_punctuation(word: &str) -> bool {
    word.chars().any(|ch| {
        ch.is_ascii_punctuation()
            || matches!(
                ch,
                '—' | '…' | '～' | '、' | '，' | '。' | '：' | '；' | '！' | '？'
            )
    })
}

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

/// 离线词库构建器入口（仅在启用 `dict-builder` 特性时编译）
#[derive(Debug, Default)]
pub struct DictBuilder;

impl DictBuilder {
    /// 判定文本词库行是否为注释、空行或词库元信息头（此类行不参与词条编译）
    fn is_skippable_dict_line(trimmed: &str) -> bool {
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
            || trimmed.starts_with("name:")
            || trimmed.starts_with("version:")
            || trimmed.starts_with("sort:")
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
        Self::merge_external_english_words(en_paths, &mut dict, &mut seen_en);

        // 若提供了外部符号词库（Emoji/颜文字/特殊符号），增量解析合并
        Self::merge_external_symbols(sym_paths, &mut dict);

        dict.save_to_file(output_path.as_ref())?;
        Ok(dict)
    }

    /// 合并外部英文词库文件中的新增英文词条（按小写形式去重，保留首次出现者）
    fn merge_external_english_words<P: AsRef<Path>>(
        en_paths: &[P],
        dict: &mut AuxiliaryDict,
        seen_en: &mut HashSet<String>,
    ) {
        for path in en_paths {
            let p_ref = path.as_ref();
            if !p_ref.exists() {
                continue;
            }
            let Ok(file) = File::open(p_ref) else {
                continue;
            };
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if Self::is_skippable_dict_line(trimmed) {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.is_empty() {
                    continue;
                }
                let word = parts[0].trim();
                if word.is_empty() {
                    continue;
                }
                let score: u32 = Self::parse_english_word_score(&parts);
                if seen_en.insert(word.to_lowercase()) {
                    dict.english_words.push((word.to_string(), score));
                }
            }
        }
    }

    /// 解析外部英文词库条目的词频分值（第三列优先，退化时取第二列，缺省为 50000）
    fn parse_english_word_score(parts: &[&str]) -> u32 {
        if parts.len() >= 3 {
            parts[2].trim().parse().unwrap_or(50_000)
        } else if parts.len() >= 2 {
            parts[1].trim().parse().unwrap_or(50_000)
        } else {
            50_000
        }
    }

    /// 合并外部符号词库文件中的符号、表情与颜文字条目
    fn merge_external_symbols<Q: AsRef<Path>>(sym_paths: &[Q], dict: &mut AuxiliaryDict) {
        for path in sym_paths {
            let p_ref = path.as_ref();
            if !p_ref.exists() {
                continue;
            }
            let Ok(file) = File::open(p_ref) else {
                continue;
            };
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if Self::is_skippable_dict_line(trimmed) {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.len() < 2 {
                    continue;
                }
                if let Some((key, sym, desc)) = Self::parse_symbol_entry(&parts)
                    && !key.is_empty()
                    && !sym.is_empty()
                {
                    dict.symbols.push((key, sym, desc));
                }
            }
        }
    }

    /// 解析外部符号词库条目为 (拼音键, 符号文本, 中文注释) 三元组
    ///
    /// 首列含非字母数字字符时判定为「表情/颜文字」形态（拼音键取第二列），
    /// 否则判定为「普通符号」形态（拼音键取首列小写形式）。
    fn parse_symbol_entry(parts: &[&str]) -> Option<(String, String, String)> {
        if parts[0]
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
            Some((py, emoji_or_sym, d))
        } else {
            let k = parts[0].trim().to_lowercase();
            let s = parts[1].trim().to_string();
            let d = if parts.len() >= 3 {
                parts[2].trim().to_string()
            } else {
                "符号".to_string()
            };
            Some((k, s, d))
        }
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
    /// 由多个文本词库源编译紧凑二进制词库（转发至 [`compact::CompactDictBuilder`]）
    ///
    /// # Errors
    /// 输入读取、词表解析或输出写入失败时返回 [`AlgoError`]。
    #[inline]
    pub fn build_compact_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        bin_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        self::compact::CompactDictBuilder::build_compact_binary_dict(input_paths, bin_path)
    }

    /// 使用指定过滤与索引配置编译紧凑二进制词库
    ///
    /// # Errors
    /// 输入读取、词表解析或输出写入失败时返回 [`AlgoError`]。
    #[inline]
    pub fn build_compact_binary_dict_with_options<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        bin_path: Q,
        options: DictBuildOptions,
    ) -> Result<BuildStats, AlgoError> {
        self::compact::CompactDictBuilder::build_compact_binary_dict_with_options(
            input_paths,
            bin_path,
            options,
        )
    }
}

#[cfg(test)]
mod tests;
