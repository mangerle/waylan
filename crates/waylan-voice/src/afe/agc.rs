//! 动态数字自动增益控制 (Digital Auto Gain Control, AGC)
//!
//! # 设计原理
//! - **实现初衷**：消除用户麦克风硬件差异与说话远近场带来的音量巨大起伏（如近距离喷麦爆音削顶、
//!   远距离细微低语无法被 ASR 声学模型捕获）。
//! - **核心优势**：基于自适应 RMS 能量追踪器与双时间常数（5ms 极速压限 Attack、100ms 平滑提升 Release），
//!   结合静音门限冻结机制，彻底杜绝在无人说话时错误放大背景底噪的“抽吸效应”（Noise Pumping）。
//! - **代价与局限**：增益调整具备毫秒级的自适应收敛过程，对于起始前几个周期的微小突发声会有极短平滑响应。

/// 动态数字 AGC 增益控制器
#[derive(Debug, Clone)]
pub struct DigitalAgc {
    /// 目标有效值（RMS）电平（线性值，-16 dBFS 对应约为 0.1585）
    target_rms: f32,
    /// 最小增益（线性值，-12 dB 对应约为 0.25）
    min_gain: f32,
    /// 最大增益（线性值，+18 dB 对应约为 7.94）
    max_gain: f32,
    /// 静音检测阈值（线性值，低于此值视为静音，增益冻结以防抽吸底噪，-45 dBFS 对应约 0.0056）
    silence_threshold: f32,
    /// 当前平滑增益因子
    current_gain: f32,
    /// 压限攻击平滑系数（Attack 系数，音量过大时快速衰减，约 5ms 响应）
    attack_coeff: f32,
    /// 提升释放平滑系数（Release 系数，音量过小时缓慢抬升，约 100ms 响应）
    release_coeff: f32,
}

impl Default for DigitalAgc {
    fn default() -> Self {
        Self::new(16_000.0)
    }
}

impl DigitalAgc {
    /// 创建数字 AGC 实例
    ///
    /// # 参数
    /// - `sample_rate`: 音频采样率（例如 16000.0 Hz）
    pub fn new(sample_rate: f32) -> Self {
        // -16 dBFS 目标电平
        let target_rms = 10.0f32.powf(-16.0 / 20.0);
        // -12 dB 最小增益
        let min_gain = 10.0f32.powf(-12.0 / 20.0);
        // +18 dB 最大增益
        let max_gain = 10.0f32.powf(18.0 / 20.0);
        // -45 dBFS 静音阈值
        let silence_threshold = 10.0f32.powf(-45.0 / 20.0);

        // 计算单样本平滑系数：coeff = exp(-1 / (sample_rate * tau))
        let attack_time_sec = 0.005; // 5ms
        let release_time_sec = 0.100; // 100ms
        let attack_coeff = (-1.0 / (sample_rate * attack_time_sec)).exp();
        let release_coeff = (-1.0 / (sample_rate * release_time_sec)).exp();

        Self {
            target_rms,
            min_gain,
            max_gain,
            silence_threshold,
            current_gain: 1.0,
            attack_coeff,
            release_coeff,
        }
    }

    /// 就地处理音频切片，执行增益平滑计算与波形放缩
    pub fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        // 1. 计算当前切片的 RMS 能量
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        let frame_rms = (sum_sq / samples.len() as f32).sqrt();

        // 2. 静音防抽吸保护：若当前切片能量低于静音门限，增益冻结，防止放大背景杂音
        if frame_rms < self.silence_threshold {
            // 对静音帧仅应用当前已冻结的增益，防止增益在此期间无底线拉大
            for s in samples.iter_mut() {
                *s = (*s * self.current_gain).clamp(-1.0, 1.0);
            }
            return;
        }

        // 3. 计算期望目标增益
        let desired_gain = (self.target_rms / frame_rms).clamp(self.min_gain, self.max_gain);

        // 4. 逐采样点执行平滑增益过渡与限幅保护
        for s in samples.iter_mut() {
            let coeff = if desired_gain < self.current_gain {
                self.attack_coeff
            } else {
                self.release_coeff
            };

            // 一阶 IIR 低通平滑增益轨迹
            self.current_gain = coeff * self.current_gain + (1.0 - coeff) * desired_gain;

            // 应用增益并严格钳位至 [-1.0, 1.0]，防止爆音溢出
            *s = (*s * self.current_gain).clamp(-1.0, 1.0);
        }
    }

    /// 获取当前生效的增益倍数
    pub fn current_gain(&self) -> f32 {
        self.current_gain
    }

    /// 重置 AGC 内部增益状态至 1.0
    pub fn reset(&mut self) {
        self.current_gain = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agc_silence_freeze() {
        let mut agc = DigitalAgc::default();
        let initial_gain = agc.current_gain();

        // 输入微弱纯静音帧（RMS 约为 0.0001，远低于 -45 dBFS 阈值）
        let mut silent = vec![0.0001f32; 1600];
        agc.process(&mut silent);

        // 增益不应疯狂暴增
        assert!(
            (agc.current_gain() - initial_gain).abs() < 1e-4,
            "遇到静音帧时 AGC 增益必须冻结，防止抽吸底噪"
        );
    }

    #[test]
    fn test_agc_amplifies_quiet_voice() {
        let mut agc = DigitalAgc::default();

        // 输入微弱但非静音的声音（幅值约 0.03，低于目标 0.1585）
        for _ in 0..10 {
            let mut quiet = vec![0.03f32; 1600];
            agc.process(&mut quiet);
        }

        // 经过一段时间自适应后，增益应当显著大于 1.0
        assert!(
            agc.current_gain() > 2.0,
            "对于微弱语音，AGC 应平滑放大音量，当前增益: {}",
            agc.current_gain()
        );
    }

    #[test]
    fn test_agc_attenuates_loud_voice() {
        let mut agc = DigitalAgc::default();

        // 输入极大音量（幅值 0.9，远高于目标 0.1585）
        for _ in 0..5 {
            let mut loud = vec![0.9f32; 1600];
            agc.process(&mut loud);
        }

        // 增益应当迅速衰减至小于 1.0
        assert!(
            agc.current_gain() < 0.8,
            "对于过大声音，AGC 应迅速衰减增益，当前增益: {}",
            agc.current_gain()
        );
    }
}
