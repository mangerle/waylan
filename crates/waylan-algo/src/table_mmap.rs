use crate::dat::DatReader;
use crate::error::AlgoError;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// 紧凑词典魔数标识（16 字节）
pub const COMPACT_DICT_MAGIC: &[u8; 16] = b"WAYLAN_COMPACT02";

/// 零拷贝紧凑只读词典 (Compact Mmap Lexicon - DAT 一体化版)
///
/// # 设计原理
/// - **实现初衷**：将拼音双数组 Trie (DAT)、高密度候选池与单源词表融合为一个紧凑二进制资产；
/// - **核心优势**：
///   1. 彻底消灭明文拼音存储，检索严格为 $O(L)$ 状态跳转；
///   2. 词条文本单源只读存储，同时承担反查与正查，消灭独立词表文件；
///   3. 零堆分配，零反序列化，由操作系统虚拟内存按需分页置换。
pub struct CompactMmapLexicon {
    mmap: Mmap,
    pub num_keys: usize,
    pub num_candidates: usize,
    pub vocab_size: usize,
    dat_node_count: usize,
    dat_tail_len: usize,
    dat_base_offset: usize,
    dat_check_offset: usize,
    dat_tail_offset: usize,
    key_metas_offset: usize,
    candidates_offset: usize,
    vocab_offsets_offset: usize,
    sorted_vocab_ids_offset: usize,
    vocab_strings_offset: usize,
}

impl std::fmt::Debug for CompactMmapLexicon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactMmapLexicon")
            .field("num_keys", &self.num_keys)
            .field("num_candidates", &self.num_candidates)
            .field("vocab_size", &self.vocab_size)
            .finish()
    }
}

impl CompactMmapLexicon {
    /// 打开紧凑二进制词库文件并建立零拷贝内存映射
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?
        };
        Self::from_mmap(mmap, path_ref)
    }

    /// 从已就绪的内存切片解析紧凑只读词典结构
    pub fn from_mmap(mmap: Mmap, path: &Path) -> Result<Self, AlgoError> {
        if mmap.len() < 64 || &mmap[0..16] != COMPACT_DICT_MAGIC {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom(
                    "无效的紧凑词典魔数标识 (需 WAYLAN_COMPACT02)".to_string(),
                )
                .into(),
            });
        }

        let version = u32::from_le_bytes(mmap[16..20].try_into().unwrap_or([0; 4]));
        if version != 2 {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom(format!("不支持的词库版本号: {}", version))
                    .into(),
            });
        }

        let num_keys = u32::from_le_bytes(mmap[20..24].try_into().unwrap_or([0; 4])) as usize;
        let num_candidates = u32::from_le_bytes(mmap[24..28].try_into().unwrap_or([0; 4])) as usize;
        let vocab_size = u32::from_le_bytes(mmap[28..32].try_into().unwrap_or([0; 4])) as usize;
        let dat_node_count = u32::from_le_bytes(mmap[32..36].try_into().unwrap_or([0; 4])) as usize;
        let dat_tail_len = u32::from_le_bytes(mmap[36..40].try_into().unwrap_or([0; 4])) as usize;

        let key_metas_offset =
            u64::from_le_bytes(mmap[40..48].try_into().unwrap_or([0; 8])) as usize;
        let candidates_offset =
            u64::from_le_bytes(mmap[48..56].try_into().unwrap_or([0; 8])) as usize;
        let vocab_offset = u64::from_le_bytes(mmap[56..64].try_into().unwrap_or([0; 8])) as usize;

        let dat_base_offset = 64usize;
        let dat_check_offset = dat_base_offset.saturating_add(dat_node_count.saturating_mul(4));
        let dat_tail_offset = dat_check_offset.saturating_add(dat_node_count.saturating_mul(4));
        let dat_tail_end = dat_tail_offset.saturating_add(dat_tail_len);
        let key_metas_end = key_metas_offset.saturating_add(num_keys.saturating_mul(8));
        let candidates_end = candidates_offset.saturating_add(num_candidates.saturating_mul(6));

        let vocab_offsets_offset = vocab_offset;
        let sorted_vocab_ids_offset =
            vocab_offsets_offset.saturating_add(vocab_size.saturating_add(1).saturating_mul(4));
        let vocab_strings_offset =
            sorted_vocab_ids_offset.saturating_add(vocab_size.saturating_mul(4));

        let min_required_len = dat_tail_end
            .max(key_metas_end)
            .max(candidates_end)
            .max(vocab_strings_offset);

        if mmap.len() < min_required_len {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("紧凑词典数据区截断".to_string()).into(),
            });
        }

        // 校验内存映射基地址及内部 32 位整数切片偏移量是否符合 4 字节对齐契约（杜绝未对齐引发 UB）
        let base_ptr = mmap.as_ptr() as usize;
        if !base_ptr.is_multiple_of(std::mem::align_of::<u32>()) {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("内存映射基地址未按 4 字节对齐".to_string())
                    .into(),
            });
        }
        if !dat_base_offset.is_multiple_of(std::mem::align_of::<i32>())
            || !dat_check_offset.is_multiple_of(std::mem::align_of::<u32>())
            || !sorted_vocab_ids_offset.is_multiple_of(std::mem::align_of::<u32>())
            || !vocab_offsets_offset.is_multiple_of(std::mem::align_of::<u32>())
        {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom(
                    "紧凑词典内部数据段偏移量未按 4 字节对齐".to_string(),
                )
                .into(),
            });
        }

        Ok(Self {
            mmap,
            num_keys,
            num_candidates,
            vocab_size,
            dat_node_count,
            dat_tail_len,
            dat_base_offset,
            dat_check_offset,
            dat_tail_offset,
            key_metas_offset,
            candidates_offset,
            vocab_offsets_offset,
            sorted_vocab_ids_offset,
            vocab_strings_offset,
        })
    }

    /// 获取只读 DAT 视图
    #[inline]
    fn get_dat_reader(&self) -> DatReader<'_> {
        let base_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_base_offset..].as_ptr() as *const i32,
                self.dat_node_count,
            )
        };
        let check_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_check_offset..].as_ptr() as *const u32,
                self.dat_node_count,
            )
        };
        let tail_slice = &self.mmap[self.dat_tail_offset..self.dat_tail_offset + self.dat_tail_len];
        DatReader::new(base_slice, check_slice, tail_slice)
    }

    /// 根据 Word ID 反查对应的词条文本（O(1) 零拷贝切片返回）
    #[inline]
    pub fn get_word(&self, word_id: u32) -> Option<&str> {
        let wid = word_id as usize;
        if wid >= self.vocab_size {
            return None;
        }

        let off1_pos = self
            .vocab_offsets_offset
            .saturating_add(wid.saturating_mul(4));
        let off2_pos = off1_pos.saturating_add(4);
        let start =
            u32::from_le_bytes(self.mmap.get(off1_pos..off1_pos + 4)?.try_into().ok()?) as usize;
        let end =
            u32::from_le_bytes(self.mmap.get(off2_pos..off2_pos + 4)?.try_into().ok()?) as usize;

        let s_start = self.vocab_strings_offset.saturating_add(start);
        let s_end = self.vocab_strings_offset.saturating_add(end);
        let slice = self.mmap.get(s_start..s_end)?;

        std::str::from_utf8(slice).ok()
    }

    /// 根据词条文本快速二分反查对应的 Word ID（纳秒级）
    pub fn get_id(&self, word: &str) -> Option<u32> {
        if self.vocab_size == 0 {
            return None;
        }

        let mut left = 0usize;
        let mut right = self.vocab_size;

        while left < right {
            let mid = left + (right - left) / 2;
            let id_pos = self
                .sorted_vocab_ids_offset
                .saturating_add(mid.saturating_mul(4));
            let cand_id = u32::from_le_bytes(self.mmap.get(id_pos..id_pos + 4)?.try_into().ok()?);
            let cand_word = self.get_word(cand_id).unwrap_or("");

            match cand_word.cmp(word) {
                std::cmp::Ordering::Equal => return Some(cand_id),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }

        None
    }

    /// 获取指定 key_idx 的候选列表（支持按需截取前 `limit` 条以减少无效堆分配）
    fn get_candidates_by_key_idx(
        &self,
        key_idx: usize,
        expected_type: Option<u8>,
        limit: Option<usize>,
    ) -> Option<Vec<(String, u32)>> {
        if key_idx >= self.num_keys {
            return None;
        }

        let meta_pos = self
            .key_metas_offset
            .saturating_add(key_idx.saturating_mul(8));
        let meta_slice = self.mmap.get(meta_pos..meta_pos + 8)?;
        let cand_start = u32::from_le_bytes(meta_slice[0..4].try_into().ok()?) as usize;
        let cand_count = u16::from_le_bytes(meta_slice[4..6].try_into().ok()?) as usize;
        let key_type = meta_slice[6];

        if let Some(exp_type) = expected_type {
            if key_type != exp_type {
                return None;
            }
        }

        let take_count = limit.map_or(cand_count, |max_k| cand_count.min(max_k));
        let mut res = Vec::with_capacity(take_count);
        for i in 0..take_count {
            let item_pos = self
                .candidates_offset
                .saturating_add(cand_start.saturating_add(i).saturating_mul(6));
            let item_slice = self.mmap.get(item_pos..item_pos + 6)?;
            let word_id = u32::from_le_bytes(item_slice[0..4].try_into().ok()?);
            let score_u16 = u16::from_le_bytes(item_slice[4..6].try_into().ok()?);
            let score = decode_compact_score(score_u16);

            if let Some(text) = self.get_word(word_id) {
                res.push((text.to_string(), score));
            }
        }

        Some(res)
    }

    /// 全拼检索（通过 DAT 精确匹配）
    pub fn lookup(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = reader.exact_match(key)? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(0), None)
    }

    /// 全拼按需截取 Top-K 检索（用于 Viterbi 束搜索热路径，消除同音长尾无用分配）
    pub fn lookup_top_k(&self, key: &str, k: usize) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = reader.exact_match(key)? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(0), Some(k))
    }

    /// 简拼缩写检索
    pub fn lookup_acronym(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let prefixed = format!("'{}", key);
        let key_idx = reader
            .exact_match(&prefixed)
            .or_else(|| reader.exact_match(key))? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(1), None)
    }

    /// 声母高频单字检索
    pub fn lookup_initial(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let prefixed = format!("''{}", initial);
        let key_idx = reader
            .exact_match(&prefixed)
            .or_else(|| reader.exact_match(initial))? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(2), None)
    }
}

/// 将 16 位紧凑存储分还原为 32 位真实词频量纲
///
/// # 设计说明
/// - `builder_compact::encode_compact_score` 对 `<= 32_767` 保持线性原值，对 `> 32_767` 采用对数压缩；
/// - 此处执行对称的逆指数反演，还原真实词频量纲，消除 Viterbi 解码阶段双重取对数导致的词频梯度坍缩。
#[inline]
pub(crate) fn decode_compact_score(score_u16: u16) -> u32 {
    if score_u16 <= 32_767 {
        score_u16 as u32
    } else {
        let extra = (score_u16 - 32_768) as f64 / 2_000.0;
        let delta = (extra.exp() - 1.0).round().max(1.0) as u32;
        32_767u32.saturating_add(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder_compact::encode_compact_score;

    #[test]
    fn test_compact_score_encode_decode_roundtrip() {
        // 线性区间应 100% 精确无损还原
        for s in [0u32, 1, 100, 1_000, 10_000, 32_767] {
            let enc = encode_compact_score(s);
            assert_eq!(decode_compact_score(enc), s);
        }

        // 高频对数压缩区间应保持严格单调且近似还原真实量纲（相对误差 < 0.2%）
        let high_scores = [32_768u32, 50_000, 100_000, 500_000, 1_000_000, 5_000_000];
        let mut prev_dec = 32_767u32;
        for &s in &high_scores {
            let enc = encode_compact_score(s);
            let dec = decode_compact_score(enc);
            assert!(
                dec > prev_dec,
                "解码分必须严格单调递增: {} <= {}",
                dec,
                prev_dec
            );
            let diff = (dec as i64 - s as i64).unsigned_abs() as f64;
            assert!(
                diff / (s as f64) < 0.002,
                "原始分 {} 解码后为 {}，误差过大",
                s,
                dec
            );
            prev_dec = dec;
        }
    }
}
