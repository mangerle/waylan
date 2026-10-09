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

/// 词库构建与导入器 (Dictionary Builder & Importer)
///
/// # 设计原理
/// - **实现初衷**：将离线全量词库打包（YAML -> bin）与普通用户个人词库一键导入（TXT -> 用户自适应库）统一抽象，
///   避免在系统层面散落碎片化的脚本与工具包。
/// - **核心优势**：流式行解析，内存开销可控；同时支持多源词库归并去重与按词频排序。
/// - **代价与局限**：对超大规模词库（>100万条）需短暂占用数十兆内存完成排序与序列化。
/// 将带声调的拼音字符归一化为无声调的 ASCII 拼音，并将空格转换为单引号切分符
pub fn normalize_pinyin_tones(pinyin: &str) -> String {
    let mut s = String::with_capacity(pinyin.len());
    for c in pinyin.chars() {
        match c {
            'ā' | 'á' | 'ǎ' | 'à' => s.push('a'),
            'ō' | 'ó' | 'ǒ' | 'ò' => s.push('o'),
            'ē' | 'é' | 'ě' | 'è' | 'ê' => s.push('e'),
            'ī' | 'í' | 'ǐ' | 'ì' => s.push('i'),
            'ū' | 'ú' | 'ǔ' | 'ù' => s.push('u'),
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' => s.push('v'),
            'ń' | 'ň' => s.push('n'),
            ' ' => s.push('\''),
            other => s.push(other),
        }
    }
    s
}

#[derive(Debug, Default)]
pub struct DictBuilder;

impl DictBuilder {
    /// 从多个文本词库源编译生成统一紧凑二进制词库文件（`base.dict.bin`）
    ///
    /// # Errors
    /// 当文件无法读取或目标路径无法写入时返回 [`AlgoError::DictionaryIo`]。
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

                // 支持制表符 \t 分隔或多空格分隔: 词条 \t 拼音 \t [词频]
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

        // 按词频降序优化每个拼音键下的候选列表
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

    /// 构建并编译辅助与混输二进制词库（`aux.dict.bin`）
    ///
    /// # 参数
    /// - `en_paths`: 可选的外部英文词库文本/YAML 路径列表
    /// - `sym_paths`: 可选的外部符号与表情文本/YAML 路径列表
    /// - `output_path`: 输出的二进制目标文件路径
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
                    for line_res in reader.lines() {
                        if let Ok(line) = line_res {
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
                                if !dict.english_words.iter().any(|(w, _)| w.to_lowercase() == word_lower) {
                                    dict.english_words.push((word.to_string(), score));
                                }
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
                    for line_res in reader.lines() {
                        if let Ok(line) = line_res {
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
                                let (key, sym, desc) = if parts[0].chars().any(|c| !c.is_ascii_alphanumeric() && c != '\'' && c != '_') {
                                    // 符号/Emoji/颜文字在前，拼音在后
                                    let py = parts[1].trim().to_lowercase().replace(' ', "");
                                    let emoji_or_sym = parts[0].trim().to_string();
                                    let d = if py == "vv" {
                                        "颜文字".to_string()
                                    } else {
                                        "表情".to_string()
                                    };
                                    (py, emoji_or_sym, d)
                                } else {
                                    // 拼音/缩写在前，符号在后
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
        }

        dict.save_to_file(output_path.as_ref())?;
        Ok(dict)
    }

    /// 用户导入个人自定义词库文本（直接注入用户自适应习惯词库中）
    ///
    /// # 格式支持
    /// 每行格式为：`词条 \t 拼音` 或 `词条 \t 拼音 \t 初始频次`
    ///
    /// # Errors
    /// 当文件无法打开时返回 [`AlgoError::DictionaryIo`]。
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_build_and_import() {
        let temp_dir = std::env::temp_dir();
        let txt_path = temp_dir.join("test_dict.txt");
        let bin_path = temp_dir.join("test_dict.bin");

        // 写入测试源数据
        let mut file = File::create(&txt_path).expect("创建临时文件");
        writeln!(file, "# 注释行").unwrap();
        writeln!(file, "微澜\twei lan\t15000").unwrap();
        writeln!(file, "智能\tzhi neng\t12000").unwrap();
        drop(file);

        // 1. 编译二进制词库
        let stats = DictBuilder::build_binary_dict(&[&txt_path], &bin_path).expect("编译成功");
        assert_eq!(stats.total_words, 2);
        assert_eq!(stats.total_keys, 2);

        // 2. 导入到用户自适应模型
        let mut user_model = UserAdaptiveModel::new();
        let imported =
            DictBuilder::import_user_text_dict(&txt_path, &mut user_model, 1_700_000_000)
                .expect("导入成功");
        assert_eq!(imported, 2);

        // 验证用户模型中已包含自造词
        let words = user_model.lookup_user_words("wei'lan", 1_700_000_000);
        assert!(!words.is_empty());
        assert_eq!(words[0].text, "微澜");

        // 清理临时文件
        let _ = std::fs::remove_file(txt_path);
        let _ = std::fs::remove_file(bin_path);
    }
}
