//! 语音会话工作线程常驻模型缓存与空闲回收管理
//!
//! # 设计原理
//! - **实现初衷**：统一纳管 ASR 语音识别器、VAD 端点检测与标点恢复模型的常驻权重与指纹校验。
//! - **核心优势**：在多次录音间复用内存模型消除 1 秒冷启动延迟；5 分钟持续空闲自动卸载释放 200MB+ 内存。
//! - **代价与局限**：需由单一常驻线程独占调用。

use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::VoiceConfig;

use super::types::IDLE_MODEL_UNLOAD_TIMEOUT;
use crate::recognizer::{create_recognizer, VoiceRecognizer};
use crate::vad::{create_vad, VadProcessor};

/// 离线 ASR 识别器缓存键指纹
///
/// 由引擎类型、本地模型目录、云端服务配置与标点恢复模型路径共同构成；
/// 任一要素变更即视为指纹不一致，需重建识别器。
pub type AsrFingerprint = (
    waylan_core::VoiceEngineType,
    Option<String>,
    Option<waylan_core::CloudAsrConfig>,
    Option<String>,
);

/// 提取当前语音识别器的关键配置指纹，用于识别器缓存失效判定
pub fn compute_asr_fingerprint(cfg: &VoiceConfig) -> AsrFingerprint {
    let effective_cloud = if let Some(ref sel) = cfg.selected_asr {
        if let Some(profile_id) = sel.strip_prefix("cloud:") {
            cfg.cloud_profiles
                .iter()
                .find(|p| p.id == profile_id)
                .map(|p| waylan_core::CloudAsrConfig {
                    provider: p.provider,
                    endpoint: p.endpoint.clone(),
                    api_key: p.api_key.clone(),
                    model: p.model.clone(),
                })
                .or_else(|| Some(cfg.cloud.clone()))
        } else {
            None
        }
    } else if cfg.engine_type == waylan_core::VoiceEngineType::Cloud {
        Some(cfg.cloud.clone())
    } else {
        None
    };

    (
        cfg.engine_type,
        cfg.selected_asr.clone(),
        effective_cloud,
        cfg.offline.model_dir.clone(),
    )
}

/// 语音会话工作线程常驻模型与指纹缓存容器
#[derive(Default)]
pub struct WorkerModelCache {
    recognizer: Option<Box<dyn VoiceRecognizer>>,
    vad: Option<Box<dyn VadProcessor>>,
    last_asr_fingerprint: Option<AsrFingerprint>,
}

impl WorkerModelCache {
    /// 清空所有模型权重缓存以释放常驻内存
    pub fn unload(&mut self) {
        self.recognizer = None;
        self.vad = None;
        self.last_asr_fingerprint = None;
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
