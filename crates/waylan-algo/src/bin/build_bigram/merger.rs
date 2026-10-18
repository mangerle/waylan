//! 临时分片落盘与 K 路最小堆多路归并器
//!
//! # 设计原理
//! - **实现初衷**：在单机内存受限场景下，采用经典外部排序（External Merge Sort）算法，
//!   将数十亿级二元词对分批排序落地为有序二进制临时分片，并基于最小堆进行流式归并。
//! - **核心优势**：严格限制内存峰值；多路归并流式计算一元/二元转移概率。

use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// 临时分片单条二元项结构（16 字节对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawBigramPair {
    pub w1_id: u32,
    pub w2_id: u32,
    pub count: u32,
}

impl PartialOrd for RawBigramPair {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RawBigramPair {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.w1_id, self.w2_id).cmp(&(other.w1_id, other.w2_id))
    }
}

/// K 路归并堆节点（最小堆逆序比较）
#[derive(PartialEq, Eq)]
pub struct MergeNode {
    pub pair: RawBigramPair,
    pub chunk_idx: usize,
}

impl PartialOrd for MergeNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.pair.cmp(&self.pair)
    }
}

/// 将内存中的局部二元词频表剪枝、并行排序并刷盘为有序临时分片文件
pub fn flush_chunk_to_disk(
    temp_chunk_dir: &Path,
    chunk_index: usize,
    current_chunk_map: &mut HashMap<(u32, u32), u32>,
) -> io::Result<PathBuf> {
    let chunk_path = temp_chunk_dir.join(format!("chunk_{:04}.bin", chunk_index));
    let mut sorted_chunk: Vec<RawBigramPair> = current_chunk_map
        .drain()
        .filter(|&(_, cnt)| cnt >= 2)
        .map(|((w1_id, w2_id), count)| RawBigramPair {
            w1_id,
            w2_id,
            count,
        })
        .collect();
    sorted_chunk.par_sort_unstable();

    let mut w = BufWriter::new(File::create(&chunk_path)?);
    for item in sorted_chunk {
        w.write_all(&item.w1_id.to_le_bytes())?;
        w.write_all(&item.w2_id.to_le_bytes())?;
        w.write_all(&item.count.to_le_bytes())?;
    }
    w.flush()?;
    Ok(chunk_path)
}

/// 从分片流中读取下一个二元项
pub fn read_pair(r: &mut BufReader<File>) -> Option<RawBigramPair> {
    let mut buf = [0u8; 12];
    r.read_exact(&mut buf).ok()?;
    Some(RawBigramPair {
        w1_id: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
        w2_id: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        count: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
    })
}

/// 结算归并后的单个二元词对亲和度得分并推入结果集
pub fn settle_pair_affinity(
    cur: RawBigramPair,
    global_unigram: &HashMap<u32, u32>,
    aggregated_pairs: &mut Vec<(u32, u32, i32, u32)>,
) {
    if cur.count >= 3 {
        let u_count = global_unigram.get(&cur.w1_id).copied().unwrap_or(cur.count);
        let prob = (cur.count as f64 / u_count as f64).max(1e-7);
        let aff = (20000.0 + prob.ln() * 2000.0).clamp(1000.0, 30000.0) as i32;
        aggregated_pairs.push((cur.w1_id, cur.w2_id, aff, cur.count));
    }
}
