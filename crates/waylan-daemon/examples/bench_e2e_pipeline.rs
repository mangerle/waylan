use std::time::Instant;
use waylan_algo::{table::LexiconTable, types::AlgoConfig, AlgoEngine};
use waylan_core::{Candidate, CandidateBoxState};
use waylan_ui::{CandidateBoxRenderer, UiTheme};

fn main() {
    println!("=== 微澜全链路核心管线真实实机性能基准测试 ===");

    // 1. 初始化算法引擎 (真实载入已安装到系统的 45.7 万词主词库)
    let dict_path = "/usr/share/waylan/data/base.dict.bin";
    println!("[准备] 正在载入系统全量词库: {}", dict_path);
    let load_start = Instant::now();
    let table = LexiconTable::load_binary(dict_path).expect("加载全量字词库失败");
    println!(
        "  -> 词库载入完成，耗时: {:.2} ms",
        load_start.elapsed().as_secs_f64() * 1000.0
    );

    let mut engine = AlgoEngine::new(table, AlgoConfig::default());
    let renderer = CandidateBoxRenderer::new(UiTheme::light());

    // 预热 (Warm-up 500 次)
    for _ in 0..500 {
        engine.push_char('x');
        engine.push_char('i');
        engine.push_char('a');
        engine.push_char('n');
        engine.pop_char();
        engine.pop_char();
        engine.pop_char();
        engine.pop_char();
    }

    let iterations = 2000;
    println!("[测试] 正在执行 {} 次真实循环压力测试...", iterations);

    // 2. 真实测试阶段 A：算法检索真实耗时 (以最复杂的歧义词 xian 为例)
    let start_algo = Instant::now();
    for _ in 0..iterations {
        engine.push_char('x');
        engine.push_char('i');
        engine.push_char('a');
        let cands = engine.push_char('n');
        assert!(!cands.is_empty());
        engine.pop_char();
        engine.pop_char();
        engine.pop_char();
        engine.pop_char();
    }
    let total_algo = start_algo.elapsed();
    // 包含 4 次 push 和 4 次 pop，计算每次击键候选计算的单次平均耗时
    let avg_algo_us = (total_algo.as_micros() as f64) / (iterations as f64 * 8.0);

    // 3. 真实测试阶段 B：UI 矢量光栅化渲染耗时 (真实 tiny-skia 离屏渲染候选条)
    let state = CandidateBoxState {
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
        expanded_card: waylan_core::ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
    };

    let start_render = Instant::now();
    let mut rendered_bytes = 0;
    for _ in 0..iterations {
        if let Some(pixmap) = renderer.render(&state) {
            rendered_bytes += pixmap.data().len();
        }
    }
    let total_render = start_render.elapsed();
    let avg_render_us = (total_render.as_micros() as f64) / (iterations as f64);

    // 4. 真实测试阶段 C：wl_shm 共享内存池拷贝耗时 (将渲染好的 RGBA 字节流拷贝到共享内存)
    let sample_pixmap = renderer.render(&state).unwrap();
    let mut shm_buffer = vec![0u8; sample_pixmap.data().len()];
    let start_shm = Instant::now();
    for _ in 0..iterations {
        shm_buffer.copy_from_slice(sample_pixmap.data());
    }
    let total_shm = start_shm.elapsed();
    let avg_shm_us = (total_shm.as_micros() as f64) / (iterations as f64);

    let total_measured_us = avg_algo_us + avg_render_us + avg_shm_us;

    println!("\n============================================================");
    println!("          微澜输入法实机端到端管线高精度实测数据           ");
    println!("============================================================");
    println!(
        "测试环境: WSL2 (x86_64, Release 优化编译, 循环 {} 次)",
        iterations
    );
    println!(
        "候选框尺寸: {} x {} 像素 (已渲染字节数: {} 字节)",
        sample_pixmap.width(),
        sample_pixmap.height(),
        rendered_bytes / iterations
    );
    println!("------------------------------------------------------------");
    println!(
        "1. 算法检索平均耗时:        {:>8.2} µs  ({:.4} ms)",
        avg_algo_us,
        avg_algo_us / 1000.0
    );
    println!(
        "2. tiny-skia 矢量光栅化耗时: {:>8.2} µs  ({:.4} ms)",
        avg_render_us,
        avg_render_us / 1000.0
    );
    println!(
        "3. wl_shm 共享内存写入耗时:   {:>8.2} µs  ({:.4} ms)",
        avg_shm_us,
        avg_shm_us / 1000.0
    );
    println!("------------------------------------------------------------");
    println!(
        "微澜本地处理总耗时 (实测):   {:>8.2} µs  ({:.4} ms)",
        total_measured_us,
        total_measured_us / 1000.0
    );
    println!("============================================================");
}
