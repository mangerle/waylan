use crate::custom_phrase::AuxiliaryDict;
use crate::dat::DoubleArrayTrie;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::syllables::STANDARD_SYLLABLES;
use crate::table::COMPACT_DICT_MAGIC;
use crate::user_adaptive::UserAdaptiveModel;
use crate::vocab::Vocabulary;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
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
            'ń' | 'ň' | 'ǹ' => s.push('n'),
            'ḿ' => s.push('m'),
            ' ' => s.push('\''),
            c if c.is_ascii_alphabetic() => s.push(c.to_ascii_lowercase()),
            c if c == '\'' => s.push('\''),
            _ => {}
        }
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
    pub fn build_compact_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        output_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        let start = Instant::now();
        let mut total_lines: usize = 0;
        let mut total_words: usize = 0;

        // 1. 构建严格稳定的全局 Word ID 词表
        // 优先检查同一目录是否存在旧版 vocab.bin，若存在则继承其 Word ID 顺序，确保与现有 Bigram 模型 100% 对应
        let mut word_to_id: HashMap<String, u32> = HashMap::new();
        let mut words: Vec<String> = Vec::new();

        let vocab_path = Path::new("data/vocab.bin");
        if vocab_path.exists() {
            if let Ok(vocab) = Vocabulary::load_binary(vocab_path) {
                for (id, w) in vocab.all_words().iter().enumerate() {
                    word_to_id.insert(w.clone(), id as u32);
                    words.push(w.clone());
                }
            }
        }

        // 2. 解析文本词库源文件
        let mut dict_map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();

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
                    if pinyin.is_empty() || word.is_empty() {
                        continue;
                    }

                    let score: u32 = if parts.len() >= 3 {
                        parts[2].trim().parse().unwrap_or(1000)
                    } else {
                        1000
                    };

                    let wid = if let Some(&id) = word_to_id.get(&word) {
                        id
                    } else {
                        let new_id = words.len() as u32;
                        words.push(word.clone());
                        word_to_id.insert(word, new_id);
                        new_id
                    };

                    dict_map.entry(pinyin).or_default().push((wid, score));
                    total_words = total_words.saturating_add(1);
                }
            }
        }

        // 按原始词频降序排序
        for entries in dict_map.values_mut() {
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        }

        // 3. 提取简拼缩写索引 (key_type = 1)
        let mut acronym_map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        for (pinyin, cand_list) in &dict_map {
            if pinyin.contains('\'') {
                let parts: Vec<&str> = pinyin.split('\'').collect();
                if parts.len() >= 2 {
                    let mut acronym = String::with_capacity(parts.len());
                    for part in parts {
                        if let Some(ch) = part.chars().next() {
                            acronym.push(ch);
                        }
                    }
                    if acronym.len() >= 2 {
                        let list = acronym_map.entry(acronym).or_default();
                        for &(wid, score) in cand_list {
                            if score >= 3_000 {
                                list.push((wid, score));
                            }
                        }
                    }
                }
            }
        }
        for list in acronym_map.values_mut() {
            list.sort_by_key(|b| std::cmp::Reverse(b.1));
            list.truncate(8);
        }

        // 4. 提取声母单字索引 (key_type = 2)
        let mut initial_map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        for &initial in STANDARD_INITIALS {
            let mut list = Vec::new();
            for &syl in STANDARD_SYLLABLES {
                if syl.starts_with(initial) {
                    if let Some(matches) = dict_map.get(syl) {
                        for &(wid, score) in matches {
                            if let Some(text) = words.get(wid as usize) {
                                if text.chars().count() == 1 {
                                    list.push((wid, score));
                                }
                            }
                        }
                    }
                }
            }
            list.sort_by_key(|r| std::cmp::Reverse(r.1));
            list.truncate(5);
            if !list.is_empty() {
                initial_map.insert(initial.to_string(), list);
            }
        }

        // 5. 汇总所有唯一键：全拼(0)、简拼(1)、声母(2)
        // 使用 HashMap 聚合，避免同一拼音键在不同来源冲突
        let mut combined_keys: HashMap<String, (u8, Vec<(u32, u32)>)> = HashMap::new();
        for (k, v) in dict_map {
            combined_keys.insert(k, (0, v));
        }
        for (k, v) in acronym_map {
            if !combined_keys.contains_key(&k) {
                combined_keys.insert(k, (1, v));
            }
        }
        for (k, v) in initial_map {
            if !combined_keys.contains_key(&k) {
                combined_keys.insert(k, (2, v));
            }
        }

        // 严格按字典序排序所有键
        let mut sorted_keys: Vec<String> = combined_keys.keys().cloned().collect();
        sorted_keys.sort();

        let num_keys = sorted_keys.len();
        let key_values: Vec<u32> = (0..num_keys as u32).collect();

        // 6. 构建双数组 Trie (DAT + Tail 压缩)
        let dat = DoubleArrayTrie::build(&sorted_keys, &key_values);

        // 7. 构建候选值数组与键元数据数组
        let mut key_metas_data = Vec::with_capacity(num_keys * 8);
        let mut candidates_data = Vec::new();
        let mut cand_counter: u32 = 0;

        for k in &sorted_keys {
            let (k_type, cands) = combined_keys.get(k).unwrap();
            let c_start = cand_counter;
            let c_count = cands.len() as u16;

            for &(wid, score) in cands {
                candidates_data.extend_from_slice(&wid.to_le_bytes());
                let score_u16 = (score.min(65535)) as u16;
                candidates_data.extend_from_slice(&score_u16.to_le_bytes());
                cand_counter += 1;
            }

            key_metas_data.extend_from_slice(&c_start.to_le_bytes());
            key_metas_data.extend_from_slice(&c_count.to_le_bytes());
            key_metas_data.push(*k_type);
            key_metas_data.push(0); // 对齐 padding
        }

        let num_candidates = cand_counter as usize;

        // 8. 构建单源词表段 (Vocab Offsets + String Pool + Sorted Vocab IDs)
        let vocab_size = words.len();
        let mut vocab_offsets: Vec<u32> = Vec::with_capacity(vocab_size + 1);
        let mut vocab_strings: Vec<u8> = Vec::new();
        let mut current_str_offset: u32 = 0;

        for w in &words {
            vocab_offsets.push(current_str_offset);
            let b = w.as_bytes();
            vocab_strings.extend_from_slice(b);
            current_str_offset += b.len() as u32;
        }
        vocab_offsets.push(current_str_offset);

        // 构建按字典序排序的 sorted_vocab_ids
        let mut sorted_vocab_ids: Vec<u32> = (0..vocab_size as u32).collect();
        sorted_vocab_ids.sort_by(|&a, &b| words[a as usize].cmp(&words[b as usize]));

        // 9. 计算各段文件绝对偏移量
        let header_len = 64usize;
        let dat_base_bytes = dat.base.len() * 4;
        let dat_check_bytes = dat.check.len() * 4;
        let dat_tail_bytes = dat.tail.len();

        let mut current_offset = header_len + dat_base_bytes + dat_check_bytes + dat_tail_bytes;
        // 4 字节对齐
        let pad1 = (4 - (current_offset % 4)) % 4;
        current_offset += pad1;

        let key_metas_offset = current_offset as u64;
        current_offset += key_metas_data.len();

        let candidates_offset = current_offset as u64;
        current_offset += candidates_data.len();

        let pad2 = (4 - (current_offset % 4)) % 4;
        current_offset += pad2;

        let vocab_offset = current_offset as u64;

        // 10. 写入目标文件
        let out_ref = output_path.as_ref();
        let out_file = File::create(out_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(out_ref),
            source: e,
        })?;
        let mut writer = BufWriter::new(out_file);

        // Header (64 字节)
        let mut header = [0u8; 64];
        header[0..16].copy_from_slice(COMPACT_DICT_MAGIC);
        header[16..20].copy_from_slice(&2u32.to_le_bytes()); // version = 2
        header[20..24].copy_from_slice(&(num_keys as u32).to_le_bytes());
        header[24..28].copy_from_slice(&(num_candidates as u32).to_le_bytes());
        header[28..32].copy_from_slice(&(vocab_size as u32).to_le_bytes());
        header[32..36].copy_from_slice(&(dat.base.len() as u32).to_le_bytes());
        header[36..40].copy_from_slice(&(dat.tail.len() as u32).to_le_bytes());
        header[40..48].copy_from_slice(&key_metas_offset.to_le_bytes());
        header[48..56].copy_from_slice(&candidates_offset.to_le_bytes());
        header[56..64].copy_from_slice(&vocab_offset.to_le_bytes());
        writer.write_all(&header).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;

        // DAT 数据
        for &b in &dat.base {
            writer.write_all(&b.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }
        for &c in &dat.check {
            writer.write_all(&c.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }
        writer.write_all(&dat.tail).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        if pad1 > 0 {
            writer.write_all(&vec![0u8; pad1]).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }

        // Key Metas 与 Candidates
        writer.write_all(&key_metas_data).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        writer.write_all(&candidates_data).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        if pad2 > 0 {
            writer.write_all(&vec![0u8; pad2]).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }

        // Vocab 段
        for &off in &vocab_offsets {
            writer.write_all(&off.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }
        for &id in &sorted_vocab_ids {
            writer.write_all(&id.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;
        }
        writer.write_all(&vocab_strings).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;

        writer.flush().map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(out_ref), source: e })?;

        let total_keys = sorted_keys.len();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(BuildStats {
            total_lines,
            total_words,
            total_keys,
            elapsed_ms,
        })
    }

    /// 构建并编译辅助与混输二进制词库（`aux.dict.bin`）
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_build_and_load_compact_mmap() {
        use crate::table::LexiconTable;

        let temp_dir = std::env::temp_dir();
        let txt_path = temp_dir.join("test_compact_dict.txt");
        let compact_bin_path = temp_dir.join("test_compact_dict.bin");

        // 写入测试源数据
        let mut file = File::create(&txt_path).expect("创建临时文件");
        writeln!(file, "微澜\twei lan\t15000").unwrap();
        writeln!(file, "中国\tzhong guo\t20000").unwrap();
        writeln!(file, "中\tzhong\t9000").unwrap();
        drop(file);

        // 1. 编译为零拷贝紧凑二进制格式 (DAT 一体化)
        let stats = DictBuilder::build_compact_binary_dict(&[&txt_path], &compact_bin_path)
            .expect("紧凑编译成功");
        assert_eq!(stats.total_words, 3);

        // 2. 使用 LexiconTable::load_binary 加载
        let table = LexiconTable::load_binary(&compact_bin_path).expect("零拷贝紧凑词库加载成功");

        // 3. 验证全拼检索
        let cands = table.lookup("zhong'guo");
        assert!(!cands.is_empty());
        assert_eq!(cands[0].text, "中国");

        // 4. 验证简拼缩写检索 (由 builder 自动提取)
        let acronym_cands = table.lookup_acronym("zg");
        assert!(!acronym_cands.is_empty(), "简拼 zg 应召回中国");
        assert_eq!(acronym_cands[0].text, "中国");

        // 5. 验证单字声母预置索引
        let initial_entries = table.get_initial_raw_entries("z");
        assert!(initial_entries.is_some(), "声母 z 应有预置单字索引");

        // 6. 验证 Word ID 与反查
        let compact = table.compact_mmap.as_ref().unwrap();
        let id_zg = compact.get_id("中国");
        assert!(id_zg.is_some());
        let word_zg = compact.get_word(id_zg.unwrap());
        assert_eq!(word_zg, Some("中国"));

        // 清理临时文件
        let _ = std::fs::remove_file(txt_path);
        let _ = std::fs::remove_file(compact_bin_path);
    }
}
