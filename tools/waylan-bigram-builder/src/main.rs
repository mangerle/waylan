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
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use accumulator::CorpusAccumulator;
use cleaner::Cleaner;
use exporter::export_csr_model;
use merger::{flush_chunk_to_disk, read_pair, settle_pair_affinity, MergeNode, RawBigramPair};
use waylan_algo::vocab::Vocabulary;

/// 构建管线统一错误类型：聚合词表算法错误与标准 IO 错误
type BuildResult<T> = Result<T, Box<dyn Error>>;

/// 单批流式处理的最大语料行数
const BATCH_LINES_LIMIT: usize = 50_000;

/// 触发分片落盘前允许累积的最大局部词对数
const CHUNK_FLUSH_PAIRS_LIMIT: usize = 4_000_000;

/// 16MB 规格模型的词对容量上限
const MODEL_16MB_CAPACITY: usize = 3_800_000;

/// 32MB 规格模型的词对容量上限
const MODEL_32MB_CAPACITY: usize = 7_800_000;

/// 构建管线所需的全部输入输出路径
struct BuildPaths {
    /// 预训练语料路径
    pretrain_path: PathBuf,
    /// 指令对话语料路径
    sft_path: PathBuf,
    /// 词典 YAML 目录
    dict_dir: PathBuf,
    /// 词表二进制输出路径
    vocab_bin_path: PathBuf,
    /// 16MB 规格模型输出路径
    out_16mb_path: PathBuf,
    /// 32MB 规格模型输出路径
    out_32mb_path: PathBuf,
    /// 分片临时目录
    temp_chunk_dir: PathBuf,
}

/// 单个语料数据集的流式处理参数包
struct CorpusProcessParams<'a> {
    /// 语料文件路径
    corpus_path: &'a Path,
    /// 是否为指令对话微调语料
    is_sft: bool,
    /// 语料用途标签（用于进度输出）
    label: &'a str,
    /// 分片临时目录
    temp_chunk_dir: &'a Path,
    /// 语料统计累加器
    acc: &'a mut CorpusAccumulator,
    /// 分词与清洗工具组合
    tools: &'a (Arc<Cleaner>, Arc<Jieba>),
}

/// 解析构建管线所需的全部输入输出路径
fn resolve_build_paths() -> BuildPaths {
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

    BuildPaths {
        pretrain_path,
        sft_path,
        dict_dir: data_dir.join("dicts"),
        vocab_bin_path: data_dir.join("vocab.bin"),
        out_16mb_path: data_dir.join("bigram_csr_16mb.bin"),
        out_32mb_path: data_dir.join("bigram_csr_32mb.bin"),
        temp_chunk_dir: data_dir.join("temp_chunks"),
    }
}

/// 加载词典 YAML 并持久化基础统一词表
///
/// # Errors
/// 当词典 YAML 解析失败或词表二进制持久化失败时返回底层错误。
fn build_vocabulary(dict_dir: &Path, vocab_bin_path: &Path) -> BuildResult<Vocabulary> {
    let yaml_files = vec![
        dict_dir.join("1_chars/rime_mint.chars.dict.yaml"),
        dict_dir.join("2_base/rime_mint.base.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.ext.dict.yaml"),
        dict_dir.join("3_ext/rime_mint.places.dict.yaml"),
    ];

    let vocab = Vocabulary::load_from_dict_yamls(&yaml_files)?;
    vocab.save_binary(vocab_bin_path)?;
    Ok(vocab)
}

/// 输出当前流式处理进度快照
fn print_progress(processed_lines: usize, acc: &CorpusAccumulator) {
    print!(
        "\r   已处理文本: {:>10} 行 | 局部缓存词对: {:>10} 对 | 词表扩充容量: {:>7} 词",
        processed_lines,
        acc.current_chunk_map.len(),
        acc.vocab.len()
    );
    let _ = io::stdout().flush();
}

/// 流式处理单个语料数据集，将词对统计累积至分片缓存
///
/// # Errors
/// 当语料文件打开失败或分片刷盘写入失败时返回底层 IO 错误。
fn process_corpus_dataset(params: CorpusProcessParams<'_>) -> BuildResult<()> {
    if !params.corpus_path.exists() {
        println!(
            "警告: 未找到语料文件 {}, 跳过此部分",
            params.corpus_path.display()
        );
        return Ok(());
    }
    println!("-> 正在流式处理: {} ...", params.label);

    let file = File::open(params.corpus_path)?;
    let reader = BufReader::with_capacity(16 * 1024 * 1024, file);
    let mut batch_buffer = Vec::with_capacity(BATCH_LINES_LIMIT);
    let mut processed_lines: usize = 0;

    for line in reader.lines().map_while(Result::ok) {
        batch_buffer.push(line);
        if batch_buffer.len() >= BATCH_LINES_LIMIT {
            processed_lines = processed_lines.saturating_add(batch_buffer.len());
            let lines_to_process =
                mem::replace(&mut batch_buffer, Vec::with_capacity(BATCH_LINES_LIMIT));
            params
                .acc
                .consume_batch(&lines_to_process, params.is_sft, params.tools);
            print_progress(processed_lines, params.acc);
            params
                .acc
                .maybe_flush_chunk(params.temp_chunk_dir, CHUNK_FLUSH_PAIRS_LIMIT)?;
        }
    }

    // 刷写文件末尾不足一个完整批次的尾批语料，防止小语料或尾数行丢失
    if !batch_buffer.is_empty() {
        processed_lines = processed_lines.saturating_add(batch_buffer.len());
        params
            .acc
            .consume_batch(&batch_buffer, params.is_sft, params.tools);
        batch_buffer.clear();
        params
            .acc
            .maybe_flush_chunk(params.temp_chunk_dir, CHUNK_FLUSH_PAIRS_LIMIT)?;
    }
    println!("\n   -> 当前语料完成，累计行数: {}", processed_lines);
    Ok(())
}

/// 依次流式处理预训练与指令对话两大语料数据集
///
/// # Errors
/// 当任一语料数据集处理失败时透传底层 IO 错误。
fn process_all_datasets(paths: &BuildPaths, acc: &mut CorpusAccumulator) -> BuildResult<()> {
    let jieba = Arc::new(Jieba::new());
    let tools = (Arc::new(Cleaner::new()), jieba);

    let datasets = [
        (
            &paths.pretrain_path,
            false,
            "预训练语料 (pretrain_t2t.jsonl)",
        ),
        (&paths.sft_path, true, "指令对话语料 (sft_t2t.jsonl)"),
    ];

    for (corpus_path, is_sft, label) in datasets {
        process_corpus_dataset(CorpusProcessParams {
            corpus_path,
            is_sft,
            label,
            temp_chunk_dir: &paths.temp_chunk_dir,
            acc: &mut *acc,
            tools: &tools,
        })?;
    }
    Ok(())
}

/// 刷写残留的局部词对缓存为最终临时分片文件
///
/// # Errors
/// 当最终分片文件写入失败时返回底层 IO 错误。
fn flush_final_chunk(paths: &BuildPaths, acc: &mut CorpusAccumulator) -> BuildResult<()> {
    if acc.current_chunk_map.is_empty() {
        return Ok(());
    }

    let chunk_path = flush_chunk_to_disk(
        &paths.temp_chunk_dir,
        acc.chunk_index,
        &mut acc.current_chunk_map,
    )?;
    acc.chunk_files.push(chunk_path);
    Ok(())
}

/// 多路归并汇聚全部分片词对，并按亲和度聚合为最终词对序列
///
/// # Errors
/// 当任一分片文件无法打开读取时返回底层 IO 错误。
fn merge_chunk_files(acc: &CorpusAccumulator) -> BuildResult<Vec<(u32, u32, i32, u32)>> {
    let mut readers: Vec<BufReader<File>> = acc
        .chunk_files
        .iter()
        .map(|p| File::open(p).map(BufReader::new))
        .collect::<io::Result<Vec<_>>>()?;
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

    Ok(aggregated_pairs)
}

/// 按综合权重排序并导出 16MB 与 32MB 两个标准规格模型
///
/// # Errors
/// 当目标模型文件写入失败时返回带有输出路径上下文的 IO 错误。
fn export_standard_models(
    aggregated_pairs: &mut [(u32, u32, i32, u32)],
    vocab_size: usize,
    out_16mb_path: &Path,
    out_32mb_path: &Path,
) -> BuildResult<()> {
    aggregated_pairs.par_sort_unstable_by(|a, b| b.3.cmp(&a.3));

    for (cap, path) in [
        (MODEL_16MB_CAPACITY, out_16mb_path),
        (MODEL_32MB_CAPACITY, out_32mb_path),
    ] {
        let take = cap.min(aggregated_pairs.len());
        let mut sub_pairs = aggregated_pairs[..take].to_vec();
        sub_pairs.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        export_csr_model(path, vocab_size, take, &sub_pairs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("导出模型文件 {} 失败: {e}", path.display()),
            )
        })?;
    }
    Ok(())
}

/// 构建语料统计累加器，并完成统一词表的加载与首次持久化
///
/// # Errors
/// 当统一词表加载或持久化失败时透传底层错误。
fn build_corpus_accumulator(paths: &BuildPaths) -> BuildResult<CorpusAccumulator> {
    let vocab = build_vocabulary(&paths.dict_dir, &paths.vocab_bin_path)?;
    Ok(CorpusAccumulator {
        global_unigram: HashMap::with_capacity(vocab.len()),
        vocab,
        current_chunk_map: HashMap::with_capacity(3_000_000),
        chunk_files: Vec::new(),
        chunk_index: 0,
    })
}

fn main() {
    if let Err(err) = run() {
        eprintln!("[构建失败] {err}");
        std::process::exit(1);
    }
}

/// 执行完整的 Bigram CSR 构建管线
///
/// # Errors
/// 当任一构建阶段（词表构建、语料统计、分片归并、模型导出）失败时返回带中文上下文的错误链。
fn run() -> BuildResult<()> {
    println!("=================================================================");
    println!("  微澜输入法 (WayLan) 全量语料 Bigram CSR 极致压缩构建管线");
    println!("=================================================================");

    let start_total = Instant::now();
    let paths = resolve_build_paths();

    let _ = fs::remove_dir_all(&paths.temp_chunk_dir);
    fs::create_dir_all(&paths.temp_chunk_dir)
        .map_err(|e| io::Error::new(e.kind(), format!("创建分片临时目录失败: {e}")))?;

    println!("\n[1/5] 构建基础统一词表 (Vocabulary)...");
    let mut acc = build_corpus_accumulator(&paths)?;

    println!("\n[2/5] 启动流式多线程分词与二元统计...");
    process_all_datasets(&paths, &mut acc)?;
    flush_final_chunk(&paths, &mut acc)?;

    println!(
        "\n[3/5] 分片扫描完毕！共生成 {} 个有序局部临时分片，进入多路归并汇聚...",
        acc.chunk_files.len()
    );

    let mut aggregated_pairs = merge_chunk_files(&acc)?;

    println!("\n[4/5] 按照综合权重排序并导出 16MB 与 32MB 两个标准规格模型...");
    export_standard_models(
        &mut aggregated_pairs,
        acc.vocab.len(),
        &paths.out_16mb_path,
        &paths.out_32mb_path,
    )?;

    let _ = fs::remove_dir_all(&paths.temp_chunk_dir);
    acc.vocab.save_binary(&paths.vocab_bin_path)?;
    let elapsed_total = start_total.elapsed();
    println!(
        "\n[5/5] 构建结束！词表: {} 条 | 总耗时: {:.2} 秒",
        acc.vocab.len(),
        elapsed_total.as_secs_f64()
    );
    Ok(())
}
