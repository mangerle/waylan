//! 语音识别器通用 Trait 与仿真测试实现

use crate::error::VoiceError;

/// 语音识别引擎核心特型
pub trait VoiceRecognizer: Send + Sync {
    /// 输入一段 16kHz 单声道 PCM 音频采样，返回当前识别到的增量/流式候选文字（如有）
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError>;

    /// 结束当前说话流并生成最终确认的整句文字
    fn finish(&mut self) -> Result<String, VoiceError>;

    /// 取消或重置当前识别引擎状态
    fn reset(&mut self);
}

/// 仿真测试语音识别器 (Mock Voice Recognizer)
#[derive(Debug, Default)]
pub struct MockVoiceRecognizer {
    /// 累计接收到的采样总数（16000 采样 = 1 秒）
    accumulated_samples: usize,
    /// 当前流式文本输出阶段索引
    step_index: usize,
    /// 预设的演示句子列表
    demo_sentences: Vec<&'static str>,
}

impl MockVoiceRecognizer {
    /// 创建仿真语音识别器
    pub fn new() -> Self {
        Self {
            accumulated_samples: 0,
            step_index: 0,
            demo_sentences: vec!["微澜", "微澜语音", "微澜语音输入", "微澜语音输入测试成功"],
        }
    }
}

impl VoiceRecognizer for MockVoiceRecognizer {
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        self.accumulated_samples += samples.len();

        let current_step = self.accumulated_samples / 4800;
        if current_step > self.step_index && !self.demo_sentences.is_empty() {
            self.step_index = current_step;
            let idx = self
                .step_index
                .saturating_sub(1)
                .min(self.demo_sentences.len() - 1);
            let text = self.demo_sentences[idx].to_string();
            return Ok(Some(text));
        }

        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        let final_text = if self.step_index > 0 && !self.demo_sentences.is_empty() {
            let idx = self
                .step_index
                .saturating_sub(1)
                .min(self.demo_sentences.len() - 1);
            self.demo_sentences[idx].to_string()
        } else {
            "微澜语音输入".to_string()
        };

        self.reset();
        Ok(final_text)
    }

    fn reset(&mut self) {
        self.accumulated_samples = 0;
        self.step_index = 0;
    }
}
