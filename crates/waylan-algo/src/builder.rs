use crate::custom_phrase::AuxiliaryDict;
use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// 将带声调的拼音字符归一化为无声调的 ASCII 拼音，并将空格转换为单引号切分符（支持大写声调字符并合并连续单引号）
pub fn normalize_pinyin_tones(pinyin: &str) -> String {
    let mut s = String::with_capacity(pinyin.len());
    for orig_c in pinyin.chars() {
        for c in orig_c.to_lowercase() {
            match c {
                'ā' | 'á' | 'ǎ' | 'à' => s.push('a'),
                'ō' | 'ó' | 'ǒ' | 'ò' => s.push('o'),
                'ē' | 'é' | 'ě' | 'è' | 'ê' => s.push('e'),
                'ī' | 'í' | 'ǐ' | 'ì' => s.push('i'),
                'ū' | 'ú' | 'ǔ' | 'ù' => s.push('u'),
                'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' => s.push('v'),
                'ń' | 'ň' | 'ǹ' => s.push('n'),
                'ḿ' => s.push('m'),
                c if c.is_ascii_alphabetic() => s.push(c),
                ' ' | '\'' if !s.is_empty() && !s.ends_with('\'') => {
                    s.push('\'');
                }
                _ => {}
            }
        }
    }
    if s.ends_with('\'') {
        s.pop();
    }
    s
}

#[derive(Debug, Default)]
pub struct DictBuilder;

impl DictBuilder {
    /// 从多个文本词库源编译生成向后兼容的传统 bincode 二进制词库文件
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

    /// 从多个文本词库源编译生成全新的 DAT 一体化零拷贝紧凑二进制词库文件 (WAYLAN_COMPACT02)
    pub fn build_auxiliary_dict<P: AsRef<Path>, Q: AsRef<Path>, R: AsRef<Path>>(
        en_paths: &[P],
        sym_paths: &[Q],
        output_path: R,
    ) -> Result<AuxiliaryDict, AlgoError> {
        let mut dict = AuxiliaryDict::with_default_data();

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
                            let word_lower = word.to_lowercase();
                            if !dict
                                .english_words
                                .iter()
                                .any(|(w, _)| w.to_lowercase() == word_lower)
                            {
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
        crate::builder_compact::CompactDictBuilder::build_compact_binary_dict(input_paths, bin_path)
    }
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
