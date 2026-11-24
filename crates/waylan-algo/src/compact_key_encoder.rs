//! 紧凑词典三类隔离键空间生成与候选字节编码模块
//!
//! # 设计原理
//! - **实现初衷**：将全拼、简拼、声母三类索引键的构建、合并与二进制序列化编码从 `compact_writer.rs`
//!   剥离，使 `compact_writer.rs` 专注于 mmap 文件段对齐与流式落盘。

use std::collections::HashMap;

use crate::syllables::{STANDARD_INITIALS, STANDARD_SYLLABLES};

/// 候选词二元组（词汇 ID, 频次分值）
pub(crate) type CandidateScore = (u32, u32);
/// 键名到候选词元组列表的映射
pub(crate) type KeyCandidateMap = HashMap<String, Vec<CandidateScore>>;
/// 合并后的键表（存储键类型标记与候选词列表）
pub(crate) type CombinedKeyMap = HashMap<String, (u8, Vec<CandidateScore>)>;

/// 生成全拼、简拼、声母三类隔离索引键并按字典序严格排序
pub(crate) fn build_combined_keys(
    dict_map: KeyCandidateMap,
    words: &[String],
    max_acronym_len: usize,
    min_acronym_score: u32,
) -> (CombinedKeyMap, Vec<String>) {
    let acronym_map = extract_acronym_map(&dict_map, max_acronym_len, min_acronym_score);
    let initial_map = extract_initial_map(&dict_map, words);
    let combined_keys = merge_isolated_key_spaces(dict_map, acronym_map, initial_map);

    let mut sorted_keys: Vec<String> = combined_keys.keys().cloned().collect();
    sorted_keys.sort();

    (combined_keys, sorted_keys)
}

/// 提取简拼缩写索引（对隔音符分隔的多音节拼音取各段首字母并拼成缩写键）
fn extract_acronym_map(
    dict_map: &KeyCandidateMap,
    max_acronym_len: usize,
    min_acronym_score: u32,
) -> KeyCandidateMap {
    let mut acronym_map: KeyCandidateMap = HashMap::new();
    for (pinyin, cand_list) in dict_map {
        if !pinyin.contains('\'') {
            continue;
        }
        let mut acronym = String::with_capacity(6);
        let mut part_count = 0usize;
        for part in pinyin.split('\'') {
            part_count += 1;
            if let Some(ch) = part.chars().next() {
                acronym.push(ch);
            }
        }
        if part_count < 2
            || acronym.len() < 2
            || (max_acronym_len > 0 && acronym.len() > max_acronym_len)
        {
            continue;
        }
        let min_score = if min_acronym_score > 0 {
            min_acronym_score
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
                if let Some(text) = words.get(wid as usize)
                    && text.chars().count() == 1
                {
                    list.push((wid, score));
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
pub(crate) fn encode_keys_and_candidates(
    combined_keys: &CombinedKeyMap,
    sorted_keys: &[String],
) -> (Vec<u8>, Vec<u8>, usize) {
    let num_keys = sorted_keys.len();
    let mut key_metas_data = Vec::with_capacity(num_keys * 8);
    let mut candidates_data = Vec::with_capacity(num_keys * 32);
    let mut cand_counter: u32 = 0;

    for k in sorted_keys {
        let Some((k_type, cands)) = combined_keys.get(k) else {
            continue;
        };
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
