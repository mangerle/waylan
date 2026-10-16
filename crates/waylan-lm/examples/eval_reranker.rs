//! 微澜 (WayLan) 工业级拼音消歧评测与准召率基准评估工具
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供大规模、端到端、真实的拼音整句解码评估流水线。
//!   直接桥接算法引擎 `AlgoEngine` 与全量词库 `base.dict.bin`，既支持单跑纯词频基线，
//!   也支持与神经网络语言模型进行对比，真实量化 10,000 ~ 500,000 条长句下的 Top-1 准确率与延时。
//! - **核心优势**：自动感知系统 CPU 物理核心数并自适应多线程并行评估（支持通过 --threads 自定义），
//!   按短词（2~4字）、中句（5~8字）、长句（9~18字）分层统计，配合单字命中率精准诊断。
//! - **代价与局限**：多线程模式下各工作线程独立挂载虚拟内存 Mmap 词典，各线程结果并行聚合。

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;
use waylan_algo::{AlgoEngine, NoopReranker, SentenceReranker};
use waylan_core::config::AppConfig;
use waylan_lm::weights::{LayerWeights, ModelWeights, Tensor2D};
use waylan_lm::{LMTokenizer, NeuralReranker, WayLanLMConfig, WayLanLMModel};

/// 评测样本数据契约
#[derive(Debug, Clone, serde::Deserialize)]
struct SampleEntry {
    #[serde(default)]
    #[allow(dead_code)]
    pub id: usize,
    pub target: String,
    pub pinyin: String,
}

/// 命令行配置选项
struct EvalOptions {
    pub baseline_only: bool,
    pub dataset_path: Option<PathBuf>,
    pub limit: Option<usize>,
    pub explicit_split: bool,
    pub model_path: Option<PathBuf>,
    pub tokenizer_path: Option<PathBuf>,
    pub threads: Option<usize>,
}

fn parse_options() -> EvalOptions {
    let args: Vec<String> = env::args().collect();
    let mut baseline_only = false;
    let mut dataset_path = None;
    let mut limit = None;
    let mut explicit_split = false;
    let mut model_path = None;
    let mut tokenizer_path = None;
    let mut threads = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--baseline-only" => {
                baseline_only = true;
                i += 1;
            }
            "--explicit-split" => {
                explicit_split = true;
                i += 1;
            }
            "--dataset" if i + 1 < args.len() => {
                dataset_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--limit" if i + 1 < args.len() => {
                limit = args[i + 1].parse::<usize>().ok();
                i += 2;
            }
            "--model" if i + 1 < args.len() => {
                model_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--tokenizer" if i + 1 < args.len() => {
                tokenizer_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--threads" if i + 1 < args.len() => {
                threads = args[i + 1].parse::<usize>().ok();
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    EvalOptions {
        baseline_only,
        dataset_path,
        limit,
        explicit_split,
        model_path,
        tokenizer_path,
        threads,
    }
}

/// 自动加载全量系统词典表并构建评测引擎
fn load_evaluation_engine() -> AlgoEngine {
    let search_paths = AppConfig::base_dict_search_paths();
    let mut resolved_table = None;

    for p in &search_paths {
        if p.exists() {
            if let Ok(table) = LexiconTable::load_binary(p) {
                resolved_table = Some(table);
                break;
            }
        }
    }

    let table = resolved_table.unwrap_or_else(LexiconTable::with_sample_data);

    let config = AlgoConfig {
        page_size: 5,
        ..Default::default()
    };
    AlgoEngine::new(table, config)
}

/// 从外部 .jsonl 文件加载数据集，优先载入纯净独立测试集
fn load_dataset(custom_path: Option<&Path>, limit: Option<usize>) -> (Vec<SampleEntry>, String) {
    let dataset_candidates = [
        custom_path.map(|p| p.to_path_buf()),
        Some(PathBuf::from("data/eval/eval_heldout_10k.jsonl")),
        Some(PathBuf::from("data/eval/eval_heldout_100k.jsonl")),
        Some(PathBuf::from("data/eval/eval_heldout_500k.jsonl")),
    ];

    for cand_opt in dataset_candidates.into_iter().flatten() {
        if cand_opt.is_file() {
            if let Ok(file) = File::open(&cand_opt) {
                let reader = BufReader::new(file);
                let mut samples = Vec::new();
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(entry) = serde_json::from_str::<SampleEntry>(&line) {
                        samples.push(entry);
                        if let Some(lim) = limit {
                            if samples.len() >= lim {
                                break;
                            }
                        }
                    }
                }
                if !samples.is_empty() {
                    let desc = format!("{} (共 {} 条样本)", cand_opt.display(), samples.len());
                    return (samples, desc);
                }
            }
        }
    }

    // 保底内置用例
    let fallback = vec![
        SampleEntry {
            id: 1,
            target: "西安".to_string(),
            pinyin: "xi'an".to_string(),
        },
        SampleEntry {
            id: 2,
            target: "清华大学".to_string(),
            pinyin: "qing'hua'da'xue".to_string(),
        },
        SampleEntry {
            id: 3,
            target: "人工智能".to_string(),
            pinyin: "ren'gong'zhi'neng".to_string(),
        },
        SampleEntry {
            id: 4,
            target: "深度学习".to_string(),
            pinyin: "shen'du'xue'xi".to_string(),
        },
        SampleEntry {
            id: 5,
            target: "今天天气真好".to_string(),
            pinyin: "jin'tian'tian'qi'zhen'hao".to_string(),
        },
    ];
    (fallback, "内置精选冒烟测试集 (5 条)".to_string())
}

/// 执行单次拼音模拟敲击解码
fn decode_pinyin(engine: &mut AlgoEngine, pinyin: &str, explicit_split: bool) -> Option<String> {
    engine.reset();
    let stream: Box<dyn Iterator<Item = char>> = if explicit_split {
        Box::new(pinyin.chars())
    } else {
        Box::new(pinyin.chars().filter(|&c| c != '\''))
    };

    for ch in stream {
        engine.push_char(ch);
    }

    engine.candidates().first().map(|c| c.text.clone())
}

/// 自动探测或载入神经网络重排序器
fn resolve_neural_reranker(
    cli_m: Option<PathBuf>,
    cli_t: Option<PathBuf>,
) -> Option<NeuralReranker> {
    let resolved_model = cli_m.or_else(|| {
        AppConfig::lm_model_search_paths()
            .into_iter()
            .find(|p| p.is_file())
    });

    let resolved_tokenizer = cli_t.or_else(|| {
        AppConfig::lm_tokenizer_search_paths()
            .into_iter()
            .find(|p| p.is_file())
    });

    match (resolved_model, resolved_tokenizer) {
        (Some(mp), Some(tp)) => {
            println!("[神经模型] 正在载入真实权重与分词器:");
            println!("  - 模型权重: {:?}", mp);
            println!("  - 词表分词: {:?}", tp);
            match NeuralReranker::load_from_files(&mp, &tp, None) {
                Ok(r) => Some(r),
                Err(e) => {
                    println!("[警告] 加载模型权重失败: {}", e);
                    None
                }
            }
        }
        _ => None,
    }
}

/// 构造轻量合成 Mock 重排序器（用于在没有真实权重时进行对照管线验证）
fn create_synthetic_mock_reranker() -> NeuralReranker {
    let config = WayLanLMConfig {
        vocab_size: 6400,
        hidden_size: 64,
        num_hidden_layers: 2,
        num_attention_heads: 4,
        num_key_value_heads: 2,
        head_dim: 16,
        intermediate_size: 128,
        max_position_embeddings: 128,
        ..Default::default()
    };

    let embed = Tensor2D::new(
        config.vocab_size,
        config.hidden_size,
        vec![0.01; config.vocab_size * config.hidden_size],
    )
    .unwrap();

    let mut layers = Vec::new();
    for _ in 0..config.num_hidden_layers {
        let q_dim = config.num_attention_heads * config.head_dim;
        let kv_dim = config.num_key_value_heads * config.head_dim;
        layers.push(LayerWeights {
            input_norm: vec![1.0; config.hidden_size],
            post_attention_norm: vec![1.0; config.hidden_size],
            q_proj: Tensor2D::new(
                q_dim,
                config.hidden_size,
                vec![0.01; q_dim * config.hidden_size],
            )
            .unwrap(),
            k_proj: Tensor2D::new(
                kv_dim,
                config.hidden_size,
                vec![0.01; kv_dim * config.hidden_size],
            )
            .unwrap(),
            v_proj: Tensor2D::new(
                kv_dim,
                config.hidden_size,
                vec![0.01; kv_dim * config.hidden_size],
            )
            .unwrap(),
            o_proj: Tensor2D::new(
                config.hidden_size,
                q_dim,
                vec![0.01; config.hidden_size * q_dim],
            )
            .unwrap(),
            gate_proj: Tensor2D::new(
                config.intermediate_size,
                config.hidden_size,
                vec![0.01; config.intermediate_size * config.hidden_size],
            )
            .unwrap(),
            up_proj: Tensor2D::new(
                config.intermediate_size,
                config.hidden_size,
                vec![0.01; config.intermediate_size * config.hidden_size],
            )
            .unwrap(),
            down_proj: Tensor2D::new(
                config.hidden_size,
                config.intermediate_size,
                vec![0.01; config.hidden_size * config.intermediate_size],
            )
            .unwrap(),
        });
    }

    let weights = ModelWeights {
        embed_tokens: embed,
        layers,
        final_norm: vec![1.0; config.hidden_size],
        lm_head: None,
    };
    let model = WayLanLMModel::new(config, weights);

    let minimal_json = r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[{"id":0,"content":"<pad>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":1,"content":"<bos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":2,"content":"<eos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true}],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,"vocab":{"<pad>":0,"<bos>":1,"<eos>":2},"merges":[]}}"#;
    let tokenizer = LMTokenizer::from_bytes(minimal_json.as_bytes()).unwrap();

    NeuralReranker {
        model,
        tokenizer,
        alpha: 0.35,
    }
}

/// 长度分层命中桶
#[derive(Default, Clone, Copy)]
struct AccuracyBucket {
    total: usize,
    hits: usize,
}

impl AccuracyBucket {
    fn record(&mut self, is_hit: bool) {
        self.total += 1;
        if is_hit {
            self.hits += 1;
        }
    }

    fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.hits as f64 / self.total as f64) * 100.0
        }
    }
}

/// 单个批次统计结果
#[derive(Default, Clone)]
struct BatchMetrics {
    total_samples: usize,
    correct_hits: usize,
    total_target_chars: usize,
    correct_chars: usize,
    total_latency_us: u128,
    bucket_short: AccuracyBucket,
    bucket_medium: AccuracyBucket,
    bucket_long: AccuracyBucket,
    error_cases: Vec<(String, String, String)>,
}

impl BatchMetrics {
    fn merge(&mut self, other: BatchMetrics) {
        self.total_samples += other.total_samples;
        self.correct_hits += other.correct_hits;
        self.total_target_chars += other.total_target_chars;
        self.correct_chars += other.correct_chars;
        self.total_latency_us += other.total_latency_us;
        self.bucket_short.total += other.bucket_short.total;
        self.bucket_short.hits += other.bucket_short.hits;
        self.bucket_medium.total += other.bucket_medium.total;
        self.bucket_medium.hits += other.bucket_medium.hits;
        self.bucket_long.total += other.bucket_long.total;
        self.bucket_long.hits += other.bucket_long.hits;
        if self.error_cases.len() < 10 {
            for err in other.error_cases {
                if self.error_cases.len() < 10 {
                    self.error_cases.push(err);
                }
            }
        }
    }
}

/// 纯词频基线评测执行器（支持多线程并行并发解码）
fn run_baseline_evaluation(dataset: &[SampleEntry], explicit_split: bool, threads: usize) {
    let total_samples = dataset.len();
    let chunk_size = (total_samples / (threads * 4)).clamp(50, 1000);

    let eval_start = Instant::now();
    println!(
        "正在对 {} 条样本执行真实输入法解码 (并发线程: {})...",
        total_samples, threads
    );

    let metrics_list: Vec<BatchMetrics> = dataset
        .par_chunks(chunk_size)
        .map(|chunk| {
            let mut engine = load_evaluation_engine();
            engine.set_reranker(Arc::new(NoopReranker));
            let mut batch = BatchMetrics::default();

            for sample in chunk {
                let t0 = Instant::now();
                let first_candidate =
                    decode_pinyin(&mut engine, &sample.pinyin, explicit_split).unwrap_or_default();
                let elapsed = t0.elapsed().as_micros();

                batch.total_latency_us += elapsed;
                batch.total_samples += 1;

                let target_chars: Vec<char> = sample.target.chars().collect();
                let pred_chars: Vec<char> = first_candidate.chars().collect();
                let char_len = target_chars.len();
                batch.total_target_chars += char_len;

                let mut matched = 0;
                for (i, &ch) in target_chars.iter().enumerate() {
                    if i < pred_chars.len() && pred_chars[i] == ch {
                        matched += 1;
                    }
                }
                batch.correct_chars += matched;

                let is_hit = first_candidate == sample.target;
                if is_hit {
                    batch.correct_hits += 1;
                } else if batch.error_cases.len() < 10 {
                    batch.error_cases.push((
                        sample.target.clone(),
                        first_candidate,
                        sample.pinyin.clone(),
                    ));
                }

                match char_len {
                    2..=4 => batch.bucket_short.record(is_hit),
                    5..=8 => batch.bucket_medium.record(is_hit),
                    _ => batch.bucket_long.record(is_hit),
                }
            }
            batch
        })
        .collect();

    let mut final_metrics = BatchMetrics::default();
    for m in metrics_list {
        final_metrics.merge(m);
    }

    let total_time_sec = eval_start.elapsed().as_secs_f64();
    let final_sent_acc = (final_metrics.correct_hits as f64 / total_samples as f64) * 100.0;
    let final_char_acc =
        (final_metrics.correct_chars as f64 / final_metrics.total_target_chars as f64) * 100.0;
    let avg_latency_ms = (final_metrics.total_latency_us as f64) / (total_samples as f64 * 1000.0);
    let qps = total_samples as f64 / total_time_sec;

    println!("\n============================================================");
    println!("                  纯词频基线评测结果报告");
    println!("============================================================");
    println!(
        "  - 评测样本总数:       {} 条 (总汉字数: {} 字)",
        total_samples, final_metrics.total_target_chars
    );
    println!(
        "  - 短词短语 (2~4字):   {:>5.2}% ({:>5}/{:>5}) [高频日常核心词]",
        final_metrics.bucket_short.rate(),
        final_metrics.bucket_short.hits,
        final_metrics.bucket_short.total
    );
    println!(
        "  - 中等短句 (5~8字):   {:>5.2}% ({:>5}/{:>5}) [常规日常整句]",
        final_metrics.bucket_medium.rate(),
        final_metrics.bucket_medium.hits,
        final_metrics.bucket_medium.total
    );
    println!(
        "  - 连续长句 (9~18字):  {:>5.2}% ({:>5}/{:>5}) [复杂连贯长句]",
        final_metrics.bucket_long.rate(),
        final_metrics.bucket_long.hits,
        final_metrics.bucket_long.total
    );
    println!("  ----------------------------------------------------------");
    println!(
        "  - 全量整句 Top-1 率:  {:>5.2}% ({:>5}/{:>5}) [严苛整句全对]",
        final_sent_acc, final_metrics.correct_hits, total_samples
    );
    println!(
        "  - 单字平均命中率:     {:>5.2}% ({:>5}/{:>5}) [单字打对比例]",
        final_char_acc, final_metrics.correct_chars, final_metrics.total_target_chars
    );
    println!("  - 单核单句解码延迟:   {:.3} ms", avg_latency_ms);
    println!("  - 多核并发总吞吐量:   {:.1} 句/秒", qps);
    println!("  - 总运行时间:         {:.2} 秒", total_time_sec);
    println!("============================================================");

    if !final_metrics.error_cases.is_empty() {
        println!("\n[典型打错案例深度抽检分析 (整句未完全匹配)]: ");
        for (i, (target, pred, py)) in final_metrics.error_cases.iter().enumerate() {
            println!("  [{:>2}] 期望目标: \"{}\"", i + 1, target);
            println!("       输入法出: \"{}\"", pred);
            println!("       输入拼音: {}", py);
            println!();
        }
    }
}

/// 双轨对比批次结果
#[derive(Default, Clone)]
struct ComparativeMetrics {
    baseline_hits: usize,
    neural_hits: usize,
    corrected_cases: Vec<(String, String, String)>,
    degraded_cases: Vec<(String, String, String)>,
}

impl ComparativeMetrics {
    fn merge(&mut self, other: ComparativeMetrics) {
        self.baseline_hits += other.baseline_hits;
        self.neural_hits += other.neural_hits;
        if self.corrected_cases.len() < 5 {
            for c in other.corrected_cases {
                if self.corrected_cases.len() < 5 {
                    self.corrected_cases.push(c);
                }
            }
        }
        if self.degraded_cases.len() < 5 {
            for d in other.degraded_cases {
                if self.degraded_cases.len() < 5 {
                    self.degraded_cases.push(d);
                }
            }
        }
    }
}

/// 纯基线 vs 神经模型对比评测执行器（支持多线程并发）
fn run_comparative_evaluation(
    reranker: NeuralReranker,
    dataset: &[SampleEntry],
    explicit_split: bool,
    threads: usize,
) {
    let total_samples = dataset.len();
    let reranker_arc: Arc<dyn SentenceReranker> = Arc::new(reranker);
    let chunk_size = (total_samples / (threads * 4)).clamp(20, 500);

    println!(
        "正在对 {} 条样本执行双轨解码对比评测 (并发线程: {})...",
        total_samples, threads
    );
    let eval_start = Instant::now();

    let metrics_list: Vec<ComparativeMetrics> = dataset
        .par_chunks(chunk_size)
        .map(|chunk| {
            let mut engine = load_evaluation_engine();
            let mut batch = ComparativeMetrics::default();

            for sample in chunk {
                // 1. 基线
                engine.set_reranker(Arc::new(NoopReranker));
                let base_res =
                    decode_pinyin(&mut engine, &sample.pinyin, explicit_split).unwrap_or_default();
                let base_correct = base_res == sample.target;
                if base_correct {
                    batch.baseline_hits += 1;
                }

                // 2. 神经重排
                engine.set_reranker(reranker_arc.clone());
                let neural_res =
                    decode_pinyin(&mut engine, &sample.pinyin, explicit_split).unwrap_or_default();
                let neural_correct = neural_res == sample.target;
                if neural_correct {
                    batch.neural_hits += 1;
                }

                if !base_correct && neural_correct && batch.corrected_cases.len() < 5 {
                    batch.corrected_cases.push((
                        sample.target.clone(),
                        base_res.clone(),
                        neural_res.clone(),
                    ));
                } else if base_correct && !neural_correct && batch.degraded_cases.len() < 5 {
                    batch.degraded_cases.push((
                        sample.target.clone(),
                        base_res.clone(),
                        neural_res.clone(),
                    ));
                }
            }
            batch
        })
        .collect();

    let mut final_metrics = ComparativeMetrics::default();
    for m in metrics_list {
        final_metrics.merge(m);
    }

    let total_time_sec = eval_start.elapsed().as_secs_f64();
    let b_acc = (final_metrics.baseline_hits as f64 / total_samples as f64) * 100.0;
    let n_acc = (final_metrics.neural_hits as f64 / total_samples as f64) * 100.0;
    let qps = total_samples as f64 / total_time_sec;

    println!("\n============================================================");
    println!("                 双轨对比评测统计报告");
    println!("============================================================");
    println!("  - 评测样本总数:       {} 条", total_samples);
    println!(
        "  - 纯词频基线正确率:   {:.2}% ({}/{})",
        b_acc, final_metrics.baseline_hits, total_samples
    );
    println!(
        "  - 神经语言模型正确率: {:.2}% ({}/{})",
        n_acc, final_metrics.neural_hits, total_samples
    );
    println!("  - 净准确率提升幅度:   {:+.2}%", n_acc - b_acc);
    println!("  - 整体处理吞吐量:     {:.1} 句/秒", qps);
    println!("  - 总运行时间:         {:.2} 秒", total_time_sec);
    println!("============================================================");

    if !final_metrics.corrected_cases.is_empty() {
        println!("\n[典型成功纠偏案例 (原本打错，模型纠正)]: ");
        for (target, base, neural) in &final_metrics.corrected_cases {
            println!("  - 目标: \"{}\"", target);
            println!("    基线打错 -> \"{}\"", base);
            println!("    模型纠正 -> \"{}\"", neural);
        }
    }

    if !final_metrics.degraded_cases.is_empty() {
        println!("\n[典型退化案例 (原本打对，模型误伤)]: ");
        for (target, base, neural) in &final_metrics.degraded_cases {
            println!("  - 目标: \"{}\"", target);
            println!("    基线打对 -> \"{}\"", base);
            println!("    模型误选 -> \"{}\"", neural);
        }
    }
    println!();
}

fn main() {
    let opts = parse_options();

    // 配置 Rayon 全局并发线程池
    let threads = opts.threads.unwrap_or_else(rayon::current_num_threads);
    if let Some(t) = opts.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()
            .ok();
    }

    println!("============================================================");
    println!("     微澜 (WayLan) 工业级中文拼音端到端准召率评测系统");
    println!("============================================================");

    let (dataset, dataset_desc) = load_dataset(opts.dataset_path.as_deref(), opts.limit);
    println!("[评测语料] {}", dataset_desc);
    println!(
        "[输入模式] {}",
        if opts.explicit_split {
            "保留隔音符 (') 显式切分"
        } else {
            "纯字母连续流 (模拟用户盲打)"
        }
    );
    println!("[并发核心] 启用 {} 个线程并发解码", threads);

    if opts.baseline_only {
        println!("[评测模式] 纯词频基线测试模式 (--baseline-only)");
        println!("------------------------------------------------------------");
        run_baseline_evaluation(&dataset, opts.explicit_split, threads);
        return;
    }

    // 对比模式：构建或载入神经重排序器
    let neural_opt = resolve_neural_reranker(opts.model_path, opts.tokenizer_path);
    let (reranker, is_mock) = match neural_opt {
        Some(r) => (r, false),
        None => {
            println!("[提示] 未在系统中检测到真实模型，使用合成基准模型进行对比流程校验。");
            println!("       训练完成后可通过 --model 和 --tokenizer 指定真实权重文件。\n");
            (create_synthetic_mock_reranker(), true)
        }
    };

    println!("[评测模式] 纯词频基线 VS 神经网络重排序 全量对比模式");
    if is_mock {
        println!("[模型状态] 合成轻量对比模型");
    } else {
        println!("[模型状态] 真实 16M 权重模型已就绪");
    }
    println!("------------------------------------------------------------");

    run_comparative_evaluation(reranker, &dataset, opts.explicit_split, threads);
}
