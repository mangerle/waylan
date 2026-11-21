use crate::error::AlgoError;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};

#[cfg(not(target_endian = "little"))]
compile_error!("仅支持小端序架构");

/// 微澜 2-Gram 紧凑高密度转移概率矩阵魔数标识（16 字节对齐）
pub const BIGRAM_MAGIC: &[u8; 16] = b"WAYLAN_BIGRAM02\0";

/// 头部固定长度（64 字节）
pub const BIGRAM_HEADER_SIZE: usize = 64;

/// 256 阶标量量化码本尺寸（256 * 4 字节 = 1024 字节）
pub const CODEBOOK_SIZE: usize = 1024;

/// 零拷贝只读 CSR 高密度 Bigram 转移矩阵 (CompactMmapBigram)
///
/// # 设计原理
/// - **实现初衷**：基于现代工业级标准，以 Compressed Sparse Row (CSR) 拓扑与 8 位标量量化彻底替代稀疏哈希表；
/// - **核心优势**：
///   1. 极致存储密度：平均每个二元搭配仅消耗约 4~5 字节，存储密度较初代开放寻址表提升 5 倍以上；
///   2. 零空白填充：行偏移数组与列数据连续紧密排布，装载率 100%，消除空桶膨胀；
///   3. 纳秒级局部二分：前驱词按行索引 $O(1)$ 随机定位，后继词数组内存连续，局部二分查找在 30 纳秒内瞬间完成；
///   4. 零堆分配与零拷贝：全生命周期通过虚拟内存 mmap 映射，由操作系统内核按需分页置换。
pub struct CompactMmapBigram {
    mmap: Mmap,
    /// 文件格式版本号（2 为经典亲和度矩阵，3 为同源 Unigram + 绝对折扣动态回退矩阵）
    pub version: u32,
    /// 词表总容量 V
    pub vocab_size: usize,
    /// 收录的有效二元词对总数 N
    pub total_pairs: usize,
    /// 码本偏移量
    codebook_offset: usize,
    /// 行偏移数组起始偏移
    row_offsets_offset: usize,
    /// 列与分数数据数组起始偏移
    cols_offset: usize,
    /// v3 同源一元对数分数组起始偏移（[i32; vocab_size]）
    unigram_offset: Option<usize>,
    /// v3 前驱词绝对折扣回退权重数组起始偏移（[i32; vocab_size]）
    backoff_offset: Option<usize>,
}

/// Bigram 文件头解析结果（记录各数据段的绝对偏移量与规模）
struct BigramHeader {
    /// 文件格式版本号
    version: u32,
    /// 词表总容量 V
    vocab_size: usize,
    /// 收录的有效二元词对总数 N
    total_pairs: usize,
    /// 码本偏移量
    codebook_offset: usize,
    /// 行偏移数组起始偏移
    row_offsets_offset: usize,
    /// 列与分数数据数组起始偏移
    cols_offset: usize,
    /// v3 同源一元对数分数组起始偏移
    unigram_offset: Option<usize>,
    /// v3 前驱词绝对折扣回退权重数组起始偏移
    backoff_offset: Option<usize>,
}

impl BigramHeader {
    /// 解析并校验 Bigram 文件头（魔数、版本与各数据段起始偏移量）
    fn parse(mmap: &Mmap, path: &Path) -> Result<Self, AlgoError> {
        let version = Self::validate_magic_and_version(mmap, path)?;

        let vocab_size = read_u32_le(mmap, 20)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram词表大小读取失败"))?
            as usize;
        let total_pairs = read_u32_le(mmap, 24)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram词对总数读取失败"))?
            as usize;
        let codebook_offset = read_u64_le(mmap, 28)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram码本偏移读取失败"))?
            as usize;
        let row_offsets_offset = read_u64_le(mmap, 36)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram行偏移表读取失败"))?
            as usize;
        let cols_offset = read_u64_le(mmap, 44)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram列数据偏移读取失败"))?
            as usize;

        let (unigram_offset, backoff_offset) = if version >= 3 {
            let uni_off = read_u64_le(mmap, 52)
                .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram一元表偏移读取失败"))?
                as usize;
            let back_off = uni_off
                .saturating_add(CODEBOOK_SIZE)
                .saturating_add(vocab_size);
            (Some(uni_off), Some(back_off))
        } else {
            (None, None)
        };

        Ok(Self {
            version,
            vocab_size,
            total_pairs,
            codebook_offset,
            row_offsets_offset,
            cols_offset,
            unigram_offset,
            backoff_offset,
        })
    }

    /// 校验 Bigram 文件的体积下限、魔数标识与版本号
    fn validate_magic_and_version(mmap: &Mmap, path: &Path) -> Result<u32, AlgoError> {
        if mmap.len() < BIGRAM_HEADER_SIZE.saturating_add(CODEBOOK_SIZE) {
            return Err(AlgoError::deserialize_failed(path, "Bigram文件体积过小"));
        }

        if &mmap[0..16] != BIGRAM_MAGIC {
            return Err(AlgoError::deserialize_failed(path, "Bigram文件魔数不匹配"));
        }

        let version = read_u32_le(mmap, 16)
            .ok_or_else(|| AlgoError::deserialize_failed(path, "Bigram版本字段缺失"))?;
        if version != 2 && version != 3 {
            return Err(AlgoError::deserialize_failed(
                path,
                format!("不支持的Bigram版本号: {}", version),
            ));
        }
        Ok(version)
    }

    /// 校验各数据段的结束偏移是否均落在内存映射范围内
    fn validate_sections(&self, mmap: &Mmap, path: &Path) -> Result<(), AlgoError> {
        let codebook_end = self.codebook_offset.saturating_add(CODEBOOK_SIZE);
        let row_offsets_end = self
            .row_offsets_offset
            .saturating_add(self.vocab_size.saturating_add(1).saturating_mul(4));
        let cols_end = self
            .cols_offset
            .saturating_add(self.total_pairs.saturating_mul(4));
        let mut expected_size = codebook_end.max(row_offsets_end).max(cols_end);
        if let Some(back_off) = self.backoff_offset {
            let backoff_end = back_off
                .saturating_add(CODEBOOK_SIZE)
                .saturating_add(self.vocab_size);
            expected_size = expected_size.max(backoff_end);
        }
        if mmap.len() < expected_size {
            return Err(AlgoError::deserialize_failed(path, "Bigram数据区截断损坏"));
        }
        Ok(())
    }
}

impl std::fmt::Debug for CompactMmapBigram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactMmapBigram")
            .field("vocab_size", &self.vocab_size)
            .field("total_pairs", &self.total_pairs)
            .finish()
    }
}

impl CompactMmapBigram {
    /// 打开二进制矩阵文件并建立零拷贝内存映射
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        // SAFETY: 二元语言模型矩阵文件属于编译期固化的静态只读资产，运行期仅以只读方式映射且不会在外部被并发截断或原地修改。
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?
        };
        Self::from_mmap(mmap, path_ref)
    }

    /// 从已映射的内存切片解析矩阵元数据并验证合法性
    pub fn from_mmap(mmap: Mmap, path: &Path) -> Result<Self, AlgoError> {
        let header = BigramHeader::parse(&mmap, path)?;
        header.validate_sections(&mmap, path)?;
        Ok(Self {
            mmap,
            version: header.version,
            vocab_size: header.vocab_size,
            total_pairs: header.total_pairs,
            codebook_offset: header.codebook_offset,
            row_offsets_offset: header.row_offsets_offset,
            cols_offset: header.cols_offset,
            unigram_offset: header.unigram_offset,
            backoff_offset: header.backoff_offset,
        })
    }

    /// 获取前驱词在矩阵中的出度（收录的不同后继词数量）
    #[inline]
    pub fn out_degree(&self, prev_id: u32) -> Option<usize> {
        let (start, end) = self.get_row_range(prev_id)?;
        Some(end.saturating_sub(start))
    }

    /// 从 `码本(1024B) + u8量化数组(vocab_size B)` 区间内读取并解码词级标量（`255` 为未登录哨兵）
    #[inline]
    fn decode_quantized_table(&self, section_base: Option<usize>, word_id: u32) -> Option<i64> {
        let base = section_base?;
        let wid = word_id as usize;
        if wid >= self.vocab_size {
            return None;
        }
        let q_idx = *self
            .mmap
            .get(base.saturating_add(CODEBOOK_SIZE).saturating_add(wid))?;
        if q_idx == u8::MAX {
            return None;
        }
        let cb_off = base.saturating_add((q_idx as usize).saturating_mul(4));
        Some(read_i32_le(&self.mmap, cb_off)? as i64)
    }

    /// 查询 v3 同源语料一元对数概率分（`u8::MAX` 哨兵表示语料未登录）
    #[inline]
    pub fn get_unigram_score(&self, word_id: u32) -> Option<i64> {
        self.decode_quantized_table(self.unigram_offset, word_id)
    }

    /// 查询 v3 前驱词绝对折扣动态回退对数权重（`d = 0.75`）
    #[inline]
    pub fn get_backoff_weight(&self, prev_id: u32) -> Option<i64> {
        self.decode_quantized_table(self.backoff_offset, prev_id)
    }

    /// 查询 8 位量化码本中指定等级还原后的分数
    #[inline]
    fn decode_score(&self, q_idx: u8) -> i64 {
        let off = self
            .codebook_offset
            .saturating_add((q_idx as usize).saturating_mul(4));
        read_i32_le(&self.mmap, off).unwrap_or(0) as i64
    }

    /// 获取前驱词 ID 的后继词起始偏移索引与结束偏移索引 (start, end)
    #[inline]
    fn get_row_range(&self, prev_id: u32) -> Option<(usize, usize)> {
        let pid = prev_id as usize;
        if pid >= self.vocab_size {
            return None;
        }

        let off1 = self
            .row_offsets_offset
            .saturating_add(pid.saturating_mul(4));
        let off2 = off1.saturating_add(4);
        let start = read_u32_le(&self.mmap, off1)? as usize;
        let end = read_u32_le(&self.mmap, off2)? as usize;
        Some((start, end))
    }

    /// 高速查询二元词对转移亲和度 (输入两个整数 Word ID)
    #[inline]
    pub fn get_affinity_by_id(&self, prev_id: u32, curr_id: u32) -> Option<i64> {
        let (start, end) = self.get_row_range(prev_id)?;
        if start >= end || end > self.total_pairs {
            return None;
        }

        let mut left = start;
        let mut right = end;

        while left < right {
            let mid = left + (right - left) / 2;
            let item_off = self.cols_offset.saturating_add(mid.saturating_mul(4));
            let entry = read_u32_le(&self.mmap, item_off)?;

            // 低 24 位为后继词 ID，高 8 位为量化分数
            let col_id = entry & 0x00FF_FFFF;

            if col_id == curr_id {
                let q_score = (entry >> 24) as u8;
                return Some(self.decode_score(q_score));
            } else if col_id < curr_id {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        None
    }
}

#[inline]
fn read_u32_le(slice: &[u8], offset: usize) -> Option<u32> {
    slice
        .get(offset..offset.saturating_add(4))?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

#[inline]
fn read_u64_le(slice: &[u8], offset: usize) -> Option<u64> {
    slice
        .get(offset..offset.saturating_add(8))?
        .try_into()
        .ok()
        .map(u64::from_le_bytes)
}

#[inline]
fn read_i32_le(slice: &[u8], offset: usize) -> Option<i32> {
    slice
        .get(offset..offset.saturating_add(4))?
        .try_into()
        .ok()
        .map(i32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用最小合法 WAYLAN_BIGRAM02 二进制镜像字节流
    fn build_synthetic_bigram_bytes() -> Vec<u8> {
        let vocab_size: u32 = 3;
        let total_pairs: u32 = 3;
        let codebook_offset: u64 = BIGRAM_HEADER_SIZE as u64;
        let row_offsets_offset: u64 = codebook_offset + CODEBOOK_SIZE as u64;
        let cols_offset: u64 = row_offsets_offset + ((vocab_size as u64 + 1) * 4);

        let mut buf = Vec::new();
        // 1. Header (64B)
        buf.extend_from_slice(BIGRAM_MAGIC);
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&vocab_size.to_le_bytes());
        buf.extend_from_slice(&total_pairs.to_le_bytes());
        buf.extend_from_slice(&codebook_offset.to_le_bytes());
        buf.extend_from_slice(&row_offsets_offset.to_le_bytes());
        buf.extend_from_slice(&cols_offset.to_le_bytes());
        buf.resize(BIGRAM_HEADER_SIZE, 0);

        // 2. Codebook (256 * 4B): q_idx = i 对应分值 i * 100
        for i in 0..256i32 {
            buf.extend_from_slice(&(i * 100).to_le_bytes());
        }

        // 3. Row offsets (vocab_size + 1 = 4 个 u32):
        // word 0 -> [0..2] (含后继词 1, 2), word 1 -> [2..3] (含后继词 2), word 2 -> [3..3] (空)
        for &row_off in &[0u32, 2, 3, 3] {
            buf.extend_from_slice(&row_off.to_le_bytes());
        }

        // 4. Cols (total_pairs = 3 个 u32, 按 col_id 升序排列):
        // (0 -> 1, q=10 => 1000), (0 -> 2, q=25 => 2500), (1 -> 2, q=50 => 5000)
        let pack = |col_id: u32, q: u8| (col_id & 0x00FF_FFFF) | ((q as u32) << 24);
        buf.extend_from_slice(&pack(1, 10).to_le_bytes());
        buf.extend_from_slice(&pack(2, 25).to_le_bytes());
        buf.extend_from_slice(&pack(2, 50).to_le_bytes());

        buf
    }

    #[test]
    fn test_entry_bit_pack() {
        let next_id: u32 = 12345;
        let q_score: u8 = 200;
        let packed = (next_id & 0x00FF_FFFF) | ((q_score as u32) << 24);

        assert_eq!(packed & 0x00FF_FFFF, 12345);
        assert_eq!((packed >> 24) as u8, 200);
        assert_eq!(BIGRAM_MAGIC, b"WAYLAN_BIGRAM02\0");
        assert_eq!(BIGRAM_HEADER_SIZE, 64);
        assert_eq!(CODEBOOK_SIZE, 1024);
    }

    #[test]
    fn test_compact_mmap_bigram_open_lookup_and_corrupted_bounds() {
        let tmp_path = std::env::temp_dir().join("test_waylan_bigram_mmap.bin");
        let valid_bytes = build_synthetic_bigram_bytes();
        std::fs::write(&tmp_path, &valid_bytes).expect("写入测试 Bigram 文件成功");

        let bigram = CompactMmapBigram::open(&tmp_path).expect("加载 Bigram 映射成功");
        assert_eq!(bigram.vocab_size, 3);
        assert_eq!(bigram.total_pairs, 3);

        // 验证二分查找命中与码本解码分值
        assert_eq!(bigram.get_affinity_by_id(0, 1), Some(1000));
        assert_eq!(bigram.get_affinity_by_id(0, 2), Some(2500));
        assert_eq!(bigram.get_affinity_by_id(1, 2), Some(5000));

        // 验证未命中与越界 Word ID 安全返回 None
        assert_eq!(bigram.get_affinity_by_id(0, 0), None);
        assert_eq!(bigram.get_affinity_by_id(2, 0), None);
        assert_eq!(bigram.get_affinity_by_id(99, 1), None);

        // 验证数据区被截断时安全报错拒绝加载
        let truncated = &valid_bytes[..valid_bytes.len() - 4];
        std::fs::write(&tmp_path, truncated).expect("写入截断 Bigram 文件");
        assert!(CompactMmapBigram::open(&tmp_path).is_err());

        let _ = std::fs::remove_file(tmp_path);
    }

    #[test]
    fn test_compact_mmap_bigram_v3_unigram_and_backoff() {
        let tmp_path = std::env::temp_dir().join("test_waylan_bigram_v3_mmap.bin");
        let vocab_size: u32 = 3;
        let total_pairs: u32 = 2;
        let codebook_offset: u64 = BIGRAM_HEADER_SIZE as u64;
        let row_offsets_offset: u64 = codebook_offset + CODEBOOK_SIZE as u64;
        let cols_offset: u64 = row_offsets_offset + ((vocab_size as u64 + 1) * 4);
        let unigram_offset: u64 = cols_offset + (total_pairs as u64 * 4);

        let mut buf = Vec::new();
        buf.extend_from_slice(BIGRAM_MAGIC);
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&vocab_size.to_le_bytes());
        buf.extend_from_slice(&total_pairs.to_le_bytes());
        buf.extend_from_slice(&codebook_offset.to_le_bytes());
        buf.extend_from_slice(&row_offsets_offset.to_le_bytes());
        buf.extend_from_slice(&cols_offset.to_le_bytes());
        buf.extend_from_slice(&unigram_offset.to_le_bytes());
        buf.resize(BIGRAM_HEADER_SIZE, 0);

        for i in 0..256i32 {
            buf.extend_from_slice(&(i * 150).to_le_bytes());
        }
        for &row_off in &[0u32, 2, 2, 2] {
            buf.extend_from_slice(&row_off.to_le_bytes());
        }
        let pack = |col_id: u32, q: u8| (col_id & 0x00FF_FFFF) | ((q as u32) << 24);
        buf.extend_from_slice(&pack(1, 20).to_le_bytes());
        buf.extend_from_slice(&pack(2, 40).to_le_bytes());

        // unigram section: 256*4B codebook + 3B u8 ([10 -> 8500, 20 -> 9200, 255 -> None])
        for i in 0..256i32 {
            let val: i32 = match i {
                10 => 8500,
                20 => 9200,
                _ => 0,
            };
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.extend_from_slice(&[10u8, 20, u8::MAX]);

        // backoff section: 256*4B codebook + 3B u8 ([5 -> -1800, 15 -> -400, 255 -> None])
        for i in 0..256i32 {
            let val: i32 = match i {
                5 => -1800,
                15 => -400,
                _ => 0,
            };
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.extend_from_slice(&[5u8, 15, u8::MAX]);

        std::fs::write(&tmp_path, &buf).expect("写入测试 v3 Bigram 文件");
        let bg = CompactMmapBigram::open(&tmp_path).expect("加载 v3 Bigram 成功");
        assert_eq!(bg.version, 3);
        assert_eq!(bg.out_degree(0), Some(2));
        assert_eq!(bg.out_degree(1), Some(0));
        assert_eq!(bg.get_affinity_by_id(0, 1), Some(3000));
        assert_eq!(bg.get_unigram_score(0), Some(8500));
        assert_eq!(bg.get_unigram_score(2), None);
        assert_eq!(bg.get_backoff_weight(0), Some(-1800));
        assert_eq!(bg.get_backoff_weight(2), None);

        let _ = std::fs::remove_file(tmp_path);
    }
}
