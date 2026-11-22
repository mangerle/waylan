//! 语音识别 (ASR) 引擎抽象特型与多后端实现（离线模型/云端API/仿真测试）
//!
//! # 设计原理
//! - **实现初衷**：定义统一解耦的 `VoiceRecognizer` Trait，支持用户在本地端侧离线模型（SenseVoice-Small/Fun-ASR-Nano）、
//!   线上云端 API 与仿真测试之间自由热插拔与平滑切换。
//! - **核心优势**：本地离线模型文件路径完全外部化配置，支持随时更换不同尺寸与架构的权重；云端 API 支持无模型轻量部署。
//! - **代价与局限**：离线模型依赖用户提前部署 ONNX 权重文件，未就绪时自动给出友好的中文定位提示并优雅降级。

mod cloud;
mod offline;
mod traits;

pub use cloud::{encode_pcm_to_wav, test_cloud_profile_connection, CloudVoiceRecognizer};
pub use offline::{resolve_asr_model_dir, OfflineVoiceRecognizer};
pub use traits::{MockVoiceRecognizer, VoiceRecognizer};

use waylan_core::{CloudAsrConfig, VoiceConfig, VoiceEngineType};

/// 识别引擎工厂函数：根据系统配置动态构造对应的识别器后端
pub fn create_recognizer(config: &VoiceConfig) -> Box<dyn VoiceRecognizer> {
    if let Some(ref sel) = config.selected_asr {
        if let Some(profile_id) = sel.strip_prefix("cloud:") {
            let cloud_cfg = config
                .cloud_profiles
                .iter()
                .find(|p| p.id == profile_id)
                .map(|p| CloudAsrConfig {
                    provider: p.provider,
                    endpoint: p.endpoint.clone(),
                    api_key: p.api_key.clone(),
                    model: p.model.clone(),
                })
                .unwrap_or_else(|| config.cloud.clone());
            return Box::new(CloudVoiceRecognizer::new(cloud_cfg));
        }
    }

    match config.engine_type {
        VoiceEngineType::Offline => Box::new(OfflineVoiceRecognizer::new(config.offline.clone())),
        VoiceEngineType::Cloud => Box::new(CloudVoiceRecognizer::new(config.cloud.clone())),
        VoiceEngineType::Mock => Box::new(MockVoiceRecognizer::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::{CloudProviderType, OfflineAsrConfig, OfflineModelType, VoiceEngineType};

    #[test]
    fn test_mock_recognizer_flow() {
        let mut rec = MockVoiceRecognizer::new();
        let chunk = vec![0.1; 5000];
        let partial = rec.feed_pcm(&chunk).unwrap();
        assert!(partial.is_some());
        assert_eq!(partial.unwrap(), "微澜");

        let finish_res = rec.finish().unwrap();
        assert_eq!(finish_res, "微澜");
    }

    #[test]
    fn test_recognizer_factory_offline() {
        let cfg = VoiceConfig {
            engine_type: VoiceEngineType::Offline,
            ..Default::default()
        };
        let mut rec = create_recognizer(&cfg);
        let chunk = vec![0.0; 5000];
        if rec.is_ready() {
            let feed_res = rec.feed_pcm(&chunk);
            assert!(feed_res.is_ok());
            let finish_res = rec.finish();
            assert!(finish_res.is_ok());
        } else {
            // 本地未下载 SenseVoice 模型时，严格遵循失败关闭契约报错
            let feed_res = rec.feed_pcm(&chunk);
            assert!(matches!(
                feed_res,
                Err(crate::error::VoiceError::RecognizerUnavailable { .. })
            ));
        }
    }

    #[test]
    fn test_offline_recognizer_default_sensevoice() {
        let cfg = OfflineAsrConfig::default();
        let rec = OfflineVoiceRecognizer::new(cfg);
        assert_eq!(rec.model_type(), OfflineModelType::SenseVoice);
    }

    #[test]
    fn test_offline_recognizer_funasr_nano() {
        let cfg = OfflineAsrConfig {
            model_type: OfflineModelType::FunAsrNano,
            ..Default::default()
        };
        let rec = OfflineVoiceRecognizer::new(cfg);
        assert_eq!(rec.model_type(), OfflineModelType::FunAsrNano);
    }

    #[test]
    fn test_offline_recognizer_qwen3_asr() {
        let cfg = OfflineAsrConfig {
            model_type: OfflineModelType::Qwen3Asr,
            ..Default::default()
        };
        let rec = OfflineVoiceRecognizer::new(cfg);
        assert_eq!(rec.model_type(), OfflineModelType::Qwen3Asr);
    }

    #[test]
    fn test_offline_recognizer_firered_asr2() {
        let cfg = OfflineAsrConfig {
            model_type: OfflineModelType::FireRedAsr2,
            ..Default::default()
        };
        let rec = OfflineVoiceRecognizer::new(cfg);
        assert_eq!(rec.model_type(), OfflineModelType::FireRedAsr2);
    }

    #[test]
    fn test_recognizer_factory_cloud_unconfigured() {
        let mut cfg = VoiceConfig {
            engine_type: VoiceEngineType::Cloud,
            ..Default::default()
        };
        cfg.cloud.provider = CloudProviderType::OpenAiCompatible;
        cfg.cloud.api_key = None;

        let mut rec = create_recognizer(&cfg);
        let chunk = vec![0.1; 16000];
        assert!(rec.feed_pcm(&chunk).unwrap().is_none());
        let res = rec.finish();
        assert!(res.is_err(), "未配置 API Key 时应当返回识别失败错误");
        let err_str = res.unwrap_err().to_string();
        assert!(
            err_str.contains("未配置有效的 API Key 凭据"),
            "错误信息应清晰提示 API Key 凭据缺失: {}",
            err_str
        );
    }

    #[test]
    fn test_encode_pcm_to_wav_header() {
        let samples = vec![0.0f32; 16000];
        let wav = encode_pcm_to_wav(&samples, 16000);
        assert!(wav.len() >= 44 + 32000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn test_cloud_recognizer_reuses_http_agent() {
        let cfg = CloudAsrConfig {
            endpoint: "https://example.com/v1/audio/transcriptions".to_string(),
            api_key: Some("test_secret_key".to_string()),
            ..Default::default()
        };
        let rec = CloudVoiceRecognizer::new(cfg);

        // 验证 agent 连接池实例已被正确构造持有
        let _agent_ref = rec.agent();
        assert_eq!(
            rec.config().endpoint,
            "https://example.com/v1/audio/transcriptions"
        );
    }
}
