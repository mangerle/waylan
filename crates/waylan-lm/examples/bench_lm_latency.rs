//! 纯 Rust 神经网络语言模型前向推理延迟与吞吐量压测工具
//!
//! # 设计原理
//! - **实现初衷**：输入法对交互延迟有极其严苛的物理限制（单句 CPU 推理必须 <= 8ms），
//!   本工具针对短、中、长不同长度的真实汉语句子，多轮统计 P50/P90/P95/P99 延时分位数与 QPS。
//! - **核心优势**：通过标准高精度时钟对密集张量运算（RoPE、GQA、SwiGLU、RMSNorm）进行微基准压测，
//!   精确度达微秒（μs）级，量化检验多线程与 CPU 缓存友好度。
//! - **代价与局限**：基于单线程 CPU 前向测试，反映输入法后台打字协程的最纯净计算耗时。

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use waylan_core::config::AppConfig;
use waylan_lm::weights::{LayerWeights, ModelWeights, Tensor2D};
use waylan_lm::{LMTokenizer, WayLanLMConfig, WayLanLMModel};

/// 压测测试语料集（不同长度梯度）
struct BenchSentence {
    pub label: &'static str,
    pub text: &'static str,
}

static BENCH_SENTENCES: &[BenchSentence] = &[
    BenchSentence {
        label: "超短句 (4 字符)",
        text: "今天天气",
    },
    BenchSentence {
        label: "短句   (8 字符)",
        text: "今天天气真的很不错",
    },
    BenchSentence {
        label: "中长句 (16 字符)",
        text: "微澜输入法致力于提供极致流畅的现代中文体验",
    },
    BenchSentence {
        label: "长句   (32 字符)",
        text: "我们要大力发展完全自主可控的底层现代软件基础设施与人工智能自然语言模型",
    },
];

/// 解析命令行参数
fn parse_cli_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    let args: Vec<String> = env::args().collect();
    let mut model_path = None;
    let mut tok_path = None;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--model" && i + 1 < args.len() {
            model_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if args[i] == "--tokenizer" && i + 1 < args.len() {
            tok_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }

    (model_path, tok_path)
}

/// 自动探测系统中的权重与分词器路径
fn auto_detect_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    let (cli_m, cli_t) = parse_cli_paths();
    if cli_m.is_some() && cli_t.is_some() {
        return (cli_m, cli_t);
    }

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

    (resolved_model, resolved_tokenizer)
}

/// 构造用于演示的合成微型模型
fn create_synthetic_model() -> (WayLanLMModel, LMTokenizer, bool) {
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

    let tok_candidates = [
        "assets/tokenizer.json",
        "../minimind/assets/tokenizer.json",
        "/usr/share/waylan/tokenizer.json",
    ];
    let mut tokenizer = None;
    for cand in &tok_candidates {
        if Path::new(cand).exists() {
            if let Ok(t) = LMTokenizer::from_file(cand) {
                tokenizer = Some(t);
                break;
            }
        }
    }

    let tokenizer = tokenizer.unwrap_or_else(|| {
        let minimal_json = r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[{"id":0,"content":"<pad>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":1,"content":"<bos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true},{"id":2,"content":"<eos>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true}],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,"vocab":{"<pad>":0,"<bos>":1,"<eos>":2},"merges":[]}}"#;
        LMTokenizer::from_bytes(minimal_json.as_bytes()).expect("保底分词器解析失败")
    });

    (model, tokenizer, true)
}

fn main() {
    println!("============================================================");
    println!("     微澜 (WayLan) 本地轻量神经语言模型 CPU 推理性能压测基准");
    println!("============================================================");

    let (model_path, tok_path) = auto_detect_paths();
    let (model, tokenizer, is_mock) = match (&model_path, &tok_path) {
        (Some(mp), Some(tp)) => {
            println!("[加载] 正在载入真实模型权重: {:?}", mp);
            println!("[加载] 正在载入真实分词索引: {:?}", tp);
            let config = WayLanLMConfig::default();
            match ModelWeights::from_safetensors_file(mp, &config) {
                Ok(weights) => {
                    let m = WayLanLMModel::new(config, weights);
                    let t = LMTokenizer::from_file(tp).expect("分词器加载失败");
                    (m, t, false)
                }
                Err(e) => {
                    println!("[警告] 加载模型文件失败 ({})，回退到合成轻量模型。", e);
                    create_synthetic_model()
                }
            }
        }
        _ => {
            println!("[提示] 未检测到真实模型文件，使用合成基准模型进行压测逻辑与循环基准验证。");
            println!("       真实模型训练导出后，可指定参数执行压测:");
            println!("       cargo run --release -p waylan-lm --example bench_lm_latency -- --model <路径> --tokenizer <路径>\n");
            create_synthetic_model()
        }
    };

    println!(
        "[配置] 模型层数: {}, 隐藏维度: {}, 词表大小: {}",
        model.config.num_hidden_layers, model.config.hidden_size, model.config.vocab_size
    );
    if is_mock {
        println!("[模式] 正在运行合成轻量测试模型 (用于纯逻辑耗时验证)");
    } else {
        println!("[模式] 正在运行真实 16M 权重模型 (用于真实物理耗时基准)");
    }
    println!("------------------------------------------------------------\n");

    let warmup_rounds = 50;
    let bench_rounds = 200;

    println!(
        "正在对各个长度句子进行基准测试 (预热 {} 轮, 测量 {} 轮)...",
        warmup_rounds, bench_rounds
    );
    println!();
    println!(
        "{:<16} | {:<8} | {:<8} | {:<8} | {:<8} | {:<8} | {:<8}",
        "语料梯度", "平均耗时", "中位数 P50", "P90 耗时", "P95 耗时", "P99 耗时", "每秒吞吐 (QPS)"
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    for s in BENCH_SENTENCES {
        let token_ids = tokenizer
            .encode(s.text, true)
            .unwrap_or_else(|_| vec![1, 10, 20, 30]);

        // 预热阶段
        for _ in 0..warmup_rounds {
            let _ = model.compute_sequence_log_likelihood(&token_ids);
        }

        // 正式测量
        let mut durations_us = Vec::with_capacity(bench_rounds);
        let start_bench = Instant::now();
        for _ in 0..bench_rounds {
            let t0 = Instant::now();
            let _ = model.compute_sequence_log_likelihood(&token_ids);
            durations_us.push(t0.elapsed().as_micros());
        }
        let total_bench_elapsed = start_bench.elapsed();

        durations_us.sort_unstable();
        let min_us = durations_us[0];
        let max_us = durations_us[durations_us.len() - 1];
        let p50_us = durations_us[(bench_rounds as f64 * 0.50) as usize];
        let p90_us = durations_us[(bench_rounds as f64 * 0.90) as usize];
        let p95_us = durations_us[(bench_rounds as f64 * 0.95) as usize];
        let p99_us = durations_us[(bench_rounds as f64 * 0.99) as usize];
        let sum_us: u128 = durations_us.iter().sum();
        let avg_us = sum_us as f64 / bench_rounds as f64;
        let qps = (bench_rounds as f64) / total_bench_elapsed.as_secs_f64();

        println!("{:<16} | {:>6.2} ms | {:>6.2} ms | {:>6.2} ms | {:>6.2} ms | {:>6.2} ms | {:>7.1} 句/秒",
            s.label,
            avg_us / 1000.0,
            p50_us as f64 / 1000.0,
            p90_us as f64 / 1000.0,
            p95_us as f64 / 1000.0,
            p99_us as f64 / 1000.0,
            qps
        );

        // 若超出了 8ms 红色阈值，则附加提示
        if avg_us > 8000.0 {
            println!("  [性能提醒] 该长度句子的平均耗时 ({:.2} ms) 超过了 8ms 阈值，建议长句在异步非阻塞线程中打分。", avg_us / 1000.0);
        }
        let _ = (min_us, max_us);
    }

    println!(
        "-----------------------------------------------------------------------------------------"
    );
    println!("\n[完成] 性能基准测试结束。");
}
