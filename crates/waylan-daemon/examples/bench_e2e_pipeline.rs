use std::time::Instant;
use waylan_algo::{table::LexiconTable, types::AlgoConfig, AlgoEngine};
use waylan_core::{Candidate, CandidateBoxState};
use waylan_ui::{CandidateBoxRenderer, UiTheme};

/// 正式压测循环次数
const ITERATIONS: usize = 2000;
/// 预热循环次数（摊销首次调用的冷启动开销）
const WARMUP_ITERATIONS: usize = 500;

/// 端到端管线基准测试实测结果汇总
struct BenchReport {
    /// 正式压测循环次数
    iterations: usize,
    /// 算法检索单次平均耗时（微秒）
    avg_algo_us: f64,
    /// 矢量光栅化单次平均耗时（微秒）
    avg_render_us: f64,
    /// 共享内存写入单次平均耗时（微秒）
    avg_shm_us: f64,
    /// 单次渲染产出的位图字节数
    rendered_bytes_per_iter: usize,
    /// 候选框位图尺寸（宽，高）
    pixmap_size: (u32, u32),
}

/// 在多级候选路径中查找并载入系统全量主词库
fn load_system_lexicon() -> Option<LexiconTable> {
    for p in waylan_daemon::config::AppConfig::base_dict_search_paths() {
        if !p.exists() {
            continue;
        }
        println!("[准备] 正在载入系统全量词库: {:?}", p);
        let load_start = Instant::now();
        if let Ok(table) = LexiconTable::load_binary(p) {
            println!(
                "  -> 词库载入完成，耗时: {:.2} ms",
                load_start.elapsed().as_secs_f64() * 1000.0
            );
            return Some(table);
        }
    }
    None
}

/// 构造压测所用算法引擎（优先系统全量词库，缺失时回退示例内置词库）
fn build_engine() -> AlgoEngine {
    match load_system_lexicon() {
        Some(table) => AlgoEngine::new(table, AlgoConfig::default()),
        None => {
            println!("[提示] 未在系统中找到 base.dict.bin，使用示例内置词库进行端到端性能压测。");
            AlgoEngine::with_sample_data(AlgoConfig::default())
        }
    }
}

/// 连续回退四个已输入的击键
fn pop_four_chars(engine: &mut AlgoEngine) {
    engine.pop_char();
    engine.pop_char();
    engine.pop_char();
    engine.pop_char();
}

/// 预热算法引擎以摊销首次调用的冷启动开销
fn warm_up(engine: &mut AlgoEngine) {
    for _ in 0..WARMUP_ITERATIONS {
        engine.push_char('x');
        engine.push_char('i');
        engine.push_char('a');
        engine.push_char('n');
        pop_four_chars(engine);
    }
}

/// 测量算法检索真实耗时（以最复杂的歧义词 xian 为例）
fn benchmark_algo(engine: &mut AlgoEngine, iterations: usize) -> f64 {
    let start_algo = Instant::now();
    for _ in 0..iterations {
        engine.push_char('x');
        engine.push_char('i');
        engine.push_char('a');
        let cands = engine.push_char('n');
        assert!(!cands.is_empty());
        pop_four_chars(engine);
    }
    let total_algo = start_algo.elapsed();
    // 包含 4 次 push 和 4 次 pop，计算每次击键候选计算的单次平均耗时
    (total_algo.as_micros() as f64) / (iterations as f64 * 8.0)
}

/// 构造用于渲染压测的候选框状态
fn build_benchmark_state() -> CandidateBoxState {
    CandidateBoxState {
        visible: true,
        preedit: "xian".to_string(),
        candidates: vec![
            Candidate::new("先", "xian", 10000),
            Candidate::new("现", "xian", 9000),
            Candidate::new("显", "xian", 8000),
            Candidate::new("西安", "xi'an", 7000),
            Candidate::new("西岸", "xi'an", 6000),
            Candidate::new("锡安", "xi'an", 5000),
            Candidate::new("线", "xian", 4000),
        ],
        selected_index: 0,
        expansion: waylan_core::CandidateExpansion::None,
        mode_badge: "中".to_string(),
        grid_candidates: Vec::new(),
        voice: None,
    }
}

/// 测量 UI 矢量光栅化渲染耗时，返回 (单次平均微秒数, 累计渲染字节数)
fn benchmark_render(
    renderer: &mut CandidateBoxRenderer,
    state: &CandidateBoxState,
    iterations: usize,
) -> (f64, usize) {
    let start_render = Instant::now();
    let mut rendered_bytes = 0usize;
    for _ in 0..iterations {
        if let Some(pixmap) = renderer.render(state) {
            rendered_bytes += pixmap.data().len();
        }
    }
    let total_render = start_render.elapsed();
    let avg_render_us = (total_render.as_micros() as f64) / (iterations as f64);
    (avg_render_us, rendered_bytes)
}

/// 测量 wl_shm 共享内存池拷贝耗时，返回 (单次平均微秒数, 位图宽高)
fn benchmark_shm(
    renderer: &mut CandidateBoxRenderer,
    state: &CandidateBoxState,
    iterations: usize,
) -> Option<(f64, (u32, u32))> {
    let sample_pixmap = renderer.render(state)?;
    let mut shm_buffer = vec![0u8; sample_pixmap.data().len()];
    let start_shm = Instant::now();
    for _ in 0..iterations {
        shm_buffer.copy_from_slice(sample_pixmap.data());
    }
    let total_shm = start_shm.elapsed();
    let avg_shm_us = (total_shm.as_micros() as f64) / (iterations as f64);
    Some((avg_shm_us, (sample_pixmap.width(), sample_pixmap.height())))
}

/// 打印端到端管线高精度实测报告
fn print_report(report: &BenchReport) {
    let total_measured_us = report.avg_algo_us + report.avg_render_us + report.avg_shm_us;
    println!("\n============================================================");
    println!("          微澜输入法实机端到端管线高精度实测数据           ");
    println!("============================================================");
    println!(
        "测试环境: WSL2 (x86_64, Release 优化编译, 循环 {} 次)",
        report.iterations
    );
    println!(
        "候选框尺寸: {} x {} 像素 (已渲染字节数: {} 字节)",
        report.pixmap_size.0, report.pixmap_size.1, report.rendered_bytes_per_iter
    );
    println!("------------------------------------------------------------");
    println!(
        "1. 算法检索平均耗时:        {:>8.2} µs  ({:.4} ms)",
        report.avg_algo_us,
        report.avg_algo_us / 1000.0
    );
    println!(
        "2. tiny-skia 矢量光栅化耗时: {:>8.2} µs  ({:.4} ms)",
        report.avg_render_us,
        report.avg_render_us / 1000.0
    );
    println!(
        "3. wl_shm 共享内存写入耗时:   {:>8.2} µs  ({:.4} ms)",
        report.avg_shm_us,
        report.avg_shm_us / 1000.0
    );
    println!("------------------------------------------------------------");
    println!(
        "微澜本地处理总耗时 (实测):   {:>8.2} µs  ({:.4} ms)",
        total_measured_us,
        total_measured_us / 1000.0
    );
    println!("============================================================");
}

fn main() {
    println!("=== 微澜全链路核心管线真实实机性能基准测试 ===");

    // 1. 初始化算法引擎与候选框渲染器
    let mut engine = build_engine();
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());

    // 2. 预热引擎，排除冷启动开销干扰
    warm_up(&mut engine);

    println!("[测试] 正在执行 {} 次真实循环压力测试...", ITERATIONS);

    // 3. 测量算法检索耗时
    let avg_algo_us = benchmark_algo(&mut engine, ITERATIONS);

    // 4. 测量矢量光栅化与共享内存写入耗时
    let state = build_benchmark_state();
    let (avg_render_us, rendered_bytes) = benchmark_render(&mut renderer, &state, ITERATIONS);
    let Some((avg_shm_us, pixmap_size)) = benchmark_shm(&mut renderer, &state, ITERATIONS) else {
        println!("[错误] 候选框渲染失败，无法完成共享内存写入耗时测量。");
        return;
    };

    print_report(&BenchReport {
        iterations: ITERATIONS,
        avg_algo_us,
        avg_render_us,
        avg_shm_us,
        rendered_bytes_per_iter: rendered_bytes / ITERATIONS,
        pixmap_size,
    });
}
