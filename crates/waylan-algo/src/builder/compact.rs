use super::DictBuildOptions;
use crate::builder::compact_writer::{write_compact_file, CompactWritePayload};
use crate::builder::{normalize_pinyin_tones, BuildStats};
use crate::dat::DoubleArrayTrie;
use crate::error::AlgoError;
use crate::syllables::STANDARD_INITIALS;
use crate::syllables::STANDARD_SYLLABLES;
use crate::table::COMPACT_DICT_MAGIC;
use crate::vocab::Vocabulary;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 候选词二元组（词汇 ID, 频次分值）
type CandidateScore = (u32, u32);

/// 键名到候选词元组列表的映射
type KeyCandidateMap = HashMap<String, Vec<CandidateScore>>;

/// 合并后的键表（存储键类型标记与候选词列表）
type CombinedKeyMap = HashMap<String, (u8, Vec<CandidateScore>)>;

/// 词表状态容器（保证 Word ID 映射与词表数组同步）
struct VocabState {
    word_to_id: HashMap<String, u32>,
    words: Vec<String>,
}

/// 文本词典解析结果摘要
struct ParsedTextSources {
    dict_map: KeyCandidateMap,
    total_lines: usize,
    total_words: usize,
}

/// 紧凑二进制词库构建器
pub struct CompactDictBuilder;

impl CompactDictBuilder {
    /// 由多个文本词库源编译紧凑二进制词库（保留全部词条的默认配置）
    ///
    /// # Errors
    /// 当输入文件读取失败、词表解析异常或输出文件写入失败时返回 [`AlgoError`]。
    pub fn build_compact_binary_dict<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        output_path: Q,
    ) -> Result<BuildStats, AlgoError> {
        Self::build_compact_binary_dict_with_options(
            input_paths,
            output_path,
            DictBuildOptions::default(),
        )
    }

    /// 使用指定过滤与索引配置编译紧凑二进制词库
    ///
    /// # Errors
    /// 当输入文件读取失败、词表解析异常或输出文件写入失败时返回 [`AlgoError`]。
    pub fn build_compact_binary_dict_with_options<P: AsRef<Path>, Q: AsRef<Path>>(
        input_paths: &[P],
        output_path: Q,
        options: DictBuildOptions,
    ) -> Result<BuildStats, AlgoError> {
        let start = Instant::now();
        let out_ref = output_path.as_ref();

        // 1. 初始化或继承全局 Word ID 词表
        let (word_to_id, words) = Self::init_vocab(out_ref);
        let mut vocab = VocabState { word_to_id, words };

        // 2. 解析文本词库源文件并更新词表
        let parsed = Self::parse_text_sources(input_paths, &mut vocab, options)?;

        // 3. 生成全拼、简拼、声母三类隔离索引键并严格排序
        let (combined_keys, sorted_keys) =
            Self::build_combined_keys(parsed.dict_map, &vocab.words, options);

        // 4. 构建双数组 Trie (DAT + Tail 压缩)
        let key_values: Vec<u32> = (0..sorted_keys.len() as u32).collect();
        let dat = DoubleArrayTrie::build(&sorted_keys, &key_values);

        // 5. 编码候选值与键元数据
        let (key_metas_data, candidates_data, num_candidates) =
            Self::encode_keys_and_candidates(&combined_keys, &sorted_keys);

        // 6. 计算绝对偏移并持久化写入目标二进制文件
        write_compact_file(CompactWritePayload {
            output_path: out_ref,
            dat: &dat,
            key_metas_data: &key_metas_data,
            candidates_data: &candidates_data,
            words: &vocab.words,
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
        vocab: &mut VocabState,
        options: DictBuildOptions,
    ) -> Result<ParsedTextSources, AlgoError> {
        let mut dict_map: KeyCandidateMap = HashMap::new();
        let mut total_lines = 0usize;
        let mut total_words = 0usize;

        for in_path in input_paths {
            let (file_lines, file_words) =
                Self::parse_text_source_file(in_path.as_ref(), &mut dict_map, vocab, options)?;
            total_lines = total_lines.saturating_add(file_lines);
            total_words = total_words.saturating_add(file_words);
        }

        // 按原始词频降序排序并去重（保留最高分）
        Self::sort_and_dedup_candidates(&mut dict_map);

        Ok(ParsedTextSources {
            dict_map,
            total_lines,
            total_words,
        })
    }

    /// 判定文本词典行是否为注释、空行或词库元信息头（此类行不参与词条编译）
    fn is_skippable_dict_line(trimmed: &str) -> bool {
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
            || trimmed.starts_with("name:")
            || trimmed.starts_with("version:")
            || trimmed.starts_with("sort:")
    }

    /// 解析单个文本词典源文件，将词条归一化为候选并同步维护全局词表
    ///
    /// 返回该文件实际读取的原始行数与有效词条数（注释与空行仅计入行数）
    fn parse_text_source_file(
        path_ref: &Path,
        dict_map: &mut KeyCandidateMap,
        vocab: &mut VocabState,
        options: DictBuildOptions,
    ) -> Result<(usize, usize), AlgoError> {
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let reader = BufReader::new(file);
        let mut total_lines = 0usize;
        let mut total_words = 0usize;

        for line_res in reader.lines() {
            total_lines = total_lines.saturating_add(1);
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };

            let trimmed = line.trim();
            if Self::is_skippable_dict_line(trimmed) {
                continue;
            }

            let parts: Vec<&str> = trimmed.split('\t').collect();
            if parts.len() >= 2 {
                let word = parts[0].trim().to_string();
                let pinyin = normalize_pinyin_tones(parts[1].trim());
                if pinyin.is_empty() || word.is_empty() {
                    continue;
                }

                let Some(score) = super::parse_dict_entry_score(&parts) else {
                    continue;
                };

                if !options.should_keep_entry(&word, score) {
                    continue;
                }

                let wid = Self::intern_word(word, &mut vocab.word_to_id, &mut vocab.words);

                dict_map.entry(pinyin).or_default().push((wid, score));
                total_words = total_words.saturating_add(1);
            }
        }

        Ok((total_lines, total_words))
    }

    /// 在全局词表中登记新词并返回其 Word ID（已存在则复用原 ID，保障 ID 稳定）
    fn intern_word(
        word: String,
        word_to_id: &mut HashMap<String, u32>,
        words: &mut Vec<String>,
    ) -> u32 {
        if let Some(&id) = word_to_id.get(&word) {
            id
        } else {
            let new_id = words.len() as u32;
            words.push(word.clone());
            word_to_id.insert(word, new_id);
            new_id
        }
    }

    /// 对同一拼音键下的候选列表按词频降序排序，并按 Word ID 去重（保留最高分）
    fn sort_and_dedup_candidates(dict_map: &mut KeyCandidateMap) {
        for entries in dict_map.values_mut() {
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            entries.dedup_by(|a, b| a.0 == b.0);
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        }
    }

    /// 提取简拼与声母单字索引，合并并生成前缀隔离的排序键集合
    fn build_combined_keys(
        dict_map: KeyCandidateMap,
        words: &[String],
        options: DictBuildOptions,
    ) -> (CombinedKeyMap, Vec<String>) {
        // 提取简拼缩写索引 (key_type = 1)
        let acronym_map = Self::extract_acronym_map(&dict_map, options);

        // 提取声母单字索引 (key_type = 2)
        let initial_map = Self::extract_initial_map(&dict_map, words);

        // 汇总隔离键空间：全拼("{k}"), 简拼("'{k}"), 声母("''{k}")
        let combined_keys = Self::merge_isolated_key_spaces(dict_map, acronym_map, initial_map);

        let mut sorted_keys: Vec<String> = combined_keys.keys().cloned().collect();
        sorted_keys.sort();

        (combined_keys, sorted_keys)
    }

    /// 提取简拼缩写索引（对隔音符分隔的多音节拼音取各段首字母并拼成缩写键）
    fn extract_acronym_map(
        dict_map: &KeyCandidateMap,
        options: DictBuildOptions,
    ) -> KeyCandidateMap {
        let mut acronym_map: KeyCandidateMap = HashMap::new();
        for (pinyin, cand_list) in dict_map {
            if !pinyin.contains('\'') {
                continue;
            }
            let parts: Vec<&str> = pinyin.split('\'').collect();
            if parts.len() < 2 {
                continue;
            }
            let mut acronym = String::with_capacity(parts.len());
            for part in parts {
                if let Some(ch) = part.chars().next() {
                    acronym.push(ch);
                }
            }
            if acronym.len() < 2
                || (options.max_acronym_len > 0 && acronym.len() > options.max_acronym_len)
            {
                continue;
            }
            let min_score = if options.min_acronym_score > 0 {
                options.min_acronym_score
            } else if acronym.len() == 2 {
                200
            } else {
                50
            };
            let list = acronym_map.entry(acronym).or_default();
            for &(wid, score) in cand_list.iter().take(8) {
                if score >= min_score {
                    list.push((wid, score));
                }
            }
        }
        for list in acronym_map.values_mut() {
            list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            list.dedup_by(|a, b| a.0 == b.0);
            list.sort_by_key(|b| std::cmp::Reverse(b.1));
            list.truncate(8);
        }
        acronym_map
    }

    /// 提取声母单字索引（仅收录以指定声母开头的标准音节所对应的单字词条）
    fn extract_initial_map(dict_map: &KeyCandidateMap, words: &[String]) -> KeyCandidateMap {
        let mut initial_map: KeyCandidateMap = HashMap::new();
        for &initial in STANDARD_INITIALS {
            let mut list = Vec::new();
            for &syl in STANDARD_SYLLABLES {
                if !syl.starts_with(initial) {
                    continue;
                }
                let Some(matches) = dict_map.get(syl) else {
                    continue;
                };
                for &(wid, score) in matches {
                    if let Some(text) = words.get(wid as usize) {
                        if text.chars().count() == 1 {
                            list.push((wid, score));
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
        initial_map
    }

    /// 汇总三类前缀隔离的键空间：全拼("{k}")、简拼("'{k}")、声母("''{k}")
    fn merge_isolated_key_spaces(
        dict_map: KeyCandidateMap,
        acronym_map: KeyCandidateMap,
        initial_map: KeyCandidateMap,
    ) -> CombinedKeyMap {
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
        combined_keys
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
}
