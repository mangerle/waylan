//! 微澜输入法 (WayLan) 全量语料 Bigram CSR 极致压缩构建主管线
//!
//! # 设计原理
//! - **实现初衷**：将 500 行以上的庞大单体离线构建工具按单一职责拆分为语料清洗器 (`cleaner`)、
//!   外排序归并器 (`merger`)、CSR 模型导出器 (`exporter`) 与调度累加器 (`accumulator`)。
//! - **核心优势**：关注点彻底分离，可维护性与模块可读性显著提升。

mod accumulator;
mod cleaner;
mod exporter;
mod merger;

use jieba_rs::Jieba;
use rayon::prelude::*;
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use accumulator::CorpusAccumulator;
use cleaner::Cleaner;
use exporter::export_csr_model;
use merger::{flush_chunk_to_disk, read_pair, settle_pair_affinity, MergeNode, RawBigramPair};
use waylan_algo::vocab::Vocabulary;

fn main() {
    println!("=================================================================");
    println!("  微澜输入法 (WayLan) 全量语料 Bigram CSR 极致压缩构建管线");
    println!("=================================================================");

    let start_total = Instant::now();
    let data_dir = env::var("WAYLAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            manifest
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("data"))
                .unwrap_or_else(|| PathBuf::from("data"))
        });

    let pretrain_path = env::var("WAYLAN_PRETRAIN_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("corpus/pretrain_t2t.jsonl"));
    let sft_path = env::var("WAYLAN_SFT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("corpus/sft_t2t.jsonl"));
    let dict_dir = data_dir.join("dicts");
    let vocab_bin_path = data_dir.join("vocab.bin");
    let out_16mb_path = data_dir.join("bigram_csr_16mb.bin");
    let out_32mb_path = data_dir.join("bigram_csr_32mb.bin");
    let temp_chunk_dir = data_dir.join("temp_chunks");

    let _ = fs::remove_dir_all(&temp_chunk_dir);
    fs::create_dir_all(&temp_chunk_dir).expect("创建分片临时目录成功");

    println!("\n[1/5] 构建基础统一词表 (Vocabulary)...");
    let yaml_files = vec![
        dict_dir.join("1_chars/rime_mint.chars.dict.yaml"),
        dict_dir.join("2_base/rime_mint.base.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.ext.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.places.dict.yaml"),
    ];

    let vocab = Vocabulary::load_from_dict_yamls(&yaml_files).expect("解析词典YAML失败");
    let jieba = Arc::new(Jieba::new());
    vocab.save_binary(&vocab_bin_path).expect("持久化词表成功");

    let tools = (Arc::new(Cleaner::new()), jieba);
    let mut acc = CorpusAccumulator {
        global_unigram: HashMap::with_capacity(vocab.len()),
        vocab,
        current_chunk_map: HashMap::with_capacity(3_000_000),
        chunk_files: Vec::new(),
        chunk_index: 0,
    };

    let batch_lines_limit = 50_000;
    let chunk_flush_pairs_limit = 4_000_000;
    let datasets = [
        (&pretrain_path, false, "预训练语料 (pretrain_t2t.jsonl)"),
        (&sft_path, true, "指令对话语料 (sft_t2t.jsonl)"),
    ];

    println!("\n[2/5] 启动流式多线程分词与二元统计...");
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

        for line in reader.lines().map_while(Result::ok) {
            batch_buffer.push(line);
            if batch_buffer.len() >= batch_lines_limit {
                processed_lines = processed_lines.saturating_add(batch_buffer.len());
                let lines_to_process =
                    mem::replace(&mut batch_buffer, Vec::with_capacity(batch_lines_limit));
                acc.consume_batch(&lines_to_process, is_sft, &tools);
                print!(
                    "\r   已处理文本: {:>10} 行 | 局部缓存词对: {:>10} 对 | 词表扩充容量: {:>7} 词",
                    processed_lines,
                    acc.current_chunk_map.len(),
                    acc.vocab.len()
                );
                let _ = io::stdout().flush();
                acc.maybe_flush_chunk(&temp_chunk_dir, chunk_flush_pairs_limit);
            }
        }

        // 刷写文件末尾不足一个完整批次的尾批语料，防止小语料或尾数行丢失
        if !batch_buffer.is_empty() {
            processed_lines = processed_lines.saturating_add(batch_buffer.len());
            acc.consume_batch(&batch_buffer, is_sft, &tools);
            batch_buffer.clear();
            acc.maybe_flush_chunk(&temp_chunk_dir, chunk_flush_pairs_limit);
        }
        println!("\n   -> 当前语料完成，累计行数: {}", processed_lines);
    }

    if !acc.current_chunk_map.is_empty() {
        let chunk_path =
            flush_chunk_to_disk(&temp_chunk_dir, acc.chunk_index, &mut acc.current_chunk_map)
                .expect("刷写最终临时分片文件失败");
        acc.chunk_files.push(chunk_path);
    }

    println!(
        "\n[3/5] 分片扫描完毕！共生成 {} 个有序局部临时分片，进入多路归并汇聚...",
        acc.chunk_files.len()
    );

    let mut readers: Vec<BufReader<File>> = acc
        .chunk_files
        .iter()
        .map(|p| BufReader::new(File::open(p).expect("打开归并分片文件失败")))
        .collect();
    let mut heap = BinaryHeap::new();
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
                cur.count = cur.count.saturating_add(top.pair.count);
                current_pair = Some(cur);
            } else {
                settle_pair_affinity(cur, &acc.global_unigram, &mut aggregated_pairs);
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
        settle_pair_affinity(cur, &acc.global_unigram, &mut aggregated_pairs);
    }

    println!("\n[4/5] 按照综合权重排序并导出 16MB 与 32MB 两个标准规格模型...");
    aggregated_pairs.par_sort_unstable_by(|a, b| b.3.cmp(&a.3));

    for (cap, path, msg) in [
        (3_800_000, &out_16mb_path, "生成16MB模型失败"),
        (7_800_000, &out_32mb_path, "生成32MB模型失败"),
    ] {
        let take = cap.min(aggregated_pairs.len());
        let mut sub_pairs = aggregated_pairs[..take].to_vec();
        sub_pairs.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        export_csr_model(path, acc.vocab.len(), take, &sub_pairs).expect(msg);
    }

    let _ = fs::remove_dir_all(&temp_chunk_dir);
    acc.vocab
        .save_binary(&vocab_bin_path)
        .expect("更新词表成功");
    let elapsed_total = start_total.elapsed();
    println!(
        "\n[5/5] 构建结束！词表: {} 条 | 总耗时: {:.2} 秒",
        acc.vocab.len(),
        elapsed_total.as_secs_f64()
    );
}
