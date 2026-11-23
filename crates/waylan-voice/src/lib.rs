//! 微澜 (WayLan) 语音采集、静音端点检测与语音识别模块
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供独立的音频采集、端点检测 (VAD)、动态波形生成与语音识别 (ASR) 契约。
//! - **核心优势**：纯异步流式设计，与 IBus/Wayland 主输入管道完全解耦，提供零延迟的文字与波形上报通道。
//! - **代价与局限**：依赖 Linux ALSA/PulseAudio/PipeWire 系统音频输入服务。

pub mod afe;
pub mod capture;
pub mod error;
pub mod interaction;
pub mod itn;
pub mod model_manager;
pub mod recognizer;
pub mod session;
pub mod vad;

pub use afe::{
    AudioFrontEndPipeline, DigitalAgc, HighPassFilter, NoiseSuppressor, create_noise_suppressor,
};
pub use capture::{AudioCapture, LinearResampler, TARGET_SAMPLE_RATE};
pub use error::VoiceError;
pub use interaction::{VoiceInteractionManager, VoiceKeyAction};
pub use itn::{
    apply_pangu_spacing, convert_chinese_numbers, filter_filler_words, normalize_voice_text,
};
pub use model_manager::{
    AsrModelInstallState, ModelItem, ModelSubdirectory, OnlineAsrModelMeta, PRESET_ASR_MODELS,
    download_and_extract_model, ensure_user_models_dirs, find_preset_model_by_id,
    get_preset_model_dest_dir, import_asr_model_from_dir, import_onnx_model_from_file,
    is_preset_model_installed, list_asr_models, list_ns_models, list_vad_models,
    open_user_models_directory, user_models_dir,
};
pub use recognizer::{
    CloudVoiceRecognizer, MockVoiceRecognizer, OfflineVoiceRecognizer, VoiceRecognizer,
    create_recognizer, encode_pcm_to_wav, test_cloud_profile_connection,
};
pub use session::{VoiceEvent, VoiceSessionController, VoiceSessionOptions};
pub use vad::{
    EnergyVad, ModelVad, VadDetector, VadProcessor, WaveformVisualizer, calculate_rms, create_vad,
};

pub use waylan_core::config::{
    AudioFrontEndConfig, CloudAsrConfig, CloudProfile, CloudProviderType, NsEngineType,
    OfflineAsrConfig, OfflineModelType, VadConfig, VadModelType, VoiceConfig, VoiceEngineType,
    VoiceInteractionConfig, VoiceItnConfig, VoiceVisualEffect,
};
