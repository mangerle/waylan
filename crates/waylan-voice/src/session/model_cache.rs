//! 语音会话工作线程常驻模型缓存与空闲回收管理
//!
//! # 设计原理
//! - **实现初衷**：统一纳管 ASR 语音识别器、VAD 端点检测与标点恢复模型的常驻权重与指纹校验。
//! - **核心优势**：在多次录音间复用内存模型消除 1 秒冷启动延迟；5 分钟持续空闲自动卸载释放 200MB+ 内存。
//! - **代价与局限**：需由单一常驻线程独占调用。

use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::{
    CloudAsrConfig, OfflineAsrConfig, VadConfig, VadModelType, VoiceConfig, VoiceEngineType,
};

use super::types::IDLE_MODEL_UNLOAD_TIMEOUT;
use crate::recognizer::{VoiceRecognizer, create_recognizer};
use crate::vad::{VadProcessor, create_vad};

/// 语音识别器配置缓存指纹
///
/// 由引擎类型、激活预设/模型标识、生效云端配置与完整离线 ASR 配置共同构成；
/// 包括离线架构 (`model_type`)、模型目录、识别语言与线程数在内的任意变更均触发识别器重建。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrFingerprint {
    /// 识别引擎类型
    pub engine_type: VoiceEngineType,
    /// 当前选中识别器标识
    pub selected_asr: Option<String>,
    /// 生效的云端识别配置
    pub cloud: Option<CloudAsrConfig>,
    /// 完整的离线模型配置（含架构类型、目录、语言、推理线程数）
    pub offline: OfflineAsrConfig,
}

/// VAD 端点检测器配置缓存指纹
///
/// 通过 `speech_threshold` 位模式比较确保阈值微调也能精确触发 VAD 实例热更新。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VadFingerprint {
    /// VAD 算法类型
    pub vad_type: VadModelType,
    /// 自定义 VAD 模型文件路径
    pub model_path: Option<String>,
    /// 语音置信度阈值 IEEE-754 位表示
    pub speech_threshold_bits: u32,
    /// 静音判停超时时长（毫秒）
    pub silence_timeout_ms: u64,
}

/// 提取当前语音识别器的关键配置指纹，用于识别器缓存失效判定
pub fn compute_asr_fingerprint(cfg: &VoiceConfig) -> AsrFingerprint {
    AsrFingerprint {
        engine_type: cfg.engine_type,
        selected_asr: cfg.selected_asr.clone(),
        cloud: cfg.resolve_active_cloud_config(),
        offline: cfg.offline.clone(),
    }
}

/// 提取当前 VAD 检测器的关键配置指纹，用于 VAD 热更新缓存失效判定
pub fn compute_vad_fingerprint(cfg: &VadConfig) -> VadFingerprint {
    VadFingerprint {
        vad_type: cfg.vad_type,
        model_path: cfg.model_path.clone(),
        speech_threshold_bits: cfg.speech_threshold.to_bits(),
        silence_timeout_ms: cfg.silence_timeout_ms,
    }
}

/// 语音会话工作线程常驻模型与指纹缓存容器
#[derive(Default)]
pub struct WorkerModelCache {
    recognizer: Option<Box<dyn VoiceRecognizer>>,
    vad: Option<Box<dyn VadProcessor>>,
    last_asr_fingerprint: Option<AsrFingerprint>,
    last_vad_fingerprint: Option<VadFingerprint>,
}

impl WorkerModelCache {
    /// 清空所有模型权重缓存以释放常驻内存
    pub fn unload(&mut self) {
        self.recognizer = None;
        self.vad = None;
        self.last_asr_fingerprint = None;
        self.last_vad_fingerprint = None;
    }

    /// 查询是否有任何模型权重驻留在内存中
    #[inline]
    pub fn has_cached_models(&self) -> bool {
        self.recognizer.is_some() || self.vad.is_some()
    }

    /// 校验或按需初始化识别器与VAD模型实例
    pub fn prepare_models(
        &mut self,
        voice_config: &VoiceConfig,
    ) -> Option<(
        &mut (dyn VoiceRecognizer + 'static),
        &mut (dyn VadProcessor + 'static),
    )> {
        let current_fingerprint = compute_asr_fingerprint(voice_config);
        if self.last_asr_fingerprint.as_ref() != Some(&current_fingerprint) {
            self.recognizer = None;
            self.last_asr_fingerprint = Some(current_fingerprint);
        }

        if self.recognizer.is_none() {
            self.recognizer = Some(create_recognizer(voice_config));
        }
        if !self
            .recognizer
            .as_ref()
            .is_some_and(|recognizer| recognizer.is_ready())
        {
            warn!("[语音会话] 识别器未就绪（本地模型缺失或云端 API Key 未配置），拒绝启动录音");
            return None;
        }

        let current_vad_fp = compute_vad_fingerprint(&voice_config.vad);
        if self.last_vad_fingerprint.as_ref() != Some(&current_vad_fp) {
            self.vad = None;
            self.last_vad_fingerprint = Some(current_vad_fp);
        }
        if self.vad.is_none() {
            self.vad = Some(create_vad(&voice_config.vad));
        }

        let recognizer = self.recognizer.as_mut()?;
        let vad = self.vad.as_mut()?;
        recognizer.reset();
        vad.reset();

        Some((recognizer.as_mut(), vad.as_mut()))
    }
}

/// 计算下一次接收命令的等待超时并执行模型空闲超时自动卸载
pub fn check_idle_timeout_and_calculate_deadline(
    cache: &mut WorkerModelCache,
    last_activity: Instant,
) -> Duration {
    if cache.has_cached_models() {
        let elapsed = last_activity.elapsed();
        if elapsed >= IDLE_MODEL_UNLOAD_TIMEOUT {
            cache.unload();
            info!("[语音会话] 语音模块已持续空闲超过 5 分钟，自动卸载模型权重以释放常驻内存");
            Duration::from_secs(3600)
        } else {
            IDLE_MODEL_UNLOAD_TIMEOUT.saturating_sub(elapsed)
        }
    } else {
        Duration::from_secs(3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::OfflineModelType;

    #[test]
    fn test_asr_and_vad_fingerprints_detect_all_relevant_changes() {
        let mut cfg = VoiceConfig {
            engine_type: VoiceEngineType::Mock,
            ..Default::default()
        };
        cfg.vad.vad_type = VadModelType::Energy;

        let mut cache = WorkerModelCache::default();
        assert!(cache.prepare_models(&cfg).is_some());
        let first_asr = cache.last_asr_fingerprint.clone();
        let first_vad = cache.last_vad_fingerprint.clone();

        // 变更离线架构、语言与线程数均应刷新 ASR 指纹
        cfg.offline.model_type = OfflineModelType::Qwen3Asr;
        cfg.offline.language = "en".to_string();
        cfg.offline.num_threads = 4;
        assert!(cache.prepare_models(&cfg).is_some());
        assert_ne!(cache.last_asr_fingerprint, first_asr);

        // 变更 VAD 参数应刷新 VAD 指纹
        cfg.vad.speech_threshold = 0.72;
        cfg.vad.silence_timeout_ms = 1200;
        assert!(cache.prepare_models(&cfg).is_some());
        assert_ne!(cache.last_vad_fingerprint, first_vad);

        cache.unload();
        assert!(cache.last_asr_fingerprint.is_none());
        assert!(cache.last_vad_fingerprint.is_none());
    }
}
