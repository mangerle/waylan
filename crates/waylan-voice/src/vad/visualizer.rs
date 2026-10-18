//! 语音能量计算与动态波形可视化
//!
//! # 设计原理
//! - **实现初衷**：为输入法候选框与语音面板提供流畅的实时音量可视化反馈（Unicode 块级波形字符）。
//! - **核心优势**：基于固定长度栈数组滑动窗口，零动态堆内存分配，纳秒级微量开销。
//! - **代价与局限**：采用简单的 8 级线性映射与历史平滑，适用于直观指示而非专业频谱仪分析。

/// Unicode 块级音量梯度符号（共 8 级）
const VOLUME_CHARS: [char; 8] = [' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// 计算给定的 32 位浮点 PCM 采样的均方根能量 (Root Mean Square, RMS)
///
/// 返回值范围通常在 `0.0` 到 `1.0` 之间。
#[inline]
pub fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_squares: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

/// 动态音量波形条生成器
#[derive(Debug, Clone)]
pub struct WaveformVisualizer {
    /// 历史平滑音量队列（固定长度滑动窗口）
    history: [f32; 5],
    /// 当前滑动窗口写入指针
    write_idx: usize,
}

impl Default for WaveformVisualizer {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveformVisualizer {
    /// 创建新的波形生成器，默认窗口大小为 5 根柱形
    pub fn new() -> Self {
        Self {
            history: [0.0; 5],
            write_idx: 0,
        }
    }

    /// 压入新的瞬间音量采样（0.0 ~ 1.0）并生成当前渲染字符串
    pub fn update(&mut self, current_volume: f32) -> String {
        self.history[self.write_idx] = current_volume;
        self.write_idx = (self.write_idx + 1) % self.history.len();

        let mut out = String::with_capacity(self.history.len());
        for i in 0..self.history.len() {
            let idx = (self.write_idx + i) % self.history.len();
            let vol = self.history[idx];
            let level = (vol * 15.0).clamp(0.0, 7.0) as usize;
            out.push(VOLUME_CHARS[level]);
        }
        out
    }

    /// 重置波形历史状态
    pub fn reset(&mut self) {
        self.history = [0.0; 5];
        self.write_idx = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rms() {
        assert_eq!(calculate_rms(&[]), 0.0);
        let samples = [0.5, -0.5, 0.5, -0.5];
        let rms = calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_waveform_visualizer() {
        let mut visualizer = WaveformVisualizer::new();
        let frame1 = visualizer.update(0.0);
        assert_eq!(frame1.chars().count(), 5);

        let frame2 = visualizer.update(0.8);
        assert_eq!(frame2.chars().count(), 5);
        assert!(frame2.contains('█'));
    }
}
