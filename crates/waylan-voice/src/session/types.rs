//! 语音会话状态机核心事件与通信指令类型

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use waylan_core::VoiceConfig;

use crate::afe::AudioFrontEndPipeline;
use crate::capture::AudioCapture;
use crate::recognizer::VoiceRecognizer;
use crate::vad::{VadProcessor, WaveformVisualizer};

/// 起音前置环形预缓冲区容量（以 16kHz 单声道采样点计）：300ms = 4800 采样点
/// 用于在 VAD 判定为说话前的静音抑制期间暂存最近 300ms 音频，确保起音瞬间不被切断微弱辅音声母
pub const PRE_SPEECH_BUFFER_CAPACITY: usize = 16_000 * 300 / 1000;

/// 语音模型空闲自动卸载超时阈值（5 分钟无录音活动自动释放权重，归还 250MB+ 系统内存）
pub const IDLE_MODEL_UNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// 语音输入会话对外发出的交互事件
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    /// 实时音量波形跳动（波形字符串如 " ▃▅▇▅▃"，音量 0.0 ~ 1.0）
    WaveformUpdated { waveform: String, volume: f32 },
    /// 流式部分识别文本更新
    PartialText(String),
    /// VAD 判定停顿自动断句提交（不中断录音，持续保持听写）
    SegmentCommit(String),
    /// 识别完毕，最终上屏文本（按快捷键或 Enter 触发会话完全结束）
    FinalCommit(String),
    /// 录音会话被用户取消或重置
    Cancelled,
    /// 录音或识别异常
    Error(String),
}

/// 语音控制器配置选项
#[derive(Debug, Clone, Default)]
pub struct VoiceSessionOptions {
    /// 语音全局配置参数（包含 VAD 配置、离线模型配置与云端配置）
    pub voice_config: VoiceConfig,
}

/// 后台常驻工作线程内部控制指令
pub(super) enum WorkerCommand {
    /// 开始新一轮录音与识别会话
    Start {
        options: Box<VoiceSessionOptions>,
        event_tx: SyncSender<VoiceEvent>,
    },
    /// 停止录音并确认提交最终文字
    StopAndCommit,
    /// 取消当前录音并丢弃
    Cancel,
    /// 卸载并释放已加载的模型权重，释放系统内存
    UnloadModels,
    /// 销毁常驻线程平滑退出
    Shutdown,
}

/// 语音会话主动停止与优雅排空提交参数上下文
pub(super) struct StopCommitContext<'a> {
    pub capture: Option<AudioCapture>,
    pub pcm_rx: &'a Receiver<Vec<f32>>,
    pub recognizer: &'a mut dyn VoiceRecognizer,
    pub afe: &'a mut AudioFrontEndPipeline,
    pub pre_speech_buffer: &'a mut VecDeque<f32>,
    pub last_partial_text: &'a mut String,
    pub voice_config: &'a VoiceConfig,
    pub event_tx: &'a SyncSender<VoiceEvent>,
    pub is_active: &'a Arc<AtomicBool>,
}

/// 活跃录音与实时推理运行上下文参数结构体
pub(super) struct ActiveRecordingContext<'a> {
    pub capture: Option<AudioCapture>,
    pub pcm_rx: Receiver<Vec<f32>>,
    pub recycle_tx: SyncSender<Vec<f32>>,
    pub cmd_rx: &'a Receiver<WorkerCommand>,
    pub event_tx: &'a SyncSender<VoiceEvent>,
    pub visualizer: &'a mut WaveformVisualizer,
    pub vad: &'a mut dyn VadProcessor,
    pub recognizer: &'a mut dyn VoiceRecognizer,
    pub voice_config: &'a VoiceConfig,
    pub is_active: &'a Arc<AtomicBool>,
    pub shutdown_requested: &'a Arc<AtomicBool>,
}

/// 识别流式馈送处理上下文参数结构体
pub(super) struct RecognitionFeedContext<'a> {
    pub recognizer: &'a mut dyn VoiceRecognizer,
    pub pre_speech_buffer: &'a mut VecDeque<f32>,
    pub last_partial_text: &'a mut String,
    pub event_tx: &'a SyncSender<VoiceEvent>,
    pub real_time_preview: bool,
}
