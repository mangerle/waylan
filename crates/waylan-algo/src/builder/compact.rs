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

/// 候选词二元组（词汇 ID, 频次分值）
type CandidateScore = (u32, u32);

/// 键名到候选词元组列表的映射
type KeyCandidateMap = HashMap<String, Vec<CandidateScore>>;

/// 合并后的键表（存储键类型标记与候选词列表）
type CombinedKeyMap = HashMap<String, (u8, Vec<CandidateScore>)>;

/// 文本词典解析结果摘要
struct ParsedTextSources {
    dict_map: KeyCandidateMap,
    total_lines: usize,
    total_words: usize,
}

/// 紧凑词库二进制持久化装配参数对象
pub struct CompactWritePayload<'a> {
    pub output_path: &'a Path,
    pub dat: &'a DoubleArrayTrie,
    pub key_metas_data: &'a [u8],
    pub candidates_data: &'a [u8],
    pub words: &'a [String],
    pub num_keys: usize,
    pub num_candidates: usize,
}

/// 紧凑二进制词库构建器
pub struct CompactDictBuilder;

impl CompactDictBuilder {
    pub fn build_compact_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        output_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        let start = Instant::now();
        let out_ref = output_path.as_ref();

        // 1. 初始化或继承全局 Word ID 词表
        let (mut word_to_id, mut words) = Self::init_vocab(out_ref);

        // 2. 解析文本词库源文件并更新词表
        let parsed = Self::parse_text_sources(input_paths, &mut word_to_id, &mut words)?;

        // 3. 生成全拼、简拼、声母三类隔离索引键并严格排序
        let (combined_keys, sorted_keys) = Self::build_combined_keys(parsed.dict_map, &words);

        // 4. 构建双数组 Trie (DAT + Tail 压缩)
        let key_values: Vec<u32> = (0..sorted_keys.len() as u32).collect();
        let dat = DoubleArrayTrie::build(&sorted_keys, &key_values);

        // 5. 编码候选值与键元数据
        let (key_metas_data, candidates_data, num_candidates) =
            Self::encode_keys_and_candidates(&combined_keys, &sorted_keys);

        // 6. 计算绝对偏移并持久化写入目标二进制文件
        Self::write_compact_file(CompactWritePayload {
            output_path: out_ref,
            dat: &dat,
            key_metas_data: &key_metas_data,
            candidates_data: &candidates_data,
            words: &words,
            num_keys: sorted_keys.len(),
            num_candidates,
        })?;

        let total_keys = sorted_keys.len();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(BuildStats {
            total_lines: parsed.total_lines,
            total_words: parsed.total_words,
            total_keys,
            elapsed_ms,
        })
    }

    /// 从现有的紧凑二进制词库文件中抽取词表（支持版本 2 与版本 3）
    fn extract_vocab_from_bin(path: &Path) -> Option<Vec<String>> {
        let file = File::open(path).ok()?;
        let mmap = unsafe { memmap2::Mmap::map(&file).ok()? };
        if mmap.len() < 64 || &mmap[0..16] != COMPACT_DICT_MAGIC {
            return None;
        }
        let version = u32::from_le_bytes(mmap[16..20].try_into().ok()?);
        if version != 2 && version != 3 {
            return None;
        }
        let vocab_size = u32::from_le_bytes(mmap[28..32].try_into().ok()?) as usize;
        let vocab_offset = u64::from_le_bytes(mmap[56..64].try_into().ok()?) as usize;
        let vocab_offsets_offset = vocab_offset;
        let sorted_vocab_ids_offset =
            vocab_offsets_offset.saturating_add((vocab_size.saturating_add(1)).saturating_mul(4));
        let vocab_strings_offset =
            sorted_vocab_ids_offset.saturating_add(vocab_size.saturating_mul(4));

        let mut words = Vec::with_capacity(vocab_size);
        for id in 0..vocab_size {
            let off1_pos = vocab_offsets_offset.saturating_add(id.saturating_mul(4));
            let off2_pos = off1_pos.saturating_add(4);
            let off3_pos = off2_pos.saturating_add(4);
            let start = u32::from_le_bytes(mmap.get(off1_pos..off2_pos)?.try_into().ok()?) as usize;
            let end = u32::from_le_bytes(mmap.get(off2_pos..off3_pos)?.try_into().ok()?) as usize;
            let s_start = vocab_strings_offset.saturating_add(start);
            let s_end = vocab_strings_offset.saturating_add(end);
            let slice = mmap.get(s_start..s_end)?;
            let text = std::str::from_utf8(slice).ok()?;
            words.push(text.to_string());
        }
        Some(words)
    }

    /// 初始化或继承旧版词表以保持 Word ID 稳定性
    fn init_vocab(output_path: &Path) -> (HashMap<String, u32>, Vec<String>) {
        let mut word_to_id = HashMap::new();
        let mut words = Vec::new();

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
            if let Some(parent) = output_path.parent() {
                if parent.join("bigram.table.bin").exists() {
                    let candidates_bin = [
                        output_path.to_path_buf(),
                        PathBuf::from("/usr/share/waylan/data/base.dict.bin"),
                    ];
                    for bin_path in &candidates_bin {
                        if bin_path.exists() {
                            if let Some(extracted) = Self::extract_vocab_from_bin(bin_path) {
                                if extracted.len() > 1_800_000 {
                                    words.reserve(extracted.len());
                                    word_to_id.reserve(extracted.len());
                                    for (id, w) in extracted.into_iter().enumerate() {
                                        word_to_id.insert(w.clone(), id as u32);
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
        (word_to_id, words)
    }

    /// 解析文本词典源文件并去重排序
    fn parse_text_sources<P: AsRef<Path>>(
        input_paths: &[P],
        word_to_id: &mut HashMap<String, u32>,
        words: &mut Vec<String>,
    ) -> Result<ParsedTextSources, AlgoError> {
        let mut dict_map: KeyCandidateMap = HashMap::new();
        let mut total_lines = 0usize;
        let mut total_words = 0usize;

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

        // 按原始词频降序排序并去重（保留最高分）
        for entries in dict_map.values_mut() {
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            entries.dedup_by(|a, b| a.0 == b.0);
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        }

        Ok(ParsedTextSources {
            dict_map,
            total_lines,
            total_words,
        })
    }

    /// 提取简拼与声母单字索引，合并并生成前缀隔离的排序键集合
    fn build_combined_keys(
        dict_map: KeyCandidateMap,
        words: &[String],
    ) -> (CombinedKeyMap, Vec<String>) {
        // 提取简拼缩写索引 (key_type = 1)
        let mut acronym_map: KeyCandidateMap = HashMap::new();
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
                        let min_score = if acronym.len() == 2 { 200 } else { 50 };
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

        // 提取声母单字索引 (key_type = 2)
        let mut initial_map: KeyCandidateMap = HashMap::new();
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
            list.truncate(16);
            if !list.is_empty() {
                initial_map.insert(initial.to_string(), list);
            }
        }

        // 汇总隔离键空间：全拼("{k}"), 简拼("'{k}"), 声母("''{k}")
        let mut combined_keys: CombinedKeyMap = HashMap::new();
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

        let mut sorted_keys: Vec<String> = combined_keys.keys().cloned().collect();
        sorted_keys.sort();

        (combined_keys, sorted_keys)
    }

    /// 编码键元数据与紧凑候选值数组
    fn encode_keys_and_candidates(
        combined_keys: &CombinedKeyMap,
        sorted_keys: &[String],
    ) -> (Vec<u8>, Vec<u8>, usize) {
        let num_keys = sorted_keys.len();
        let mut key_metas_data = Vec::with_capacity(num_keys * 8);
        let mut candidates_data = Vec::new();
        let mut cand_counter: u32 = 0;

        for k in sorted_keys {
            let (k_type, cands) = combined_keys
                .get(k)
                .expect("键来自于 combined_keys 自身集合，查询必定命中");
            let c_start = cand_counter;
            let capped_len = cands.len().min(64);
            let c_count = capped_len as u16;

            for &(wid, score) in cands.iter().take(capped_len) {
                candidates_data.extend_from_slice(&wid.to_le_bytes());
                candidates_data.extend_from_slice(&score.to_le_bytes());
                cand_counter += 1;
            }

            key_metas_data.extend_from_slice(&c_start.to_le_bytes());
            key_metas_data.extend_from_slice(&c_count.to_le_bytes());
            key_metas_data.push(*k_type);
            key_metas_data.push(0); // 对齐 padding
        }

        (key_metas_data, candidates_data, cand_counter as usize)
    }

    /// 组装文件头与各段数据并写入目标磁盘文件
    fn write_compact_file(payload: CompactWritePayload<'_>) -> Result<(), AlgoError> {
        let CompactWritePayload {
            output_path,
            dat,
            key_metas_data,
            candidates_data,
            words,
            num_keys,
            num_candidates,
        } = payload;
        let vocab_size = words.len();
        let mut vocab_offsets: Vec<u32> = Vec::with_capacity(vocab_size + 1);
        let mut vocab_strings: Vec<u8> = Vec::new();
        let mut current_str_offset: u32 = 0;

        for w in words {
            vocab_offsets.push(current_str_offset);
            let b = w.as_bytes();
            vocab_strings.extend_from_slice(b);
            current_str_offset += b.len() as u32;
        }
        vocab_offsets.push(current_str_offset);

        let mut sorted_vocab_ids: Vec<u32> = (0..vocab_size as u32).collect();
        sorted_vocab_ids.sort_by(|&a, &b| words[a as usize].cmp(&words[b as usize]));

        let header_len = 64usize;
        let dat_base_bytes = dat.base.len() * 4;
        let dat_check_bytes = dat.check.len() * 4;
        let dat_tail_bytes = dat.tail.len();

        let mut current_offset = header_len + dat_base_bytes + dat_check_bytes + dat_tail_bytes;
        let pad1 = (4 - (current_offset % 4)) % 4;
        current_offset += pad1;

        let key_metas_offset = current_offset as u64;
        current_offset += key_metas_data.len();

        let candidates_offset = current_offset as u64;
        current_offset += candidates_data.len();

        let pad2 = (4 - (current_offset % 4)) % 4;
        current_offset += pad2;

        let vocab_offset = current_offset as u64;

        let out_file = File::create(output_path).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(output_path),
            source: e,
        })?;
        let mut writer = BufWriter::new(out_file);

        let mut header = [0u8; 64];
        header[0..16].copy_from_slice(COMPACT_DICT_MAGIC);
        header[16..20].copy_from_slice(&3u32.to_le_bytes());
        header[20..24].copy_from_slice(&(num_keys as u32).to_le_bytes());
        header[24..28].copy_from_slice(&(num_candidates as u32).to_le_bytes());
        header[28..32].copy_from_slice(&(vocab_size as u32).to_le_bytes());
        header[32..36].copy_from_slice(&(dat.base.len() as u32).to_le_bytes());
        header[36..40].copy_from_slice(&(dat.tail.len() as u32).to_le_bytes());
        header[40..48].copy_from_slice(&key_metas_offset.to_le_bytes());
        header[48..56].copy_from_slice(&candidates_offset.to_le_bytes());
        header[56..64].copy_from_slice(&vocab_offset.to_le_bytes());

        let io_err = |e| AlgoError::DictionaryIo {
            path: PathBuf::from(output_path),
            source: e,
        };

        writer.write_all(&header).map_err(io_err)?;
        for &b in &dat.base {
            writer.write_all(&b.to_le_bytes()).map_err(io_err)?;
        }
        for &c in &dat.check {
            writer.write_all(&c.to_le_bytes()).map_err(io_err)?;
        }
        writer.write_all(&dat.tail).map_err(io_err)?;
        if pad1 > 0 {
            writer.write_all(&vec![0u8; pad1]).map_err(io_err)?;
        }

        writer.write_all(key_metas_data).map_err(io_err)?;
        writer.write_all(candidates_data).map_err(io_err)?;
        if pad2 > 0 {
            writer.write_all(&vec![0u8; pad2]).map_err(io_err)?;
        }

        for &off in &vocab_offsets {
            writer.write_all(&off.to_le_bytes()).map_err(io_err)?;
        }
        for &id in &sorted_vocab_ids {
            writer.write_all(&id.to_le_bytes()).map_err(io_err)?;
        }
        writer.write_all(&vocab_strings).map_err(io_err)?;

        writer.flush().map_err(io_err)?;
        Ok(())
    }
}
