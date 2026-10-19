//! 紧凑 CSR 二进制语言模型导出器
//!
//! # 设计原理
//! - **实现初衷**：将归并后的二元词对序列化为压缩稀疏行 (CSR) 格式，并结合 256 阶标量量化码本，
//!   实现无损/低损的超高压缩比与内存零拷贝映射。
//! - **核心优势**：单词对压缩至 4 字节（24-bit 目标词 ID + 8-bit 量化索引）。

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use waylan_algo::bigram::{BIGRAM_HEADER_SIZE, BIGRAM_MAGIC, CODEBOOK_SIZE};

/// 写入 CSR 紧凑二进制格式模型文件
pub fn export_csr_model(
    output_path: &Path,
    vocab_size: usize,
    target_capacity: usize,
    pairs: &[(u32, u32, i32, u32)],
) -> io::Result<()> {
    let num_pairs = pairs.len().min(target_capacity);
    let selected = &pairs[..num_pairs];
    let file_label = output_path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    println!(
        "-> 正在为规格 [{}] 生成 256 阶标量量化码本与 CSR 拓扑...",
        file_label
    );

    let mut codebook = [0i32; 256];
    let (min_score, max_score) = (1000i32, 30000i32);
    for (i, item) in codebook.iter_mut().enumerate() {
        let ratio = i as f64 / 255.0;
        *item = (min_score as f64 + (max_score - min_score) as f64 * ratio) as i32;
    }

    let mut row_counts = vec![0u32; vocab_size];
    for &(w1, _, _, _) in selected {
        if (w1 as usize) < vocab_size {
            row_counts[w1 as usize] = row_counts[w1 as usize].saturating_add(1);
        }
    }
    let mut row_offsets = vec![0u32; vocab_size + 1];
    let mut current_offset = 0u32;
    for i in 0..vocab_size {
        row_offsets[i] = current_offset;
        current_offset = current_offset.saturating_add(row_counts[i]);
    }
    row_offsets[vocab_size] = current_offset;

    assert!(vocab_size <= 0x00FF_FFFF, "词表规模超过 24 位上限");
    let mut cols_data = Vec::with_capacity(num_pairs);
    for &(_, w2, aff, _) in selected {
        assert!(w2 <= 0x00FF_FFFF, "后继词 ID 超过 24 位上限");
        let clamped_aff = aff.clamp(min_score, max_score);
        let q_score = (((clamped_aff - min_score) as f64 / (max_score - min_score) as f64) * 255.0)
            .round() as u8;
        cols_data.push((w2 & 0x00FF_FFFF) | ((q_score as u32) << 24));
    }

    let mut writer = BufWriter::new(File::create(output_path)?);
    let codebook_offset = BIGRAM_HEADER_SIZE as u64;
    let row_offsets_offset = codebook_offset + CODEBOOK_SIZE as u64;
    let cols_offset = row_offsets_offset + ((vocab_size + 1) * 4) as u64;

    let mut header = [0u8; 64];
    header[0..16].copy_from_slice(BIGRAM_MAGIC);
    header[16..20].copy_from_slice(&2u32.to_le_bytes());
    header[20..24].copy_from_slice(&(vocab_size as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(num_pairs as u32).to_le_bytes());
    header[28..36].copy_from_slice(&codebook_offset.to_le_bytes());
    header[36..44].copy_from_slice(&row_offsets_offset.to_le_bytes());
    header[44..52].copy_from_slice(&cols_offset.to_le_bytes());
    writer.write_all(&header)?;
    for &val in &codebook {
        writer.write_all(&val.to_le_bytes())?;
    }
    for &off in &row_offsets {
        writer.write_all(&off.to_le_bytes())?;
    }
    for &col in &cols_data {
        writer.write_all(&col.to_le_bytes())?;
    }
    writer.flush()?;
    let size_mb = fs::metadata(output_path)?.len() as f64 / (1024.0 * 1024.0);
    println!(
        "   模型生成成功: {} | 文件大小: {:.2} MB | 容纳词对: {} 对",
        output_path.display(),
        size_mb,
        num_pairs
    );
    Ok(())
}
