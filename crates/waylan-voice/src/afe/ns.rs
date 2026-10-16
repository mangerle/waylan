//! 人声智能降噪引擎 (Noise Suppression, NS) 模块
//!
//! # 设计原理
//! - **实现初衷**：消除机械键盘/鼠标突发敲击声、机箱风扇白噪音以及空调气流声，从源头避免 VAD 误触发与 ASR 吐拟声杂字。
//! - **核心优势**：
//!   1. 内置纯 Rust 版 RNNoise (`nnnoiseless`)，静态集成，零外部 C 库依赖，零动态链接，全平台开箱即用；
//!   2. 提供标准抽象 Trait `NoiseSuppressor`，支持 DeepFilterNet 与第三方 ONNX 深度模型热插拔扩展；
//!   3. 内建 16kHz <-> 48kHz 整数倍线性插值重采样状态机，自动适配神经网络最佳工作频段。
//! - **代价与局限**：因流式帧长对齐（10ms），初次启动会引入约 10ms（160 采样点）的极微小因果缓冲延迟。

use log::{info, warn};
use waylan_core::{AudioFrontEndConfig, NsEngineType};

/// 人声降噪引擎统一 Trait 抽象
pub trait NoiseSuppressor: Send {
    /// 就地对 16kHz 单声道 PCM 浮点音频切片执行智能人声降噪
    fn process(&mut self, samples: &mut [f32]);

    /// 重置降噪内部循环记忆状态
    fn reset(&mut self);
}

/// 直通模式降噪器（Bypass，不执行任何处理）
#[derive(Debug, Clone, Default)]
pub struct BypassSuppressor;

impl BypassSuppressor {
    pub fn new() -> Self {
        Self
    }
}

impl NoiseSuppressor for BypassSuppressor {
    fn process(&mut self, _samples: &mut [f32]) {
        // 直通不作修改
    }

    fn reset(&mut self) {}
}

/// 内置纯 Rust RNNoise 降噪器（基于 `nnnoiseless`）
pub struct RnnoiseSuppressor {
    /// 神经网络状态单例
    denoiser: Box<nnnoiseless::DenoiseState<'static>>,
    /// 上一个 16kHz 采样点，用于 1:3 线性插值
    last_16k_sample: f32,
    /// 累积待送入 48kHz 降噪网络的缓冲区（满 480 点处理一帧）
    in_48k_buf: Vec<f32>,
    /// 降噪完毕已转换回 16kHz 的暂存队列
    out_16k_queue: Vec<f32>,
    /// 用于送入 process_frame 的输入帧缓冲 (480 点)
    frame_in: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
    /// 用于接收 process_frame 的输出帧缓冲 (480 点)
    frame_out: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
}

impl RnnoiseSuppressor {
    /// 创建纯 Rust RNNoise 降噪器
    pub fn new() -> Self {
        let mut denoiser = nnnoiseless::DenoiseState::new();
        // 丢弃第一帧瞬态初始响应
        let dummy_in = [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let mut dummy_out = [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let _ = denoiser.process_frame(&mut dummy_out, &dummy_in);

        Self {
            denoiser,
            last_16k_sample: 0.0,
            in_48k_buf: Vec::with_capacity(1024),
            out_16k_queue: Vec::with_capacity(1024),
            frame_in: [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE],
            frame_out: [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE],
        }
    }

    /// 执行 16kHz 到 48kHz 的 3 倍线性插值上采样并追加至 in_48k_buf
    fn upsample_16k_to_48k(&mut self, input: &[f32]) {
        for &s in input {
            let s_prev = self.last_16k_sample;
            // 3 倍插值：s_prev + 1/3*(s - s_prev), s_prev + 2/3*(s - s_prev), s
            let delta = s - s_prev;
            self.in_48k_buf.push(s_prev + delta * (1.0 / 3.0));
            self.in_48k_buf.push(s_prev + delta * (2.0 / 3.0));
            self.in_48k_buf.push(s);
            self.last_16k_sample = s;
        }
    }

    /// 循环消化 48kHz 缓冲区中的整帧并降噪
    fn process_available_frames(&mut self) {
        let frame_size = nnnoiseless::DenoiseState::FRAME_SIZE;

        while self.in_48k_buf.len() >= frame_size {
            // 拷贝 480 个样本并放缩至 16-bit 浮点范围 [-32768, 32767]
            for (i, target) in self.frame_in.iter_mut().enumerate() {
                *target = self.in_48k_buf[i] * 32767.0;
            }

            // 神经网络帧级推理
            let _ = self
                .denoiser
                .process_frame(&mut self.frame_out, &self.frame_in);

            // 移除已消费的 480 个 48kHz 采样点
            self.in_48k_buf.drain(0..frame_size);

            // 3:1 抗混叠均值下采样回到 16kHz（480 点 -> 160 点）
            let out_16k_samples = frame_size / 3;
            for i in 0..out_16k_samples {
                let idx = i * 3;
                let sum = self.frame_out[idx] + self.frame_out[idx + 1] + self.frame_out[idx + 2];
                let downsampled = (sum / 3.0) / 32767.0;
                self.out_16k_queue.push(downsampled.clamp(-1.0, 1.0));
            }
        }
    }
}

impl Default for RnnoiseSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for RnnoiseSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        // 1. 线性插值上采样为 48kHz 并存入缓冲
        self.upsample_16k_to_48k(samples);

        // 2. 批量处理所有凑满 480 样本（10ms）的整帧
        self.process_available_frames();

        // 3. 从降噪完成队列写回对应长度样本
        let needed = samples.len();
        if self.out_16k_queue.len() >= needed {
            // 队列中有充足的降噪样本，按序写入
            samples.copy_from_slice(&self.out_16k_queue[..needed]);
            self.out_16k_queue.drain(0..needed);
        } else {
            // 初始冷启动期间队列尚未排满，将已有样本填入，不足部分以静音垫底，严禁残留原始样本拼接导致时域阶跃爆音
            let available = self.out_16k_queue.len();
            samples[..available].copy_from_slice(&self.out_16k_queue[..available]);
            samples[available..].fill(0.0);
            self.out_16k_queue.clear();
        }
    }

    fn reset(&mut self) {
        self.denoiser = nnnoiseless::DenoiseState::new();
        self.last_16k_sample = 0.0;
        self.in_48k_buf.clear();
        self.out_16k_queue.clear();
    }
}

/// 内置高阶深度级 DeepFilterNet3 降噪器（基于纯 Rust DeepFilterNet3 架构）
pub struct DeepFilterSuppressor {
    /// DeepFilterNet3 模型引擎单例
    hush: Option<hush_vani::Hush>,
    /// 累积待处理的 16kHz PCM 样本
    in_buf: Vec<f32>,
    /// 降噪完毕的样本队列
    out_queue: Vec<f32>,
}

impl DeepFilterSuppressor {
    /// 创建 DeepFilterNet3 深度降噪器实例
    pub fn new() -> Self {
        let hush = match hush_vani::Hush::new() {
            Ok(h) => {
                info!("[音频前端] 成功初始化系统内置 DeepFilterNet3 深度降噪神经网络引擎 (AVX2+FMA 自动加速)");
                Some(h)
            }
            Err(e) => {
                warn!("[音频前端] 初始化 DeepFilterNet3 引擎失败: {}，降级运行", e);
                None
            }
        };

        Self {
            hush,
            in_buf: Vec::with_capacity(1600),
            out_queue: Vec::with_capacity(1600),
        }
    }
}

impl Default for DeepFilterSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for DeepFilterSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let Some(ref hush) = self.hush else {
            return;
        };

        // 压入当前切片
        self.in_buf.extend_from_slice(samples);

        // 每次按 160 采样点（10ms 整帧）处理
        let frame_size = 160;
        let available_frames = self.in_buf.len() / frame_size;
        if available_frames > 0 {
            let process_len = available_frames * frame_size;
            let chunk = &self.in_buf[..process_len];
            if let Ok(cleaned) = hush.enhance(chunk) {
                self.out_queue.extend_from_slice(&cleaned);
            }
            self.in_buf.drain(..process_len);
        }

        // 从降噪队列写回 samples
        let needed = samples.len();
        if self.out_queue.len() >= needed {
            samples.copy_from_slice(&self.out_queue[..needed]);
            self.out_queue.drain(..needed);
        } else {
            // 初始冷启动期间队列尚未排满，将已有样本填入，不足部分以静音垫底，严禁残留原始样本拼接导致时域阶跃爆音
            let available = self.out_queue.len();
            samples[..available].copy_from_slice(&self.out_queue[..available]);
            samples[available..].fill(0.0);
            self.out_queue.clear();
        }
    }

    fn reset(&mut self) {
        self.in_buf.clear();
        self.out_queue.clear();
    }
}

/// 基于 sherpa-onnx 原生驱动的 SOTA 级 GTCRN 卷积循环深度降噪器 (523KB)
pub struct GtcrnSuppressor {
    denoiser: Option<sherpa_onnx::OnlineSpeechDenoiser>,
    in_buf: Vec<f32>,
    out_queue: Vec<f32>,
}

impl GtcrnSuppressor {
    /// 创建 GTCRN 降噪器实例
    pub fn new() -> Self {
        let model_path = resolve_gtcrn_model_path();
        let denoiser = if let Some(ref p) = model_path {
            let mut cfg = sherpa_onnx::OnlineSpeechDenoiserConfig::default();
            cfg.model.gtcrn.model = Some(p.to_string_lossy().to_string());
            cfg.model.num_threads = 1;
            cfg.model.provider = Some("cpu".to_string());
            match sherpa_onnx::OnlineSpeechDenoiser::create(&cfg) {
                Some(d) => {
                    info!("[音频前端] 成功初始化系统内置 GTCRN 深度降噪神经网络引擎 (523KB, 模型: {:?})", p);
                    Some(d)
                }
                None => {
                    warn!("[音频前端] 初始化 GTCRN 降噪模型失败，将回退为直通模式");
                    None
                }
            }
        } else {
            warn!("[音频前端] 未找到 GTCRN 降噪模型文件 (models/ns/gtcrn/gtcrn_simple.onnx)，将回退为直通模式");
            None
        };

        Self {
            denoiser,
            in_buf: Vec::with_capacity(1600),
            out_queue: Vec::with_capacity(1600),
        }
    }
}

impl Default for GtcrnSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for GtcrnSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let Some(ref denoiser) = self.denoiser else {
            return;
        };

        // 将切片送入流式推理，GTCRN 原生在 16kHz 单声道下工作
        let denoised = denoiser.run(samples, 16000);
        if !denoised.samples.is_empty() {
            self.out_queue.extend(denoised.samples);
        }

        let needed = samples.len();
        if self.out_queue.len() >= needed {
            samples.copy_from_slice(&self.out_queue[..needed]);
            self.out_queue.drain(..needed);
        } else {
            // 初始冷启动期间队列尚未排满，将已有样本填入，不足部分以静音垫底，严禁残留原始样本拼接导致时域阶跃爆音
            let available = self.out_queue.len();
            samples[..available].copy_from_slice(&self.out_queue[..available]);
            samples[available..].fill(0.0);
            self.out_queue.clear();
        }
    }

    fn reset(&mut self) {
        if let Some(ref denoiser) = self.denoiser {
            denoiser.reset();
        }
        self.in_buf.clear();
        self.out_queue.clear();
    }
}

/// 解析 GTCRN 模型文件路径（支持用户目录、系统共享目录与源码本地目录）
pub fn resolve_gtcrn_model_path() -> Option<std::path::PathBuf> {
    let sub = "ns/gtcrn/gtcrn_simple.onnx";
    let mut search_roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        search_roots.push(std::path::PathBuf::from(home).join(".local/share/waylan/models"));
    }
    search_roots.push(std::path::PathBuf::from("/usr/share/waylan/models"));
    search_roots.push(std::path::PathBuf::from("models"));
    search_roots.push(std::path::PathBuf::from("../../models"));
    search_roots.push(std::path::PathBuf::from("../models"));

    for root in &search_roots {
        let p = root.join(sub);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 解析指定的 DeepFilterNet3 降噪模型路径
pub fn resolve_ns_model_path(custom_path: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(p) = custom_path.filter(|s| !s.trim().is_empty()) {
        let path = std::path::PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = std::path::PathBuf::from(home).join(".local/share/waylan/models/ns/deepfilternet3");
        if p.exists() {
            return Some(p);
        }
    }
    let sys_p = std::path::PathBuf::from("/usr/share/waylan/models/ns/deepfilternet3");
    if sys_p.exists() {
        return Some(sys_p);
    }
    let local_p = std::path::PathBuf::from("models/ns/deepfilternet3");
    if local_p.exists() {
        return Some(local_p);
    }
    let parent_p = std::path::PathBuf::from("../../models/ns/deepfilternet3");
    if parent_p.exists() {
        return Some(parent_p);
    }
    None
}

/// 降噪器工厂构造函数
pub fn create_noise_suppressor(config: &AudioFrontEndConfig) -> Box<dyn NoiseSuppressor> {
    if !config.noise_suppression || config.ns_engine == NsEngineType::Bypass {
        info!("[音频前端] 用户配置关闭人声降噪，已启用直通透传模式");
        return Box::new(BypassSuppressor::new());
    }

    match config.ns_engine {
        NsEngineType::DeepFilterNet => {
            info!("[音频前端] 激活系统内置高阶深度级 DeepFilterNet3 降噪引擎 (强力消除机械键盘打字声)");
            Box::new(DeepFilterSuppressor::new())
        }
        NsEngineType::Gtcrn => {
            info!("[音频前端] 激活系统内置 SOTA 级 GTCRN 深度降噪引擎 (523KB 极速)");
            Box::new(GtcrnSuppressor::new())
        }
        NsEngineType::Rnnoise => {
            info!("[音频前端] 启用系统内置极轻量级 RNNoise 智能人声降噪引擎 (零资源占用)");
            Box::new(RnnoiseSuppressor::new())
        }
        NsEngineType::Bypass => Box::new(BypassSuppressor::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bypass_suppressor() {
        let mut suppressor = BypassSuppressor::new();
        let mut samples = vec![0.1f32, -0.2, 0.3, 0.4];
        let original = samples.clone();
        suppressor.process(&mut samples);
        assert_eq!(samples, original);
    }

    #[test]
    fn test_rnnoise_suppressor_processing() {
        let mut suppressor = RnnoiseSuppressor::new();

        // 连续送入 1600 个采样点（100ms @ 16kHz）的正弦纯音
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();

        suppressor.process(&mut samples);

        // 降噪后样本必须在合法 [-1.0, 1.0] 范围内，且不包含 NaN 或 Inf
        for &s in &samples {
            assert!(!s.is_nan(), "降噪输出不得包含 NaN");
            assert!(!s.is_infinite(), "降噪输出不得包含 Inf");
            assert!(
                (-1.05..=1.05).contains(&s),
                "降噪输出必须处于有效幅值范围，当前: {}",
                s
            );
        }
    }

    #[test]
    fn test_rnnoise_suppressor_reset() {
        let mut suppressor = RnnoiseSuppressor::new();
        let mut samples = vec![0.5f32; 320];
        suppressor.process(&mut samples);

        suppressor.reset();
        assert_eq!(suppressor.last_16k_sample, 0.0);
        assert!(suppressor.in_48k_buf.is_empty());
        assert!(suppressor.out_16k_queue.is_empty());
    }

    #[test]
    fn test_create_noise_suppressor_factory() {
        let cfg = AudioFrontEndConfig {
            noise_suppression: false,
            ..Default::default()
        };
        let mut supp = create_noise_suppressor(&cfg);
        let mut s = vec![0.3f32; 10];
        supp.process(&mut s);
        assert_eq!(s[0], 0.3f32);
    }

    #[test]
    fn test_resolve_ns_model_path_detection() {
        // 验证可成功解析当前工程目录下的 models/ns/deepfilternet3
        let resolved = resolve_ns_model_path(None);
        assert!(
            resolved.is_some(),
            "必须能成功探测到本地内置的 deepfilternet3 降噪模型目录"
        );
        let path = resolved.unwrap();
        assert!(path.join("config.ini").exists());
        assert!(path.join("enc.onnx").exists());

        // 验证自定义有效路径优先级
        let custom_str = path.to_string_lossy();
        let custom = resolve_ns_model_path(Some(&custom_str));
        assert_eq!(custom, Some(path));
    }

    #[test]
    fn test_deep_filter_suppressor_processing_and_reset() {
        let mut suppressor = DeepFilterSuppressor::new();
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();
        suppressor.process(&mut samples);
        for &s in &samples {
            assert!(!s.is_nan());
            assert!(!s.is_infinite());
            assert!((-1.05..=1.05).contains(&s));
        }

        suppressor.reset();
        assert!(suppressor.in_buf.is_empty());
        assert!(suppressor.out_queue.is_empty());
    }

    #[test]
    fn test_gtcrn_suppressor_processing_and_reset() {
        let mut suppressor = GtcrnSuppressor::new();
        // 构造 100ms 的 16kHz 正弦波音频
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();
        suppressor.process(&mut samples);
        for &s in &samples {
            assert!(!s.is_nan(), "GTCRN 降噪输出不得包含 NaN");
            assert!(!s.is_infinite(), "GTCRN 降噪输出不得包含 Inf");
            assert!((-1.05..=1.05).contains(&s));
        }

        suppressor.reset();
        assert!(suppressor.in_buf.is_empty());
        assert!(suppressor.out_queue.is_empty());
    }
}
