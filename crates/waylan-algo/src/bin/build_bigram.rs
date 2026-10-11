use jieba_rs::Jieba;
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use waylan_algo::bigram::{BIGRAM_HEADER_SIZE, BIGRAM_MAGIC, CODEBOOK_SIZE};
use waylan_algo::vocab::Vocabulary;

/// 临时分片单条二元项结构（16 字节对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawBigramPair {
    w1_id: u32,
    w2_id: u32,
    count: u32,
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

/// K路归并堆节点（最小堆逆序比较）
#[derive(PartialEq, Eq)]
struct MergeNode {
    pair: RawBigramPair,
    chunk_idx: usize,
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

/// 构建预处理与分词清洗器
struct Cleaner {
    re_code: Regex,
    re_split: Regex,
    re_chinese: Regex,
}

impl Cleaner {
    fn new() -> Self {
        Self {
            re_code: Regex::new(r"```[\s\S]*?```").unwrap(),
            re_split: Regex::new(r"[，。！？；、\n\r\t]+").unwrap(),
            re_chinese: Regex::new(r"[\u4e00-\u9fff]").unwrap(),
        }
    }

    /// 清洗长文本并切分为高置信度独立中文句子
    fn clean_and_split(&self, text: &str) -> Vec<String> {
        let text_no_code = self.re_code.replace_all(text, "");
        let raw_sents: Vec<&str> = self.re_split.split(&text_no_code).collect();
        let mut valid_sents = Vec::new();

        for s in raw_sents {
            let trimmed = s.trim();
            let char_count = trimmed.chars().count();
            if (2..=80).contains(&char_count) {
                let zh_count = self.re_chinese.find_iter(trimmed).count();
                if zh_count as f64 / char_count as f64 >= 0.6 {
                    valid_sents.push(trimmed.to_string());
                }
            }
        }

        valid_sents
    }
}

/// 从单段文本中切分提取相邻二元词对并累加至输出容器
fn collect_text_pairs(
    text: &str,
    weight: u32,
    cleaner: &Cleaner,
    jieba: &Jieba,
    out: &mut Vec<(String, String, u32)>,
) {
    for sent in cleaner.clean_and_split(text) {
        let words = jieba.cut(&sent, false);
        for pair in words.windows(2) {
            out.push((pair[0].to_string(), pair[1].to_string(), weight));
        }
    }
}

/// 解析单行 JSONL 语料并提取相邻二元词对
fn extract_line_pairs(
    raw_line: &str,
    is_sft: bool,
    cleaner: &Cleaner,
    jieba: &Jieba,
) -> Vec<(String, String, u32)> {
    let mut local_res = Vec::new();
    let Ok(val) = serde_json::from_str::<Value>(raw_line) else {
        return local_res;
    };

    if is_sft {
        if let Some(convs) = val.get("conversations").and_then(|v| v.as_array()) {
            for turn in convs {
                let content = turn.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let role = turn.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let weight: u32 = if role == "user" { 2 } else { 1 };
                collect_text_pairs(content, weight, cleaner, jieba, &mut local_res);
            }
        }
    } else if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        collect_text_pairs(text, 1, cleaner, jieba, &mut local_res);
    }

    local_res
}

/// 将内存中的局部二元词频表剪枝、排序并刷盘为有序临时分片文件
fn flush_chunk_to_disk(
    temp_chunk_dir: &Path,
    chunk_index: usize,
    current_chunk_map: &mut HashMap<(u32, u32), u32>,
) -> PathBuf {
    let chunk_path = temp_chunk_dir.join(format!("chunk_{:04}.bin", chunk_index));
    let mut sorted_chunk: Vec<RawBigramPair> = current_chunk_map
        .drain()
        .filter(|&(_, cnt)| cnt >= 2)
        .map(|((id1, id2), cnt)| RawBigramPair {
            w1_id: id1,
            w2_id: id2,
            count: cnt,
        })
        .collect();
    sorted_chunk.par_sort_unstable();

    let f = File::create(&chunk_path).expect("创建分片文件失败");
    let mut w = BufWriter::new(f);
    for item in sorted_chunk {
        w.write_all(&item.w1_id.to_le_bytes()).unwrap();
        w.write_all(&item.w2_id.to_le_bytes()).unwrap();
        w.write_all(&item.count.to_le_bytes()).unwrap();
    }
    w.flush().unwrap();
    chunk_path
}

/// 结算归并后的单个二元词对亲和度得分并推入结果集
fn settle_pair_affinity(
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

/// 写入 CSR 紧凑二进制格式模型文件
fn export_csr_model(
    output_path: &Path,
    vocab_size: usize,
    target_capacity: usize,
    pairs: &[(u32, u32, i32, u32)], // (w1, w2, affinity, count)
) -> io::Result<()> {
    let num_pairs = pairs.len().min(target_capacity);
    let selected = &pairs[..num_pairs];

    println!(
        "-> 正在为规格 [{}] 生成 256 阶标量量化码本与 CSR 拓扑...",
        output_path.file_name().unwrap().to_string_lossy()
    );

    // 1. 生成 256 阶码本
    let mut codebook = [0i32; 256];
    let (min_score, max_score) = (1000i32, 30000i32);
    for (i, item) in codebook.iter_mut().enumerate() {
        let ratio = i as f64 / 255.0;
        *item = (min_score as f64 + (max_score - min_score) as f64 * ratio) as i32;
    }

    // 2. 统计行偏移量 Row Offsets: [u32; vocab_size + 1]
    let mut row_counts = vec![0u32; vocab_size];
    for &(w1, _, _, _) in selected {
        if (w1 as usize) < vocab_size {
            row_counts[w1 as usize] += 1;
        }
    }
    let mut row_offsets = vec![0u32; vocab_size + 1];
    let mut current_offset = 0u32;
    for i in 0..vocab_size {
        row_offsets[i] = current_offset;
        current_offset += row_counts[i];
    }
    row_offsets[vocab_size] = current_offset;

    // 3. 构建列数据数组 Cols: [u32; num_pairs] (低24位next_id, 高8位q_score)
    let mut cols_data = Vec::with_capacity(num_pairs);
    for &(_, w2, aff, _) in selected {
        let clamped_aff = aff.clamp(min_score, max_score);
        let q_score = (((clamped_aff - min_score) as f64 / (max_score - min_score) as f64) * 255.0)
            .round() as u8;
        cols_data.push((w2 & 0x00FF_FFFF) | ((q_score as u32) << 24));
    }

    // 4. 构造标准二进制文件
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

fn main() {
    println!("=================================================================");
    println!("  微澜输入法 (WayLan) 全量语料 Bigram CSR 极致压缩构建管线");
    println!("  架构规范: 微信键盘同级 CSR 前缀聚类 + 8位标量量化 + 流式受控");
    println!("=================================================================");

    let start_total = Instant::now();

    // 1. 初始化路径配置
    let pretrain_path = Path::new("/mnt/d/data/modelscope/minimind_dataset/pretrain_t2t.jsonl");
    let sft_path = Path::new("/mnt/d/data/modelscope/minimind_dataset/sft_t2t.jsonl");
    let dict_dir = Path::new("/home/wsl/waylan/data/dicts");
    let vocab_bin_path = Path::new("/home/wsl/waylan/data/vocab.bin");
    let out_16mb_path = Path::new("/home/wsl/waylan/data/bigram_csr_16mb.bin");
    let out_32mb_path = Path::new("/home/wsl/waylan/data/bigram_csr_32mb.bin");
    let temp_chunk_dir = Path::new("/home/wsl/waylan/data/temp_chunks");

    let _ = fs::remove_dir_all(temp_chunk_dir);
    fs::create_dir_all(temp_chunk_dir).expect("创建分片临时目录成功");

    // 2. 载入或构建基础词表
    println!("\n[1/5] 构建基础统一词表 (Vocabulary)...");
    let yaml_files = vec![
        dict_dir.join("1_chars/rime_mint.chars.dict.yaml"),
        dict_dir.join("2_base/rime_mint.base.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.ext.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.places.dict.yaml"),
    ];

    let mut vocab = Vocabulary::load_from_dict_yamls(&yaml_files).expect("解析词典YAML失败");
    println!("   从词典YAML载入基础核心词条: {} 条", vocab.len());

    println!("   正在初始化 Jieba 分词器并注入微澜词条...");
    let jieba = Arc::new(Jieba::new());
    vocab.save_binary(vocab_bin_path).expect("持久化词表成功");
    println!("   词表已固化至: {}", vocab_bin_path.display());

    // 3. 多线程流式逐批扫描全量语料
    let cleaner = Arc::new(Cleaner::new());
    let mut chunk_files = Vec::new();
    let mut chunk_index = 0;

    let mut global_unigram: HashMap<u32, u32> = HashMap::with_capacity(vocab.len());
    let mut current_chunk_map: HashMap<(u32, u32), u32> = HashMap::with_capacity(3_000_000);

    let batch_lines_limit = 50_000;
    let chunk_flush_pairs_limit = 4_000_000;

    let datasets = vec![
        (
            pretrain_path,
            false,
            "预训练语料 (pretrain_t2t.jsonl 7.7GB)",
        ),
        (sft_path, true, "指令对话语料 (sft_t2t.jsonl 13.1GB)"),
    ];

    println!("\n[2/5] 启动流式多线程分词与二元统计 (内存上限严格控制在 2GB 以内)...");

    for (corpus_path, is_sft, label) in datasets {
        if !corpus_path.exists() {
            println!("警告: 未找到语料文件 {}, 跳过此部分", corpus_path.display());
            continue;
        }

        println!("-> 正在流式处理: {} ...", label);
        let file = File::open(corpus_path).expect("打开语料文件失败");
        let reader = BufReader::with_capacity(16 * 1024 * 1024, file);

        let mut batch_buffer = Vec::with_capacity(batch_lines_limit);
        let mut processed_lines: usize = 0;

        for line_res in reader.lines() {
            let Ok(line) = line_res else {
                continue;
            };
            batch_buffer.push(line);

            if batch_buffer.len() >= batch_lines_limit {
                processed_lines += batch_buffer.len();
                let lines_to_process =
                    mem::replace(&mut batch_buffer, Vec::with_capacity(batch_lines_limit));

                let cleaner_ref = Arc::clone(&cleaner);
                let jieba_ref = Arc::clone(&jieba);

                let local_pairs_list: Vec<Vec<(String, String, u32)>> = lines_to_process
                    .par_iter()
                    .map(|raw_line| extract_line_pairs(raw_line, is_sft, &cleaner_ref, &jieba_ref))
                    .collect();

                for local_pairs in local_pairs_list {
                    for (w1, w2, weight) in local_pairs {
                        let id1 = vocab.insert_or_get(&w1);
                        let id2 = vocab.insert_or_get(&w2);
                        *global_unigram.entry(id1).or_insert(0) += weight;
                        *current_chunk_map.entry((id1, id2)).or_insert(0) += weight;
                    }
                }

                print!(
                    "\r   已处理文本: {:>10} 行 | 局部缓存词对: {:>10} 对 | 词表扩充容量: {:>7} 词",
                    processed_lines,
                    current_chunk_map.len(),
                    vocab.len()
                );
                io::stdout().flush().unwrap();

                if current_chunk_map.len() >= chunk_flush_pairs_limit {
                    println!(
                        "\n   -> [内存控制] 触发局部剪枝与分片刷盘 (分片 #{}) ...",
                        chunk_index
                    );
                    let chunk_path =
                        flush_chunk_to_disk(temp_chunk_dir, chunk_index, &mut current_chunk_map);
                    chunk_index += 1;
                    chunk_files.push(chunk_path);
                }
            }
        }
        println!("\n   -> 当前语料完成，累计行数: {}", processed_lines);
    }

    if !current_chunk_map.is_empty() {
        let chunk_path = flush_chunk_to_disk(temp_chunk_dir, chunk_index, &mut current_chunk_map);
        chunk_files.push(chunk_path);
    }

    println!(
        "\n[3/5] 分片扫描完毕！共生成 {} 个有序局部临时分片，进入多路归并汇聚...",
        chunk_files.len()
    );

    // 4. K-Way 多路归并流式累加
    let mut readers: Vec<BufReader<File>> = chunk_files
        .iter()
        .map(|p| BufReader::new(File::open(p).unwrap()))
        .collect();

    let mut heap = BinaryHeap::new();

    fn read_pair(r: &mut BufReader<File>) -> Option<RawBigramPair> {
        let mut buf = [0u8; 12];
        if r.read_exact(&mut buf).is_ok() {
            let w1_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
            let w2_id = u32::from_le_bytes(buf[4..8].try_into().unwrap());
            let count = u32::from_le_bytes(buf[8..12].try_into().unwrap());
            Some(RawBigramPair {
                w1_id,
                w2_id,
                count,
            })
        } else {
            None
        }
    }

    for (idx, r) in readers.iter_mut().enumerate() {
        if let Some(pair) = read_pair(r) {
            heap.push(MergeNode {
                pair,
                chunk_idx: idx,
            });
        }
    }

    let mut aggregated_pairs: Vec<(u32, u32, i32, u32)> = Vec::new();
    let mut current_pair: Option<RawBigramPair> = None;

    while let Some(top) = heap.pop() {
        if let Some(mut cur) = current_pair {
            if cur.w1_id == top.pair.w1_id && cur.w2_id == top.pair.w2_id {
                cur.count += top.pair.count;
                current_pair = Some(cur);
            } else {
                settle_pair_affinity(cur, &global_unigram, &mut aggregated_pairs);
                current_pair = Some(top.pair);
            }
        } else {
            current_pair = Some(top.pair);
        }

        if let Some(next_p) = read_pair(&mut readers[top.chunk_idx]) {
            heap.push(MergeNode {
                pair: next_p,
                chunk_idx: top.chunk_idx,
            });
        }
    }

    if let Some(cur) = current_pair {
        settle_pair_affinity(cur, &global_unigram, &mut aggregated_pairs);
    }

    println!(
        "   归并汇总完成！全局有效高频二元搭配总数: {} 对",
        aggregated_pairs.len()
    );

    // 5. 按频次排序并输出 16MB 与 32MB 两个标准规格
    println!("\n[4/5] 按照综合权重排序并导出 16MB 与 32MB 两个标准规格模型...");
    aggregated_pairs.par_sort_unstable_by(|a, b| b.3.cmp(&a.3));

    let cap_16mb = 3_800_000.min(aggregated_pairs.len());
    let mut pairs_16mb = aggregated_pairs[..cap_16mb].to_vec();
    pairs_16mb.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    export_csr_model(out_16mb_path, vocab.len(), cap_16mb, &pairs_16mb).expect("生成16MB模型失败");

    let cap_32mb = 7_800_000.min(aggregated_pairs.len());
    let mut pairs_32mb = aggregated_pairs[..cap_32mb].to_vec();
    pairs_32mb.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    export_csr_model(out_32mb_path, vocab.len(), cap_32mb, &pairs_32mb).expect("生成32MB模型失败");

    let _ = fs::remove_dir_all(temp_chunk_dir);
    vocab.save_binary(vocab_bin_path).expect("更新词表成功");

    let elapsed_total = start_total.elapsed();
    println!("\n[5/5] 构建全流程圆满结束！");
    println!("=================================================================");
    println!(
        "  词表产物:     {} (收录词条: {})",
        vocab_bin_path.display(),
        vocab.len()
    );
    println!("  16MB 规格:    {}", out_16mb_path.display());
    println!("  32MB 规格:    {}", out_32mb_path.display());
    println!(
        "  全流程总耗时: {:.2} 秒 ({:.1} 分钟)",
        elapsed_total.as_secs_f64(),
        elapsed_total.as_secs_f64() / 60.0
    );
    println!("=================================================================");
}
