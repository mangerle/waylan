use super::mmap_header::CompactDictHeader;
pub use super::mmap_header::COMPACT_DICT_MAGIC;
use crate::dat::DatReader;
use crate::error::AlgoError;
use memmap2::{Mmap, MmapOptions};
use std::fs::File;
use std::path::{Path, PathBuf};

#[cfg(not(target_endian = "little"))]
compile_error!("仅支持小端序架构");

/// 零拷贝紧凑只读词典 (Compact Mmap Lexicon - DAT 一体化版)
///
/// # 设计原理
/// - **实现初衷**：将拼音双数组 Trie (DAT)、高密度候选池与单源词表融合为一个紧凑二进制资产；
/// - **核心优势**：
///   1. 彻底消灭明文拼音存储，检索严格为 $O(L)$ 状态跳转；
///   2. 词条文本单源只读存储，同时承担反查与正查，消灭独立词表文件；
///   3. 零堆分配，零反序列化，由操作系统虚拟内存按需分页置换；
///   4. 原生 32 位词频存储与 8 字节天然对齐，消灭对数压缩失真与溢出截断，零浮点解压开销。
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
        // SAFETY: 紧凑词库文件属于编译期生成的只读静态资产，运行期仅以只读方式映射且不会在外部被并发截断或原地修改。
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?
        };
        Self::from_mmap(mmap, path_ref)
    }

    /// 从内存字节切片构建零拷贝只读词典（通过匿名只读内存映射实现，零磁盘 I/O）
    ///
    /// # Errors
    /// 当字节长度不足、匿名内存映射失败或紧凑词典头部/段校验失败时返回 [`AlgoError`]。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AlgoError> {
        let mem_path = Path::new("<memory>");
        if bytes.len() < 64 {
            return Err(AlgoError::deserialize_failed(
                mem_path,
                "紧凑词典字节数据不足 64 字节",
            ));
        }
        let mut mmap_mut = MmapOptions::new()
            .len(bytes.len())
            .map_anon()
            .map_err(|e| AlgoError::DictionaryIo {
                path: mem_path.to_path_buf(),
                source: e,
            })?;
        mmap_mut.copy_from_slice(bytes);
        let mmap = mmap_mut
            .make_read_only()
            .map_err(|e| AlgoError::DictionaryIo {
                path: mem_path.to_path_buf(),
                source: e,
            })?;
        Self::from_mmap(mmap, mem_path)
    }

    /// 从已就绪的内存切片解析紧凑只读词典结构
    pub fn from_mmap(mmap: Mmap, path: &Path) -> Result<Self, AlgoError> {
        let header = CompactDictHeader::parse(&mmap, path)?;
        Ok(Self {
            mmap,
            num_keys: header.num_keys,
            num_candidates: header.num_candidates,
            vocab_size: header.vocab_size,
            dat_node_count: header.dat_node_count,
            dat_tail_len: header.dat_tail_len,
            dat_base_offset: header.dat_base_offset,
            dat_check_offset: header.dat_check_offset,
            dat_tail_offset: header.dat_tail_offset,
            key_metas_offset: header.key_metas_offset,
            candidates_offset: header.candidates_offset,
            vocab_offsets_offset: header.vocab_offsets_offset,
            sorted_vocab_ids_offset: header.sorted_vocab_ids_offset,
            vocab_strings_offset: header.vocab_strings_offset,
        })
    }

    /// 获取只读 DAT 视图
    #[inline]
    pub(crate) fn get_dat_reader(&self) -> DatReader<'_> {
        // SAFETY: `from_mmap` 构造期已严格验证 `mmap` 总长度覆盖 `dat_base_offset + dat_node_count * 4`
        // 且映射基地址与 `dat_base_offset` 均满足 `i32` 的 4 字节对齐要求，目标架构为小端序。
        let base_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_base_offset..].as_ptr() as *const i32,
                self.dat_node_count,
            )
        };
        // SAFETY: `from_mmap` 构造期已严格验证 `mmap` 总长度覆盖 `dat_check_offset + dat_node_count * 4`
        // 且映射基地址与 `dat_check_offset` 均满足 `u32` 的 4 字节对齐要求，目标架构为小端序。
        let check_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_check_offset..].as_ptr() as *const u32,
                self.dat_node_count,
            )
        };
        let tail_end = self.dat_tail_offset.saturating_add(self.dat_tail_len);
        let tail_slice = &self.mmap[self.dat_tail_offset..tail_end];
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
        let off3_pos = off2_pos.saturating_add(4);
        let start =
            u32::from_le_bytes(self.mmap.get(off1_pos..off2_pos)?.try_into().ok()?) as usize;
        let end = u32::from_le_bytes(self.mmap.get(off2_pos..off3_pos)?.try_into().ok()?) as usize;

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
            let id_end = id_pos.saturating_add(4);
            let cand_id = u32::from_le_bytes(self.mmap.get(id_pos..id_end)?.try_into().ok()?);
            let cand_word = self.get_word(cand_id).unwrap_or("");

            match cand_word.cmp(word) {
                std::cmp::Ordering::Equal => return Some(cand_id),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }

        None
    }

    /// 按指定 key_idx 零拷贝迭代前 `limit` 条候选（仅传递 mmap 内的 `&str` 切片与 `(score, word_id)`）
    pub(crate) fn for_each_candidate_by_key_idx(
        &self,
        key_idx: usize,
        expected_type: Option<u8>,
        limit: Option<usize>,
        mut visitor: impl FnMut(&str, u32, u32),
    ) -> bool {
        if key_idx >= self.num_keys {
            return false;
        }

        let meta_pos = self
            .key_metas_offset
            .saturating_add(key_idx.saturating_mul(8));
        let Some(meta_slice) = self.mmap.get(meta_pos..meta_pos.saturating_add(8)) else {
            return false;
        };
        let Ok(cand_start_bytes) = meta_slice[0..4].try_into() else {
            return false;
        };
        let Ok(cand_count_bytes) = meta_slice[4..6].try_into() else {
            return false;
        };
        let cand_start = u32::from_le_bytes(cand_start_bytes) as usize;
        let cand_count = u16::from_le_bytes(cand_count_bytes) as usize;
        let key_type = meta_slice[6];

        if let Some(exp_type) = expected_type {
            if key_type != exp_type {
                return false;
            }
        }

        let take_count = limit.map_or(cand_count, |max_k| cand_count.min(max_k));
        for i in 0..take_count {
            let item_pos = self
                .candidates_offset
                .saturating_add(cand_start.saturating_add(i).saturating_mul(8));
            let Some(item_slice) = self.mmap.get(item_pos..item_pos.saturating_add(8)) else {
                return false;
            };
            let Ok(word_id_bytes) = item_slice[0..4].try_into() else {
                return false;
            };
            let Ok(score_bytes) = item_slice[4..8].try_into() else {
                return false;
            };
            let word_id = u32::from_le_bytes(word_id_bytes);
            let score = u32::from_le_bytes(score_bytes);

            if let Some(text) = self.get_word(word_id) {
                visitor(text, score, word_id);
            }
        }

        true
    }

    /// 获取指定 key_idx 的候选列表（支持按需截取前 `limit` 条以减少无效堆分配，携带 word_id）
    pub(crate) fn get_candidates_by_key_idx(
        &self,
        key_idx: usize,
        expected_type: Option<u8>,
        limit: Option<usize>,
    ) -> Option<Vec<(String, u32, u32)>> {
        let mut res = Vec::with_capacity(limit.unwrap_or(16));
        if self.for_each_candidate_by_key_idx(key_idx, expected_type, limit, |text, score, id| {
            res.push((text.to_string(), score, id));
        }) {
            Some(res)
        } else {
            None
        }
    }

    /// 全拼检索（通过 DAT 精确匹配）
    pub fn lookup(&self, key: &str) -> Option<Vec<(String, u32, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = reader.exact_match(key)? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(0), None)
    }

    /// 全拼按需截取 Top-K 检索（用于 Viterbi 束搜索热路径，消除同音长尾无用分配）
    pub fn lookup_top_k(&self, key: &str, k: usize) -> Option<Vec<(String, u32, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = reader.exact_match(key)? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(0), Some(k))
    }

    /// 简拼缩写检索
    pub fn lookup_acronym(&self, key: &str) -> Option<Vec<(String, u32, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = match_with_prefix(&reader, b"'", key)?;
        self.get_candidates_by_key_idx(key_idx, Some(1), None)
    }

    /// 简拼缩写按需截取 Top-K 检索
    pub fn lookup_acronym_top_k(&self, key: &str, k: usize) -> Option<Vec<(String, u32, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = match_with_prefix(&reader, b"'", key)?;
        self.get_candidates_by_key_idx(key_idx, Some(1), Some(k))
    }

    /// 声母高频单字检索
    pub fn lookup_initial(&self, initial: &str) -> Option<Vec<(String, u32, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = match_with_prefix(&reader, b"''", initial)?;
        self.get_candidates_by_key_idx(key_idx, Some(2), None)
    }
}

/// 在栈上为拼音键拼接前缀并在 DAT 中执行精确匹配（彻底消除高频击键时 `format!` 的临时堆字符串分配）
#[inline]
fn match_with_prefix(reader: &DatReader<'_>, prefix: &[u8], key: &str) -> Option<usize> {
    let key_bytes = key.as_bytes();
    let total_len = prefix.len().saturating_add(key_bytes.len());
    if total_len <= 64 {
        let mut buf = [0u8; 64];
        buf[..prefix.len()].copy_from_slice(prefix);
        buf[prefix.len()..total_len].copy_from_slice(key_bytes);
        if let Ok(prefixed_str) = std::str::from_utf8(&buf[..total_len]) {
            return reader.exact_match(prefixed_str).map(|idx| idx as usize);
        }
    }
    // 降级兜底路径：超长输入（> 64 字节罕见极端情况）
    let mut s = String::with_capacity(total_len);
    if let Ok(prefix_str) = std::str::from_utf8(prefix) {
        s.push_str(prefix_str);
    }
    s.push_str(key);
    reader.exact_match(&s).map(|idx| idx as usize)
}
