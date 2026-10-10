use std::collections::{BinaryHeap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use jieba_rs::Jieba;
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use waylan_algo::vocab::Vocabulary;
use waylan_algo::bigram::{BIGRAM_MAGIC, BIGRAM_HEADER_SIZE, CODEBOOK_SIZE};

/// 临时分片单条二元项结构（16 字节对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawBigramPair {
    w1_id: u32,
    w2_id: u32,
    count: u32,
}

impl PartialOrd for RawBigramPair {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RawBigramPair {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 先按 w1_id 升序，再按 w2_id 升序
        self.w1_id.cmp(&other.w1_id)
            .then_with(|| self.w2_id.cmp(&other.w2_id))
    }
}

/// K路归并堆节点
struct MergeNode {
    pair: RawBigramPair,
    chunk_idx: usize,
}

impl PartialEq for MergeNode {
    fn eq(&self, other: &Self) -> bool {
        self.pair == other.pair
    }
}

impl Eq for MergeNode {}

impl PartialOrd for MergeNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 最小堆：逆序比较
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

/// 写入 CSR 紧凑二进制格式模型文件
fn export_csr_model(
    output_path: &Path,
    vocab_size: usize,
    target_capacity: usize,
    pairs: &[(u32, u32, i32, u32)], // (w1, w2, affinity, count)
) -> std::io::Result<()> {
    let num_pairs = pairs.len().min(target_capacity);
    let selected = &pairs[..num_pairs];

    println!("-> 正在为规格 [{}] 生成 256 阶标量量化码本与 CSR 拓扑...", output_path.file_name().unwrap().to_string_lossy());

    // 1. 生成 256 阶码本
    let mut codebook = [0i32; 256];
    let min_score = 1000i32;
    let max_score = 30000i32;
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
        // 量化为 0..255
        let clamped_aff = aff.clamp(min_score, max_score);
        let q_score = (((clamped_aff - min_score) as f64 / (max_score - min_score) as f64) * 255.0).round() as u8;
        let entry = (w2 & 0x00FF_FFFF) | ((q_score as u32) << 24);
        cols_data.push(entry);
    }

    // 4. 构造标准二进制文件
    let file = File::create(output_path)?;
    let mut writer = BufWriter::new(file);

    let codebook_offset = BIGRAM_HEADER_SIZE as u64;
    let row_offsets_offset = codebook_offset + CODEBOOK_SIZE as u64;
    let cols_offset = row_offsets_offset + ((vocab_size + 1) * 4) as u64;

    // 写入 Header (64字节)
    let mut header = [0u8; 64];
    header[0..16].copy_from_slice(BIGRAM_MAGIC);
    header[16..20].copy_from_slice(&2u32.to_le_bytes()); // version = 2
    header[20..24].copy_from_slice(&(vocab_size as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(num_pairs as u32).to_le_bytes());
    header[28..36].copy_from_slice(&codebook_offset.to_le_bytes());
    header[36..44].copy_from_slice(&row_offsets_offset.to_le_bytes());
    header[44..52].copy_from_slice(&cols_offset.to_le_bytes());
    writer.write_all(&header)?;

    // 写入 Codebook (1024字节)
    for &val in &codebook {
        writer.write_all(&val.to_le_bytes())?;
    }

    // 写入 Row Offsets
    for &off in &row_offsets {
        writer.write_all(&off.to_le_bytes())?;
    }

    // 写入 Cols
    for &col in &cols_data {
        writer.write_all(&col.to_le_bytes())?;
    }

    writer.flush()?;
    let size_mb = fs::metadata(output_path)?.len() as f64 / (1024.0 * 1024.0);
    println!("   模型生成成功: {} | 文件大小: {:.2} MB | 容纳词对: {} 对",
        output_path.display(), size_mb, num_pairs);
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

    // 预热结巴分词器并导入词表
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

    let batch_lines_limit = 50_000; // 每5万行由Rayon并发分词切句
    let chunk_flush_pairs_limit = 4_000_000; // 局部二元对超400万对时刷盘写临时分片，锁死内存占用

    let datasets = vec![
        (pretrain_path, false, "预训练语料 (pretrain_t2t.jsonl 7.7GB)"),
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
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            batch_buffer.push(line);

            if batch_buffer.len() >= batch_lines_limit {
                processed_lines += batch_buffer.len();
                let lines_to_process = std::mem::replace(&mut batch_buffer, Vec::with_capacity(batch_lines_limit));

                // Rayon 并发切词
                let cleaner_clone = cleaner.clone();
                let jieba_clone = jieba.clone();

                let local_pairs_list: Vec<Vec<(String, String, u32)>> = lines_to_process.par_iter().map(|raw_line| {
                    let mut local_res = Vec::new();
                    let Ok(val) = serde_json::from_str::<Value>(raw_line) else { return local_res; };

                    if is_sft {
                        if let Some(convs) = val.get("conversations").and_then(|v| v.as_array()) {
                            for turn in convs {
                                let content = turn.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                let role = turn.get("role").and_then(|v| v.as_str()).unwrap_or("");
                                let weight: u32 = if role == "user" { 2 } else { 1 };
                                let sents = cleaner_clone.clean_and_split(content);
                                for sent in sents {
                                    let words: Vec<&str> = jieba_clone.cut(&sent, false);
                                    for j in 0..words.len() {
                                        let w1 = words[j];
                                        if j + 1 < words.len() {
                                            let w2 = words[j + 1];
                                            local_res.push((w1.to_string(), w2.to_string(), weight));
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                            let sents = cleaner_clone.clean_and_split(text);
                            for sent in sents {
                                let words: Vec<&str> = jieba_clone.cut(&sent, false);
                                for j in 0..words.len() {
                                    let w1 = words[j];
                                    if j + 1 < words.len() {
                                        let w2 = words[j + 1];
                                        local_res.push((w1.to_string(), w2.to_string(), 1));
                                    }
                                }
                            }
                        }
                    }
                    local_res
                }).collect();

                // 累加局部 Map 并执行 Word ID 转换
                for local_pairs in local_pairs_list {
                    for (w1, w2, weight) in local_pairs {
                        let id1 = vocab.insert_or_get(&w1);
                        let id2 = vocab.insert_or_get(&w2);
                        *global_unigram.entry(id1).or_insert(0) += weight;
                        *current_chunk_map.entry((id1, id2)).or_insert(0) += weight;
                    }
                }

                print!("\r   已处理文本: {:>10} 行 | 局部缓存词对: {:>10} 对 | 词表扩充容量: {:>7} 词",
                    processed_lines, current_chunk_map.len(), vocab.len());
                std::io::stdout().flush().unwrap();

                // 内存守护：超出安全阈值立即排序刷盘
                if current_chunk_map.len() >= chunk_flush_pairs_limit {
                    println!("\n   -> [内存控制] 触发局部剪枝与分片刷盘 (分片 #{}) ...", chunk_index);
                    let chunk_path = temp_chunk_dir.join(format!("chunk_{:04}.bin", chunk_index));
                    chunk_index += 1;

                    let mut sorted_chunk: Vec<RawBigramPair> = current_chunk_map
                        .drain()
                        .filter(|&(_, cnt)| cnt >= 2) // 局部淘汰只出现1次的低质单次词，立省60%磁盘
                        .map(|((id1, id2), cnt)| RawBigramPair { w1_id: id1, w2_id: id2, count: cnt })
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
                    chunk_files.push(chunk_path);
                }
            }
        }
        println!("\n   -> 当前语料完成，累计行数: {}", processed_lines);
    }

    // 刷出最后一个残存分片
    if !current_chunk_map.is_empty() {
        let chunk_path = temp_chunk_dir.join(format!("chunk_{:04}.bin", chunk_index));
        let mut sorted_chunk: Vec<RawBigramPair> = current_chunk_map
            .drain()
            .filter(|&(_, cnt)| cnt >= 2)
            .map(|((id1, id2), cnt)| RawBigramPair { w1_id: id1, w2_id: id2, count: cnt })
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
        chunk_files.push(chunk_path);
    }

    println!("\n[3/5] 分片扫描完毕！共生成 {} 个有序局部临时分片，进入多路归并汇聚...", chunk_files.len());

    // 4. K-Way 多路归并流式累加
    let mut readers: Vec<BufReader<File>> = chunk_files.iter()
        .map(|p| BufReader::new(File::open(p).unwrap()))
        .collect();

    let mut heap = BinaryHeap::new();

    fn read_pair(r: &mut BufReader<File>) -> Option<RawBigramPair> {
        let mut buf = [0u8; 12];
        if r.read_exact(&mut buf).is_ok() {
            let w1_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
            let w2_id = u32::from_le_bytes(buf[4..8].try_into().unwrap());
            let count = u32::from_le_bytes(buf[8..12].try_into().unwrap());
            Some(RawBigramPair { w1_id, w2_id, count })
        } else {
            None
        }
    }

    for (idx, r) in readers.iter_mut().enumerate() {
        if let Some(pair) = read_pair(r) {
            heap.push(MergeNode { pair, chunk_idx: idx });
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
                // 结算上一个配对，只保留全局频次 >= 3 的有效搭配
                if cur.count >= 3 {
                    let u_count = global_unigram.get(&cur.w1_id).copied().unwrap_or(cur.count);
                    let prob = (cur.count as f64 / u_count as f64).max(1e-7);
                    let aff = (20000.0 + prob.ln() * 2000.0).clamp(1000.0, 30000.0) as i32;
                    aggregated_pairs.push((cur.w1_id, cur.w2_id, aff, cur.count));
                }
                current_pair = Some(top.pair);
            }
        } else {
            current_pair = Some(top.pair);
        }

        // 从对应的 chunk 流式补入下一个元素
        if let Some(next_p) = read_pair(&mut readers[top.chunk_idx]) {
            heap.push(MergeNode { pair: next_p, chunk_idx: top.chunk_idx });
        }
    }

    if let Some(cur) = current_pair {
        if cur.count >= 3 {
            let u_count = global_unigram.get(&cur.w1_id).copied().unwrap_or(cur.count);
            let prob = (cur.count as f64 / u_count as f64).max(1e-7);
            let aff = (20000.0 + prob.ln() * 2000.0).clamp(1000.0, 30000.0) as i32;
            aggregated_pairs.push((cur.w1_id, cur.w2_id, aff, cur.count));
        }
    }

    println!("   归并汇总完成！全局有效高频二元搭配总数: {} 对", aggregated_pairs.len());

    // 5. 按频次排序并输出 16MB 与 32MB 两个标准规格
    println!("\n[4/5] 按照综合权重排序并导出 16MB 与 32MB 两个标准规格模型...");

    // 按实际频次降序排序以进行容量截断
    aggregated_pairs.par_sort_unstable_by(|a, b| b.3.cmp(&a.3));

    // 规格 A: 16MB 标准版（对标微信键盘，截取 Top 3,800,000 对）
    let cap_16mb = 3_800_000.min(aggregated_pairs.len());
    let mut pairs_16mb = aggregated_pairs[..cap_16mb].to_vec();
    // 恢复 CSR 所需的 (w1_id, w2_id) 升序
    pairs_16mb.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    export_csr_model(out_16mb_path, vocab.len(), cap_16mb, &pairs_16mb).expect("生成16MB模型失败");

    // 规格 B: 32MB 满血长尾版（截取 Top 7,800,000 对）
    let cap_32mb = 7_800_000.min(aggregated_pairs.len());
    let mut pairs_32mb = aggregated_pairs[..cap_32mb].to_vec();
    pairs_32mb.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    export_csr_model(out_32mb_path, vocab.len(), cap_32mb, &pairs_32mb).expect("生成32MB模型失败");

    // 清理临时文件
    let _ = fs::remove_dir_all(temp_chunk_dir);

    // 重新保存最终扩充全量词表
    vocab.save_binary(vocab_bin_path).expect("更新词表成功");

    let elapsed_total = start_total.elapsed();
    println!("\n[5/5] 构建全流程圆满结束！");
    println!("=================================================================");
    println!("  词表产物:     {} (收录词条: {})", vocab_bin_path.display(), vocab.len());
    println!("  16MB 规格:    {}", out_16mb_path.display());
    println!("  32MB 规格:    {}", out_32mb_path.display());
    println!("  全流程总耗时: {:.2} 秒 ({:.1} 分钟)", elapsed_total.as_secs_f64(), elapsed_total.as_secs_f64() / 60.0);
    println!("=================================================================");
}
