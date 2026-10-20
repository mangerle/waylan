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

/// 标量量化码本的分数下界
const SCORE_MIN: i32 = 1000;

/// 标量量化码本的分数上界
const SCORE_MAX: i32 = 30000;

/// 标量量化码本的阶数
const CODEBOOK_STEPS: usize = 256;

/// CSR 模型文件写入参数包
struct CsrWriteParams<'a> {
    /// 目标模型文件路径
    output_path: &'a Path,
    /// 词表规模
    vocab_size: usize,
    /// 实际写入的词对数量
    num_pairs: usize,
    /// 标量量化码本
    codebook: &'a [i32; CODEBOOK_STEPS],
    /// CSR 行偏移表
    row_offsets: &'a [u32],
    /// 紧凑列数据
    cols_data: &'a [u32],
}

/// 构建 256 阶标量量化码本，将分数区间线性映射为均匀量化刻度
fn build_quantization_codebook() -> [i32; CODEBOOK_STEPS] {
    let mut codebook = [0i32; CODEBOOK_STEPS];
    for (i, item) in codebook.iter_mut().enumerate() {
        let ratio = i as f64 / (CODEBOOK_STEPS - 1) as f64;
        *item = (SCORE_MIN as f64 + (SCORE_MAX - SCORE_MIN) as f64 * ratio) as i32;
    }
    codebook
}

/// 统计每个首词 ID 的词对数量并推导 CSR 行偏移表
fn build_row_offsets(vocab_size: usize, selected: &[(u32, u32, i32, u32)]) -> Vec<u32> {
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
    row_offsets
}

/// 将词对打包为 24 位后继词 ID 与 8 位量化索引的紧凑列数据
fn build_cols_data(selected: &[(u32, u32, i32, u32)]) -> Vec<u32> {
    let mut cols_data = Vec::with_capacity(selected.len());
    for &(_, w2, aff, _) in selected {
        assert!(w2 <= 0x00FF_FFFF, "后继词 ID 超过 24 位上限");
        let clamped_aff = aff.clamp(SCORE_MIN, SCORE_MAX);
        let q_score = (((clamped_aff - SCORE_MIN) as f64 / (SCORE_MAX - SCORE_MIN) as f64) * 255.0)
            .round() as u8;
        cols_data.push((w2 & 0x00FF_FFFF) | ((q_score as u32) << 24));
    }
    cols_data
}

/// 将模型头部、量化码本、行偏移表与紧凑列数据顺序写入目标二进制文件
fn write_csr_model_file(params: CsrWriteParams<'_>) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(params.output_path)?);
    let codebook_offset = BIGRAM_HEADER_SIZE as u64;
    let row_offsets_offset = codebook_offset + CODEBOOK_SIZE as u64;
    let cols_offset = row_offsets_offset + ((params.vocab_size + 1) * 4) as u64;

    let mut header = [0u8; 64];
    header[0..16].copy_from_slice(BIGRAM_MAGIC);
    header[16..20].copy_from_slice(&2u32.to_le_bytes());
    header[20..24].copy_from_slice(&(params.vocab_size as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(params.num_pairs as u32).to_le_bytes());
    header[28..36].copy_from_slice(&codebook_offset.to_le_bytes());
    header[36..44].copy_from_slice(&row_offsets_offset.to_le_bytes());
    header[44..52].copy_from_slice(&cols_offset.to_le_bytes());
    writer.write_all(&header)?;
    for &val in params.codebook {
        writer.write_all(&val.to_le_bytes())?;
    }
    for &off in params.row_offsets {
        writer.write_all(&off.to_le_bytes())?;
    }
    for &col in params.cols_data {
        writer.write_all(&col.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

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

    let codebook = build_quantization_codebook();
    let row_offsets = build_row_offsets(vocab_size, selected);

    assert!(vocab_size <= 0x00FF_FFFF, "词表规模超过 24 位上限");
    let cols_data = build_cols_data(selected);

    write_csr_model_file(CsrWriteParams {
        output_path,
        vocab_size,
        num_pairs,
        codebook: &codebook,
        row_offsets: &row_offsets,
        cols_data: &cols_data,
    })?;

    let size_mb = fs::metadata(output_path)?.len() as f64 / (1024.0 * 1024.0);
    println!(
        "   模型生成成功: {} | 文件大小: {:.2} MB | 容纳词对: {} 对",
        output_path.display(),
        size_mb,
        num_pairs
    );
    Ok(())
}
