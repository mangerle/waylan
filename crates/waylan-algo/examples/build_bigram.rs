//! 基于多源清洗语料与词表对齐分词构建 v3 绝对折扣 Bigram + 同源 Unigram 矩阵
//!
//! # 设计原理
//! - **实现初衷**：利用 `data/base.dict.bin` 词表对清洗后的百科、新闻、知识与对话语料执行
//!   正向最大匹配（FMM）对齐分词，使用绝对折扣插值平滑（`d = 0.75`）统一计算
//!   同源 Unigram 对数概率、Bigram 条件概率增益及前驱词动态回退权重，导出 v3 二进制文件。
//! - **核心优势**：流式处理 GB 级语料时内置周期性低频噪声淘汰，内存占用可控且零外部依赖。

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use waylan_algo::bigram::{BIGRAM_HEADER_SIZE, BIGRAM_MAGIC, CODEBOOK_SIZE};
use waylan_algo::table::CompactMmapLexicon;

/// 绝对折扣因子
const DISCOUNT: f64 = 0.75;
/// 最大保留二元词对上限（控制总文件体积在 ~25MB 左右）
const MAX_PAIRS: usize = 3_500_000;
/// 最小二元共现频次阈值
const MIN_PAIR_COUNT: u32 = 3;
/// 哈希表触发低频淘汰的容量阈值，防止内存无限膨胀
const PRUNE_THRESHOLD: usize = 24_000_000;

/// 语料源配置：(文件路径, 采样重复权重)
const CORPUS_SOURCES: &[(&str, usize)] = &[
    (
        "/home/wsl/coding/waylan/data/corpus_clean/wiki_clean.txt",
        1,
    ),
    (
        "/home/wsl/coding/waylan/data/corpus_clean/thucnews_clean.txt",
        3,
    ),
    (
        "/home/wsl/coding/waylan/data/corpus_clean/knowledge_clean.txt",
        1,
    ),
    (
        "/home/wsl/coding/waylan/data/corpus_clean/spoken_lccc_clean.txt",
        1,
    ),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base_path = args
        .iter()
        .position(|a| a == "--base")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("data/base.dict.bin");
    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("data/bigram.table.v3.bin");

    let t0 = Instant::now();
    let lexicon = match CompactMmapLexicon::open(base_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("打开主词典 {base_path} 失败: {e}");
            return;
        }
    };
    let vocab_size = lexicon.vocab_size;
    println!("已加载主词典 {base_path}，词表容量: {vocab_size}");

    let mut word_to_id: HashMap<&str, u32> = HashMap::with_capacity(vocab_size);
    for wid in 0..vocab_size as u32 {
        if let Some(w) = lexicon.get_word(wid) {
            word_to_id.entry(w).or_insert(wid);
        }
    }
    println!(
        "已建立词表索引 ({} 条)，耗时 {:.2}s",
        word_to_id.len(),
        t0.elapsed().as_secs_f64()
    );

    let mut unigram_counts = vec![0u64; vocab_size];
    let mut pair_counts: HashMap<u64, u32> = HashMap::with_capacity(4_000_000);
    let mut total_tokens: u64 = 0;

    for &(path, weight) in CORPUS_SOURCES {
        if !Path::new(path).exists() {
            println!("[跳过] 未找到语料文件: {path}");
            continue;
        }
        let ts = Instant::now();
        match count_corpus_file(
            path,
            weight,
            &word_to_id,
            &mut unigram_counts,
            &mut pair_counts,
            &mut total_tokens,
        ) {
            Ok(lines) => println!(
                "[完成] {} (x{}) | 行数: {} | 累计词次: {} | 活跃二元对: {} | 耗时: {:.2}s",
                path,
                weight,
                lines,
                total_tokens,
                pair_counts.len(),
                ts.elapsed().as_secs_f64()
            ),
            Err(e) => eprintln!("[失败] 处理 {path} 出错: {e}"),
        }
    }

    if total_tokens == 0 || pair_counts.is_empty() {
        eprintln!("有效统计语料为空，终止构建");
        return;
    }

    if let Err(e) = export_v3_bigram(
        out_path,
        vocab_size,
        total_tokens,
        &unigram_counts,
        pair_counts,
    ) {
        eprintln!("导出 v3 Bigram 失败: {e}");
        return;
    }
    println!(
        "v3 Bigram + 同源 Unigram 构建完成: {}，总耗时 {:.2}s",
        out_path,
        t0.elapsed().as_secs_f64()
    );
}

/// 流式读取清洗后纯文本并执行词表对齐正向最大匹配分词统计
fn count_corpus_file(
    path: &str,
    weight: usize,
    word_to_id: &HashMap<&str, u32>,
    unigram_counts: &mut [u64],
    pair_counts: &mut HashMap<u64, u32>,
    total_tokens: &mut u64,
) -> Result<usize, String> {
    let file = File::open(path).map_err(|e| format!("打开 {path} 失败: {e}"))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut line = String::with_capacity(256);
    let mut tokens: Vec<u32> = Vec::with_capacity(32);
    let mut lines = 0usize;
    let w_u64 = weight as u64;
    let w_u32 = weight as u32;

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        segment_fmm(trimmed, word_to_id, &mut tokens);
        if tokens.is_empty() {
            continue;
        }

        for &wid in &tokens {
            unigram_counts[wid as usize] = unigram_counts[wid as usize].saturating_add(w_u64);
            *total_tokens = total_tokens.saturating_add(w_u64);
        }
        for win in tokens.windows(2) {
            let key = ((win[0] as u64) << 32) | (win[1] as u64);
            let entry = pair_counts.entry(key).or_insert(0);
            *entry = entry.saturating_add(w_u32);
        }

        if pair_counts.len() >= PRUNE_THRESHOLD {
            let mut min_keep = 2u32;
            while pair_counts.len() >= (PRUNE_THRESHOLD * 7 / 10) && min_keep <= 4 {
                pair_counts.retain(|_, cnt| *cnt >= min_keep);
                min_keep += 1;
            }
        }
    }

    Ok(lines)
}

/// 基于 `base.dict.bin` 词表执行正向最大匹配分词（最大词长 8 个汉字）
fn segment_fmm(text: &str, word_to_id: &HashMap<&str, u32>, out: &mut Vec<u32>) {
    out.clear();
    let mut pos = 0usize;
    let len = text.len();
    while pos < len {
        let rem = &text[pos..];
        let mut char_ends = [0usize; 8];
        let mut char_cnt = 0usize;
        for (idx, ch) in rem.char_indices() {
            if char_cnt < 8 {
                char_ends[char_cnt] = idx + ch.len_utf8();
                char_cnt += 1;
            } else {
                break;
            }
        }
        if char_cnt == 0 {
            break;
        }

        let mut matched = false;
        for l in (1..=char_cnt).rev() {
            let end_byte = char_ends[l - 1];
            if let Some(&wid) = word_to_id.get(&rem[..end_byte]) {
                out.push(wid);
                pos += end_byte;
                matched = true;
                break;
            }
        }
        if !matched {
            pos += char_ends[0];
        }
    }
}

/// 过滤 Top-N 二元词对、计算绝对折扣条件概率与动态回退权重，并导出 v3 二进制文件
fn export_v3_bigram(
    out_path: &str,
    vocab_size: usize,
    total_tokens: u64,
    unigram_counts: &[u64],
    pair_counts: HashMap<u64, u32>,
) -> Result<(), String> {
    let mut pairs: Vec<(u32, u32, u32)> = pair_counts
        .into_iter()
        .filter(|&(_, cnt)| cnt >= MIN_PAIR_COUNT)
        .map(|(k, cnt)| ((k >> 32) as u32, k as u32, cnt))
        .collect();

    if pairs.len() > MAX_PAIRS {
        pairs.select_nth_unstable_by(MAX_PAIRS, |a, b| b.2.cmp(&a.2));
        pairs.truncate(MAX_PAIRS);
    }
    pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let total_pairs = pairs.len();
    println!("过滤排序后保留有效二元对: {total_pairs}");

    // 1. 计算同源 Unigram 对数分及前驱出度
    let denom = (total_tokens as f64) + 0.5 * (vocab_size as f64);
    let mut unigram_probs = vec![0.5 / denom; vocab_size];
    let mut unigram_scores = vec![i32::MIN; vocab_size];
    for (wid, &cnt) in unigram_counts.iter().enumerate() {
        if cnt > 0 {
            let p = ((cnt as f64) + 0.5) / denom;
            unigram_probs[wid] = p;
            let score = ((p.ln() * 1_200.0) + 18_000.0).round() as i64;
            unigram_scores[wid] = score.clamp(100, 18_000) as i32;
        }
    }

    let mut row_offsets = vec![0u32; vocab_size + 1];
    for &(prev_id, _, _) in &pairs {
        row_offsets[prev_id as usize + 1] += 1;
    }
    for i in 1..=vocab_size {
        row_offsets[i] += row_offsets[i - 1];
    }

    // 2. 计算每个前驱词的绝对折扣回退权重 lambda(prev) = d * N_1+(prev) / (C(prev) + 60.0)
    let mut backoff_weights = vec![i32::MIN; vocab_size];
    let mut lambda_by_prev = vec![1.0f64; vocab_size];
    for pid in 0..vocab_size {
        let out_deg = row_offsets[pid + 1] - row_offsets[pid];
        let c_prev = unigram_counts[pid];
        if c_prev > 0 && out_deg > 0 {
            let smoothed_c = (c_prev as f64) + 60.0;
            let lambda = (DISCOUNT * (out_deg as f64) / smoothed_c).clamp(1e-4, 0.999);
            lambda_by_prev[pid] = lambda;
            backoff_weights[pid] = (lambda.ln() * 250.0).round().clamp(-2_000.0, 0.0) as i32;
        }
    }

    // 3. 计算每条二元词对的绝对折扣插值对数条件概率 log P_abs(cur | prev)，并量化到 256 阶码本
    //    分母加入先验平滑 (+60.0)，防止低频前驱词因总频次过小导致条件概率虚高
    let mut raw_scores = Vec::with_capacity(total_pairs);
    let mut min_score = f64::INFINITY;
    let mut max_score = f64::NEG_INFINITY;
    for &(prev_id, curr_id, cnt) in &pairs {
        let pid = prev_id as usize;
        let cid = curr_id as usize;
        let smoothed_c_prev = (unigram_counts[pid] as f64).max(cnt as f64) + 60.0;
        let p_abs = ((cnt as f64 - DISCOUNT).max(0.0) / smoothed_c_prev)
            + lambda_by_prev[pid] * unigram_probs[cid];
        let cond_score = (p_abs.max(1e-7).ln() * 2_000.0 + 22_000.0).clamp(0.0, 22_000.0);
        min_score = min_score.min(cond_score);
        max_score = max_score.max(cond_score);
        raw_scores.push(cond_score);
    }
    println!("条件概率增益范围: [{min_score:.1}, {max_score:.1}]");

    let span = (max_score - min_score).max(1.0);
    let mut codebook = [0i32; 256];
    for (q, slot) in codebook.iter_mut().enumerate() {
        *slot = (min_score + (q as f64 / 255.0) * span).round() as i32;
    }

    // 4. 写出 v3 二进制文件
    let file = File::create(out_path).map_err(|e| format!("创建 {out_path} 失败: {e}"))?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);

    let codebook_offset = BIGRAM_HEADER_SIZE as u64;
    let row_offsets_offset = codebook_offset + CODEBOOK_SIZE as u64;
    let cols_offset = row_offsets_offset + ((vocab_size as u64 + 1) * 4);
    let unigram_offset = cols_offset + (total_pairs as u64 * 4);

    let mut header = [0u8; BIGRAM_HEADER_SIZE];
    header[0..16].copy_from_slice(BIGRAM_MAGIC);
    header[16..20].copy_from_slice(&3u32.to_le_bytes());
    header[20..24].copy_from_slice(&(vocab_size as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(total_pairs as u32).to_le_bytes());
    header[28..36].copy_from_slice(&codebook_offset.to_le_bytes());
    header[36..44].copy_from_slice(&row_offsets_offset.to_le_bytes());
    header[44..52].copy_from_slice(&cols_offset.to_le_bytes());
    header[52..60].copy_from_slice(&unigram_offset.to_le_bytes());

    writer
        .write_all(&header)
        .map_err(|e| format!("写头失败: {e}"))?;
    for &c in &codebook {
        writer
            .write_all(&c.to_le_bytes())
            .map_err(|e| format!("写码本失败: {e}"))?;
    }
    for &ro in &row_offsets {
        writer
            .write_all(&ro.to_le_bytes())
            .map_err(|e| format!("写行偏移失败: {e}"))?;
    }
    for (i, &(_, curr_id, _)) in pairs.iter().enumerate() {
        let q = (((raw_scores[i] - min_score) / span) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
        let packed = (curr_id & 0x00FF_FFFF) | ((q as u32) << 24);
        writer
            .write_all(&packed.to_le_bytes())
            .map_err(|e| format!("写列数据失败: {e}"))?;
    }
    write_quantized_u8_section(&mut writer, &unigram_scores)?;
    write_quantized_u8_section(&mut writer, &backoff_weights)?;
    writer.flush().map_err(|e| format!("刷新文件失败: {e}"))?;
    Ok(())
}

/// 将词级 `i32` 数组（`i32::MIN` 为未登录哨兵）量化为 `1024B 码本 + u8 索引数组（255 为哨兵）` 写入
fn write_quantized_u8_section(writer: &mut BufWriter<File>, values: &[i32]) -> Result<(), String> {
    let mut min_v = i32::MAX;
    let mut max_v = i32::MIN;
    for &v in values {
        if v != i32::MIN {
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
    }
    if min_v > max_v {
        min_v = 0;
        max_v = 0;
    }

    let span = ((max_v as f64) - (min_v as f64)).max(1.0);
    let mut codebook = [0i32; 256];
    for (q, slot) in codebook.iter_mut().enumerate() {
        if q < 255 {
            *slot = ((min_v as f64) + (q as f64 / 254.0) * span).round() as i32;
        }
    }
    for &c in &codebook {
        writer
            .write_all(&c.to_le_bytes())
            .map_err(|e| format!("写量化码本失败: {e}"))?;
    }
    for &v in values {
        let q = if v == i32::MIN {
            u8::MAX
        } else {
            ((((v as f64) - (min_v as f64)) / span) * 254.0)
                .round()
                .clamp(0.0, 254.0) as u8
        };
        writer
            .write_all(&[q])
            .map_err(|e| format!("写量化索引失败: {e}"))?;
    }
    Ok(())
}
