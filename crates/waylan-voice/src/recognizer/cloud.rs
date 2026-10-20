//! 线上云端语音识别器（基于 OpenAI/Whisper 规范 API 与 HTTP 连接池）

use log::info;
use waylan_core::{CloudAsrConfig, CloudProviderType};

use super::traits::VoiceRecognizer;
use crate::error::VoiceError;

/// 单次云端识别允许缓存的最大 PCM 采样数（16kHz 下 5 分钟）
const MAX_CLOUD_PCM_SAMPLES: usize = 16_000 * 300;

/// 校验本次追加采样后是否仍处于单次云端识别上限内
fn validate_buffer_growth(
    current_samples: usize,
    incoming_samples: usize,
) -> Result<(), VoiceError> {
    if current_samples.saturating_add(incoming_samples) > MAX_CLOUD_PCM_SAMPLES {
        return Err(VoiceError::RecognizerFailed {
            engine_type: "云端 Whisper".to_string(),
            message: format!(
                "单次云端识别音频超过 {} 秒上限，已终止本次请求；请缩短单次听写时长或改用本地离线模型",
                MAX_CLOUD_PCM_SAMPLES / 16_000
            ),
        });
    }
    Ok(())
}

/// 将 16kHz f32 PCM 浮点音频采样序列编码为标准 16 位单声道 WAV 字节数据
pub fn encode_pcm_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let subchunk2_size = (samples.len() * 2) as u32;
    let chunk_size = 36 + subchunk2_size;

    let mut wav = Vec::with_capacity(44 + subchunk2_size as usize);
    // RIFF 块描述
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // "fmt " 子块
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM 格式块固定大小 16
    wav.extend_from_slice(&1u16.to_le_bytes()); // 格式 1 = 线性 PCM
    wav.extend_from_slice(&num_channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // "data" 数据子块
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&subchunk2_size.to_le_bytes());

    // 转换 f32 (-1.0 ~ 1.0) 为 i16 小端 PCM 字节
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let val = (clamped * 32767.0).round() as i16;
        wav.extend_from_slice(&val.to_le_bytes());
    }

    wav
}

/// 线上云端语音识别器（支持自由切换不同云服务与 API Key）
pub struct CloudVoiceRecognizer {
    /// 云端配置
    config: CloudAsrConfig,
    /// 累积待发送的 PCM 音频缓冲（16kHz f32）
    pcm_buffer: Vec<f32>,
    /// 长生命周期 HTTP 客户端连接池，复用底层 TCP Keep-Alive、TLS 握手与本地 DNS 缓存
    agent: ureq::Agent,
}

impl CloudVoiceRecognizer {
    /// 创建云端识别器并初始化长生命周期 HTTP 客户端连接池
    pub fn new(config: CloudAsrConfig) -> Self {
        info!(
            "[云端 ASR] 正在初始化线上语音服务客户端 (协议: {:?}, 地址: {})",
            config.provider, config.endpoint
        );
        let timeout = std::time::Duration::from_secs(15);
        let agent = ureq::AgentBuilder::new()
            .timeout(timeout)
            .max_idle_connections(8)
            .max_idle_connections_per_host(4)
            .build();
        Self {
            config,
            pcm_buffer: Vec::with_capacity(16_000 * 5),
            agent,
        }
    }

    /// 获取当前云端配置
    pub fn config(&self) -> &CloudAsrConfig {
        &self.config
    }

    /// 获取当前持有的 HTTP 客户端连接池引用
    pub fn agent(&self) -> &ureq::Agent {
        &self.agent
    }
}

impl VoiceRecognizer for CloudVoiceRecognizer {
    fn is_ready(&self) -> bool {
        self.config
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
    }

    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        validate_buffer_growth(self.pcm_buffer.len(), samples.len())?;
        self.pcm_buffer.extend_from_slice(samples);
        // 云端批处理/分块上传接口在说话过程中累积采样
        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        if self.config.provider != CloudProviderType::OpenAiCompatible {
            return Err(VoiceError::UnsupportedCloudProvider {
                provider: format!("{:?}", self.config.provider),
            });
        }
        if self.pcm_buffer.is_empty() {
            return Ok(String::new());
        }

        let (api_key, endpoint) = match resolve_api_credentials(&self.config) {
            Ok(creds) => creds,
            Err(e) => {
                self.reset();
                return Err(e);
            }
        };

        let wav_data = encode_pcm_to_wav(&self.pcm_buffer, 16_000);
        let duration_secs = self.pcm_buffer.len() as f64 / 16000.0;
        self.reset();

        info!(
            "[云端 ASR] 正在向远端服务发起识别请求 (地址: {}, 音频: {:.2}s, 字节数: {})",
            endpoint,
            duration_secs,
            wav_data.len()
        );

        let (boundary, body) = build_multipart_payload(&self.config.model, &wav_data);

        // 复用长生命周期 HTTP 连接池发送识别请求
        let resp = self
            .agent
            .post(&endpoint)
            .set("Authorization", &format!("Bearer {}", api_key))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .send_bytes(&body)
            .map_err(|e| VoiceError::RecognizerFailed {
                engine_type: "云端 Whisper".to_string(),
                message: format!("请求远端语音识别服务失败: {}", e),
            })?;

        parse_cloud_response(resp)
    }

    fn reset(&mut self) {
        self.pcm_buffer.clear();
    }
}

/// 测试云端服务接口连通性与远端响应
///
/// # Errors
/// 当网络请求失败或参数错误时返回 VoiceError
pub fn test_cloud_profile_connection(config: &CloudAsrConfig) -> Result<String, VoiceError> {
    let mut recognizer = CloudVoiceRecognizer::new(config.clone());
    let dummy_samples = vec![0.0f32; 8000];
    recognizer.feed_pcm(&dummy_samples)?;
    recognizer.finish()
}

/// 解析与验证云端 ASR 的 API Key 与终端地址
fn resolve_api_credentials(config: &CloudAsrConfig) -> Result<(String, String), VoiceError> {
    let api_key = match &config.api_key {
        Some(key) if !key.trim().is_empty() => key.trim().to_string(),
        _ => {
            return Err(VoiceError::RecognizerFailed {
                engine_type: "云端 Whisper".to_string(),
                message: "未配置有效的 API Key 凭据，无法发起远端识别".to_string(),
            });
        }
    };

    let endpoint = if config.endpoint.trim().is_empty() {
        "https://api.openai.com/v1/audio/transcriptions".to_string()
    } else {
        config.endpoint.clone()
    };

    Ok((api_key, endpoint))
}

/// 构造标准的 multipart/form-data 报文体
fn build_multipart_payload(model: &str, wav_data: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!(
        "----WayLanBoundary{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut body = Vec::new();

    // 1. model 字段
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
    let model_name = if model.trim().is_empty() {
        "whisper-1"
    } else {
        model.trim()
    };
    body.extend_from_slice(model_name.as_bytes());
    body.extend_from_slice(b"\r\n");

    // 2. language 字段
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
    body.extend_from_slice(b"zh\r\n");

    // 3. file 音频文件内容
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav_data);
    body.extend_from_slice(b"\r\n");

    // 结束边界标记
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    (boundary, body)
}

/// 解析云端 HTTP 响应与错误提取
fn parse_cloud_response(resp: ureq::Response) -> Result<String, VoiceError> {
    let status = resp.status();
    if (200..300).contains(&status) {
        let resp_str = resp
            .into_string()
            .map_err(|e| VoiceError::RecognizerFailed {
                engine_type: "云端 Whisper".to_string(),
                message: format!("解析云端响应字符串失败: {}", e),
            })?;
        // 尝试从标准 Whisper 返回 JSON 中解析识别文本
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_str) {
            if let Some(text) = val.get("text").and_then(|t| t.as_str()) {
                info!("[云端 ASR] 识别成功，文本长度: {}", text.len());
                return Ok(text.to_string());
            }
        }
        Ok(resp_str)
    } else {
        let status_text = resp.status_text().to_string();
        let resp_body = resp.into_string().unwrap_or_default();
        let detail = if !resp_body.trim().is_empty() {
            format!(
                "状态码: {} ({}), 详情: {}",
                status,
                status_text,
                resp_body.trim()
            )
        } else {
            format!("状态码: {} ({})", status, status_text)
        };
        Err(VoiceError::RecognizerFailed {
            engine_type: "云端 Whisper".to_string(),
            message: format!("远端服务器返回错误: {}", detail),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unsupported_cloud_provider_is_reported() {
        let mut recognizer = CloudVoiceRecognizer::new(CloudAsrConfig {
            provider: CloudProviderType::Aliyun,
            api_key: Some("test-key".to_string()),
            ..Default::default()
        });
        assert!(matches!(
            recognizer.finish(),
            Err(VoiceError::UnsupportedCloudProvider { .. })
        ));
    }

    #[test]
    fn test_cloud_pcm_buffer_limit() {
        assert!(validate_buffer_growth(MAX_CLOUD_PCM_SAMPLES - 1, 1).is_ok());
        assert!(matches!(
            validate_buffer_growth(MAX_CLOUD_PCM_SAMPLES, 1),
            Err(VoiceError::RecognizerFailed { .. })
        ));
        assert!(matches!(
            validate_buffer_growth(MAX_CLOUD_PCM_SAMPLES, usize::MAX),
            Err(VoiceError::RecognizerFailed { .. })
        ));
    }
}
