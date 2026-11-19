//! 紧凑二进制词典文件头解析与对齐校验模块

use crate::error::AlgoError;
use memmap2::Mmap;
use std::path::Path;

/// 紧凑词典魔数标识（16 字节）
pub const COMPACT_DICT_MAGIC: &[u8; 16] = b"WAYLAN_COMPACT02";

/// 紧凑词典文件头解析结果（记录各数据段在内存映射中的绝对偏移量）
pub(crate) struct CompactDictHeader {
    pub(crate) num_keys: usize,
    pub(crate) num_candidates: usize,
    pub(crate) vocab_size: usize,
    pub(crate) dat_node_count: usize,
    pub(crate) dat_tail_len: usize,
    pub(crate) dat_base_offset: usize,
    pub(crate) dat_check_offset: usize,
    pub(crate) dat_tail_offset: usize,
    pub(crate) key_metas_offset: usize,
    pub(crate) candidates_offset: usize,
    pub(crate) vocab_offsets_offset: usize,
    pub(crate) sorted_vocab_ids_offset: usize,
    pub(crate) vocab_strings_offset: usize,
}

impl CompactDictHeader {
    /// 解析并校验紧凑词典文件头（魔数、版本、数据区边界与内存对齐契约）
    pub(crate) fn parse(mmap: &Mmap, path: &Path) -> Result<Self, AlgoError> {
        Self::validate_magic_and_version(mmap, path)?;

        let (num_keys, num_candidates, vocab_size, dat_node_count, dat_tail_len) =
            read_header_counts(mmap);
        let (key_metas_offset, candidates_offset, vocab_offset) = read_header_offsets(mmap);

        let (dat_base_offset, dat_check_offset, dat_tail_offset) =
            Self::derive_dat_offsets(dat_node_count);
        let (vocab_offsets_offset, sorted_vocab_ids_offset, vocab_strings_offset) =
            Self::derive_vocab_offsets(vocab_size, vocab_offset);

        let dat_tail_end = dat_tail_offset.saturating_add(dat_tail_len);
        let key_metas_end = key_metas_offset.saturating_add(num_keys.saturating_mul(8));
        let candidates_end = candidates_offset.saturating_add(num_candidates.saturating_mul(8));
        let min_required_len = dat_tail_end
            .max(key_metas_end)
            .max(candidates_end)
            .max(vocab_strings_offset);
        Self::validate_sections(mmap, path, min_required_len)?;

        // 校验内存映射基地址及内部 32 位整数切片偏移量是否符合 4 字节对齐契约（杜绝未对齐引发 UB）
        Self::validate_alignment(
            mmap,
            path,
            dat_base_offset,
            &[
                dat_check_offset,
                sorted_vocab_ids_offset,
                vocab_offsets_offset,
            ],
        )?;

        Ok(Self {
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

    /// 校验紧凑词典文件的魔数标识与版本号
    fn validate_magic_and_version(mmap: &Mmap, path: &Path) -> Result<(), AlgoError> {
        if mmap.len() < 64 || &mmap[0..16] != COMPACT_DICT_MAGIC {
            return Err(AlgoError::deserialize_failed(
                path,
                "无效的紧凑词典魔数标识 (需 WAYLAN_COMPACT02)",
            ));
        }
        let version = read_u32_le(mmap, 16) as u32;
        if version != 3 {
            return Err(AlgoError::deserialize_failed(
                path,
                format!("不支持的词库版本号: {} (需版本 3)", version),
            ));
        }
        Ok(())
    }

    /// 推导 DAT 段（base / check / tail）在内存映射中的绝对偏移量
    fn derive_dat_offsets(dat_node_count: usize) -> (usize, usize, usize) {
        let dat_base_offset = 64usize;
        let dat_check_offset = dat_base_offset.saturating_add(dat_node_count.saturating_mul(4));
        let dat_tail_offset = dat_check_offset.saturating_add(dat_node_count.saturating_mul(4));
        (dat_base_offset, dat_check_offset, dat_tail_offset)
    }

    /// 推导词表段（词条偏移表 / 排序词 ID 表 / 字符串池）的绝对偏移量
    fn derive_vocab_offsets(vocab_size: usize, vocab_offset: usize) -> (usize, usize, usize) {
        let vocab_offsets_offset = vocab_offset;
        let sorted_vocab_ids_offset =
            vocab_offsets_offset.saturating_add(vocab_size.saturating_add(1).saturating_mul(4));
        let vocab_strings_offset =
            sorted_vocab_ids_offset.saturating_add(vocab_size.saturating_mul(4));
        (
            vocab_offsets_offset,
            sorted_vocab_ids_offset,
            vocab_strings_offset,
        )
    }

    /// 校验各数据段的结束偏移是否均落在内存映射范围内（防止越界读取）
    fn validate_sections(
        mmap: &Mmap,
        path: &Path,
        min_required_len: usize,
    ) -> Result<(), AlgoError> {
        if mmap.len() < min_required_len {
            return Err(AlgoError::deserialize_failed(path, "紧凑词典数据区截断"));
        }
        Ok(())
    }

    /// 校验内存映射基地址与内部数据段偏移量是否符合 4 字节对齐契约（杜绝未对齐引发 UB）
    fn validate_alignment(
        mmap: &Mmap,
        path: &Path,
        dat_base_offset: usize,
        u32_aligned_offsets: &[usize],
    ) -> Result<(), AlgoError> {
        let base_ptr = mmap.as_ptr() as usize;
        if !base_ptr.is_multiple_of(std::mem::align_of::<u32>()) {
            return Err(AlgoError::deserialize_failed(
                path,
                "内存映射基地址未按 4 字节对齐",
            ));
        }
        if !dat_base_offset.is_multiple_of(std::mem::align_of::<i32>())
            || u32_aligned_offsets
                .iter()
                .any(|off| !off.is_multiple_of(std::mem::align_of::<u32>()))
        {
            return Err(AlgoError::deserialize_failed(
                path,
                "紧凑词典内部数据段偏移量未按 4 字节对齐",
            ));
        }
        Ok(())
    }
}

/// 从内存映射中读取小端 32 位无符号整型（解析失败时回退为 0）
#[inline]
fn read_u32_le(mmap: &Mmap, offset: usize) -> usize {
    u32::from_le_bytes(mmap[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize
}

/// 从内存映射中读取小端 64 位无符号整型（解析失败时回退为 0）
#[inline]
fn read_u64_le(mmap: &Mmap, offset: usize) -> usize {
    u64::from_le_bytes(mmap[offset..offset + 8].try_into().unwrap_or([0; 8])) as usize
}

/// 读取紧凑词典文件头中声明的各段规模字段
#[inline]
fn read_header_counts(mmap: &Mmap) -> (usize, usize, usize, usize, usize) {
    (
        read_u32_le(mmap, 20),
        read_u32_le(mmap, 24),
        read_u32_le(mmap, 28),
        read_u32_le(mmap, 32),
        read_u32_le(mmap, 36),
    )
}

/// 读取紧凑词典文件头中声明的各数据段起始偏移量
#[inline]
fn read_header_offsets(mmap: &Mmap) -> (usize, usize, usize) {
    (
        read_u64_le(mmap, 40),
        read_u64_le(mmap, 48),
        read_u64_le(mmap, 56),
    )
}
