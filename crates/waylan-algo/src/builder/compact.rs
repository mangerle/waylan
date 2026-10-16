use crate::builder::{normalize_pinyin_tones, BuildStats};
use crate::dat::DoubleArrayTrie;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::syllables::STANDARD_SYLLABLES;
use crate::table::COMPACT_DICT_MAGIC;
use crate::vocab::Vocabulary;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 紧凑二进制词库构建器
pub struct CompactDictBuilder;

impl CompactDictBuilder {
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
        if words.is_empty() {
            if let Some(parent) = output_path.as_ref().parent() {
                if parent.join("bigram.table.bin").exists() {
                    let candidates_bin = [
                        output_path.as_ref().to_path_buf(),
                        PathBuf::from("/usr/share/waylan/data/base.dict.bin"),
                    ];
                    for bin_path in &candidates_bin {
                        if bin_path.exists() {
                            if let Ok(compact) = crate::table::CompactMmapLexicon::open(bin_path) {
                                if compact.vocab_size > 1_800_000 {
                                    words.reserve(compact.vocab_size);
                                    word_to_id.reserve(compact.vocab_size);
                                    for id in 0..compact.vocab_size as u32 {
                                        let w = compact.get_word(id).unwrap_or("").to_string();
                                        word_to_id.insert(w.clone(), id);
                                        words.push(w);
                                    }
                                    break;
                                }
                            }
                        }
                    }
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

        // 按原始词频降序排序并对同音同词去重（保留最高分）
        for entries in dict_map.values_mut() {
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            entries.dedup_by(|a, b| a.0 == b.0);
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        }

        // 3. 提取简拼缩写索引 (key_type = 1)
        // 分级准入门槛：2字词碰撞率高，要求基础词频 >= 200；3字及以上长词与四字成语碰撞率低，保留全部有效词频 (score >= 1)，
        // 并由后续按词频降序排序与 Top-8 截断统一保障索引紧凑度与成语召回率。
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
                        let min_score = if acronym.len() == 2 { 200 } else { 1 };
                        let list = acronym_map.entry(acronym).or_default();
                        for &(wid, score) in cand_list.iter().take(8) {
                            if score >= min_score {
                                list.push((wid, score));
                            }
                        }
                    }
                }
            }
        }
        for list in acronym_map.values_mut() {
            list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            list.dedup_by(|a, b| a.0 == b.0);
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
            list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            list.dedup_by(|a, b| a.0 == b.0);
            list.sort_by_key(|r| std::cmp::Reverse(r.1));
            list.truncate(5);
            if !list.is_empty() {
                initial_map.insert(initial.to_string(), list);
            }
        }

        // 5. 汇总所有唯一键：全拼(0)、简拼(1)、声母(2)
        // 采用内部单引号前缀隔离三类键空间（全拼: "{k}", 简拼: "'{k}", 声母: "''{k}"），
        // 避免同形全拼、简拼、声母（如 "zh", "xian", "ma"）在 DAT 中发生键名覆盖冲突。
        let mut combined_keys: HashMap<String, (u8, Vec<(u32, u32)>)> = HashMap::new();
        for (k, v) in dict_map {
            combined_keys.insert(k, (0, v));
        }
        for (k, v) in acronym_map {
            if !v.is_empty() {
                combined_keys.insert(format!("'{}", k), (1, v));
            }
        }
        for (k, v) in initial_map {
            if !v.is_empty() {
                combined_keys.insert(format!("''{}", k), (2, v));
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
            let (k_type, cands) = combined_keys
                .get(k)
                .expect("键来自于 combined_keys 自身集合，查询必定命中");
            let c_start = cand_counter;
            let capped_len = cands.len().min(u16::MAX as usize);
            let c_count = capped_len as u16;

            for &(wid, score) in cands.iter().take(capped_len) {
                candidates_data.extend_from_slice(&wid.to_le_bytes());
                let score_u16 = encode_compact_score(score);
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
        writer
            .write_all(&header)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(out_ref),
                source: e,
            })?;

        // DAT 数据
        for &b in &dat.base {
            writer
                .write_all(&b.to_le_bytes())
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }
        for &c in &dat.check {
            writer
                .write_all(&c.to_le_bytes())
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }
        writer
            .write_all(&dat.tail)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(out_ref),
                source: e,
            })?;
        if pad1 > 0 {
            writer
                .write_all(&vec![0u8; pad1])
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }

        // Key Metas 与 Candidates
        writer
            .write_all(&key_metas_data)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(out_ref),
                source: e,
            })?;
        writer
            .write_all(&candidates_data)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(out_ref),
                source: e,
            })?;
        if pad2 > 0 {
            writer
                .write_all(&vec![0u8; pad2])
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }

        // Vocab 段
        for &off in &vocab_offsets {
            writer
                .write_all(&off.to_le_bytes())
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }
        for &id in &sorted_vocab_ids {
            writer
                .write_all(&id.to_le_bytes())
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(out_ref),
                    source: e,
                })?;
        }
        writer
            .write_all(&vocab_strings)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(out_ref),
                source: e,
            })?;

        writer.flush().map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(out_ref),
            source: e,
        })?;

        let total_keys = sorted_keys.len();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(BuildStats {
            total_lines,
            total_words,
            total_keys,
            elapsed_ms,
        })
    }
}

/// 将 32 位原始词频单调映射为 16 位紧凑存储分：
/// - `0..=32_767` 保持 1:1 线性原值；
/// - `> 32_767` 采用对数压缩映射至 `32_768..=65_534`，保留百万级高频词之间的相对梯度。
#[inline]
pub(crate) fn encode_compact_score(score: u32) -> u16 {
    if score <= 32_767 {
        score as u16
    } else {
        let extra = ((score - 32_767) as f64 + 1.0).ln() * 2_000.0;
        (32_768 + (extra as u32).min(32_766)) as u16
    }
}
