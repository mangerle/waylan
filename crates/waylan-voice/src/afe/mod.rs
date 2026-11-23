//! 音频前端预处理流水线 (Audio Front-End Pipeline, AFE)
//!
//! # 设计原理
//! - **实现初衷**：统合 80Hz 高通滤波、人声智能降噪与动态数字 AGC，提供面向语音识别流水线的统一增强门面。
//! - **核心优势**：
//!   1. 毫秒级在线就地（In-Place）流式变换，零额外堆内存重新分配；
//!   2. 顺序科学：先通过 HPF 切除 80Hz 以下低频桌面震动，释放神经网络动态范围；
//!      再通过 NS 抹除非平稳敲击与平稳白噪音；
//!      最后通过 AGC 平滑提升或压限至标准 ASR 舒适电平 (-16 dBFS)；
//!   3. 各级组件均支持按配置动态开启/旁路跳过。
//! - **代价与局限**：前置滤波与降噪网络会对纯环境白噪音进行抑制，波形可视化将更专注人声动态。

pub mod agc;
pub mod hpf;
pub mod ns;

pub use agc::DigitalAgc;
pub use hpf::HighPassFilter;
pub use ns::{NoiseSuppressor, create_noise_suppressor};

use log::debug;
use waylan_core::AudioFrontEndConfig;

/// 音频前端预处理全流水线控制器
pub struct AudioFrontEndPipeline {
    /// 是否开启前端预处理阶段总控
    enabled: bool,
    /// 是否开启 80Hz 高通滤波
    enable_hpf: bool,
    /// 80Hz 高通滤波器
    hpf: HighPassFilter,
    /// 是否开启人声智能降噪
    enable_ns: bool,
    /// 人声降噪引擎实例
    ns: Box<dyn NoiseSuppressor>,
    /// 是否开启动态数字自动增益控制 (AGC)
    enable_agc: bool,
    /// 动态数字 AGC
    agc: DigitalAgc,
}

impl AudioFrontEndPipeline {
    /// 根据语音配置创建音频前端预处理管道
    pub fn new(config: &AudioFrontEndConfig) -> Self {
        Self {
            enabled: config.enabled,
            enable_hpf: config.high_pass_filter,
            hpf: HighPassFilter::default(),
            enable_ns: config.noise_suppression,
            ns: create_noise_suppressor(config),
            enable_agc: config.auto_gain_control,
            agc: DigitalAgc::default(),
        }
    }

    /// 对输入单声道 16kHz PCM 浮点音频切片就地执行三阶段音频增强
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.enabled || samples.is_empty() {
            return;
        }

        // 阶段一：80Hz 高通滤波，切除低频机械与电磁轰鸣
        if self.enable_hpf {
            self.hpf.process(samples);
        }

        // 阶段二：人声智能降噪，消灭环境键盘声与风扇噪声
        if self.enable_ns {
            self.ns.process(samples);
        }

        // 阶段三：动态数字 AGC，平衡音量至最佳识别电平 (-16 dBFS)
        if self.enable_agc {
            self.agc.process(samples);
        }

        debug!(
            "[音频前端] 完成 {} 个采样点的就地音频增强处理",
            samples.len()
        );
    }

    /// 重置整条流水线的所有滤波器与循环状态
    pub fn reset(&mut self) {
        self.hpf.reset();
        self.ns.reset();
        self.agc.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_front_end_pipeline_full_run() {
        let config = AudioFrontEndConfig::default();
        let mut pipeline = AudioFrontEndPipeline::new(&config);

        // 模拟 1600 个采样点（100ms @ 16kHz）
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| {
                // 30Hz 低频干扰 + 500Hz 人声基频 + 0.05 随机白噪声
                let low = (2.0 * std::f32::consts::PI * 30.0 * (i as f32) / 16000.0).sin() * 0.3;
                let voice = (2.0 * std::f32::consts::PI * 500.0 * (i as f32) / 16000.0).sin() * 0.1;
                low + voice
            })
            .collect();

        pipeline.process(&mut samples);

        // 确保处理后所有样本合法，无越界与非法数值
        for &s in &samples {
            assert!(!s.is_nan());
            assert!(!s.is_infinite());
            assert!((-1.0..=1.0).contains(&s));
        }

        pipeline.reset();
    }
}
