//! 微澜 (WayLan) 语音采集、静音端点检测与语音识别模块
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供独立的音频采集、端点检测 (VAD)、动态波形生成与语音识别 (ASR) 契约。
//! - **核心优势**：纯异步流式设计，与 IBus/Wayland 主输入管道完全解耦，提供零延迟的文字与波形上报通道。
//! - **代价与局限**：依赖 Linux ALSA/PulseAudio/PipeWire 系统音频输入服务。

pub mod afe;
pub mod capture;
pub mod error;
pub mod model_manager;
pub mod recognizer;
pub mod session;
pub mod vad;

pub use afe::{
    create_noise_suppressor, AudioFrontEndPipeline, DigitalAgc, HighPassFilter, NoiseSuppressor,
};
pub use capture::{AudioCapture, LinearResampler, TARGET_SAMPLE_RATE};
pub use error::VoiceError;
pub use model_manager::{
    ensure_user_models_dirs, import_asr_model_dialog, import_ns_model_dialog,
    import_onnx_model_dialog, list_asr_models, list_ns_models, list_punct_models, list_vad_models,
    open_user_models_directory, user_models_dir, ModelItem,
};
pub use recognizer::{
    create_recognizer, encode_pcm_to_wav, CloudVoiceRecognizer, MockVoiceRecognizer,
    OfflineVoiceRecognizer, PunctuationRestorer, VoiceRecognizer,
};
pub use session::{VoiceEvent, VoiceSessionController, VoiceSessionOptions};
pub use vad::{
    calculate_rms, create_vad, EnergyVad, ModelVad, VadDetector, VadProcessor, WaveformVisualizer,
};
