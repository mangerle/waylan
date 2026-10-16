//! 语音输入生命周期会话控制器
//!
//! # 设计原理
//! - **实现初衷**：统一纳管音频捕获流、端点静音检测、Unicode 波形生成与识别引擎管道。
//! - **核心优势**：神经网络模型（Paraformer 与 Silero VAD）在后台专用工作线程常驻复用，避免每次录音重复加载
//!   高达 200MB 的模型权重文件；消除近 1 秒的冷启动延迟，实现快捷键按下后 0 延迟秒开录音与识别。
//! - **代价与局限**：后台常驻线程需通过命令通道安全纳管状态流转，并在 Drop 时平滑停机。

mod pipeline;
mod types;

pub use types::{VoiceEvent, VoiceSessionOptions};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::{info, warn};

use self::pipeline::{active_recording_loop, compute_asr_fingerprint};
use self::types::{ActiveRecordingContext, WorkerCommand, IDLE_MODEL_UNLOAD_TIMEOUT};
use crate::capture::AudioCapture;
use crate::error::VoiceError;
use crate::recognizer::{create_recognizer, PunctuationRestorer, VoiceRecognizer};
use crate::vad::{create_vad, VadProcessor, WaveformVisualizer};

/// 语音会话状态纳管器（长生命周期常驻控制器）
pub struct VoiceSessionController {
    /// 录音活动标志（供外界高频原子查询）
    is_active: Arc<AtomicBool>,
    /// 命令发送端
    cmd_tx: SyncSender<WorkerCommand>,
    /// 后台常驻工作线程句柄
    worker_handle: Option<JoinHandle<()>>,
}

impl Default for VoiceSessionController {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceSessionController {
    /// 尝试创建会话控制器并拉起常驻工作线程
    ///
    /// # Errors
    /// 当系统底层线程资源耗尽或创建工作线程失败时返回错误。
    pub fn try_new() -> Result<Self, VoiceError> {
        let is_active = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(16);

        let active_clone = Arc::clone(&is_active);
        let handle = thread::Builder::new()
            .name("waylan-voice-worker".to_string())
            .spawn(move || {
                Self::worker_main(cmd_rx, active_clone);
            })
            .map_err(|e| VoiceError::WorkerThreadSpawnFailed(e.to_string()))?;

        Ok(Self {
            is_active,
            cmd_tx,
            worker_handle: Some(handle),
        })
    }

    /// 创建会话控制器并拉起常驻工作线程
    ///
    /// # 容错降级
    /// 若系统资源紧张导致后台工作线程派生失败，记录错误日志并安全降级为禁用语音功能，
    /// 保证输入法核心守护进程不会意外崩溃。
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|e| {
            log::error!(
                "[微澜语音] 创建后台常驻语音工作线程失败，已安全降级为禁用语音功能: {}",
                e
            );
            let (cmd_tx, _cmd_rx) = sync_channel::<WorkerCommand>(1);
            Self {
                is_active: Arc::new(AtomicBool::new(false)),
                cmd_tx,
                worker_handle: None,
            }
        })
    }

    /// 当前是否正处于录音识别中
    pub fn is_recording(&self) -> bool {
        self.worker_handle.is_some() && self.is_active.load(Ordering::Acquire)
    }

    /// 启动语音录制与识别会话
    ///
    /// # 参数
    /// - `options`: 会话配置参数；
    /// - `event_tx`: 向输入法核心发送事件的有界通道。
    pub fn start(
        &mut self,
        options: VoiceSessionOptions,
        event_tx: SyncSender<VoiceEvent>,
    ) -> Result<(), VoiceError> {
        if self.worker_handle.is_none() {
            warn!("[微澜语音] 语音后台工作线程未就绪（已被安全降级禁用），忽略启动录音请求");
            return Err(VoiceError::WorkerThreadSpawnFailed(
                "语音后台工作线程未就绪（已被安全降级禁用）".to_string(),
            ));
        }

        if self.is_recording() {
            warn!("[语音会话] 录音已在进行中，忽略重复启动请求");
            return Ok(());
        }

        self.is_active.store(true, Ordering::Release);
        if let Err(e) = self.cmd_tx.send(WorkerCommand::Start {
            options: Box::new(options),
            event_tx,
        }) {
            self.is_active.store(false, Ordering::Release);
            return Err(VoiceError::StreamStartFailed(format!(
                "向语音工作线程发送启动指令失败: {}",
                e
            )));
        }

        Ok(())
    }

    /// 停止录音并请求最终提交识别结果
    pub fn stop_and_commit(&mut self) {
        if self.worker_handle.is_some() && self.is_recording() {
            self.is_active.store(false, Ordering::Release);
            let _ = self.cmd_tx.send(WorkerCommand::StopAndCommit);
        }
    }

    /// 取消当前录音并丢弃未完成的识别
    pub fn cancel(&mut self) {
        if self.worker_handle.is_some() && self.is_recording() {
            self.is_active.store(false, Ordering::Release);
            let _ = self.cmd_tx.send(WorkerCommand::Cancel);
        }
    }

    /// 卸载并释放常驻工作线程中已加载的全部模型权重，归还系统内存
    pub fn unload_models(&self) {
        if self.worker_handle.is_some() {
            let _ = self.cmd_tx.send(WorkerCommand::UnloadModels);
        }
    }

    /// 常驻工作线程主入口：纳管模型单例生命周期与会话状态机
    fn worker_main(cmd_rx: Receiver<WorkerCommand>, is_active: Arc<AtomicBool>) {
        let mut visualizer = WaveformVisualizer::new();
        let mut cached_vad: Option<Box<dyn VadProcessor>> = None;
        let mut cached_recognizer: Option<Box<dyn VoiceRecognizer>> = None;
        let mut cached_punct: Option<PunctuationRestorer> = None;
        let mut last_asr_fingerprint = None;

        info!("[语音会话] 后台常驻语音工作线程启动完成，进入就绪待命状态");

        let mut last_activity_time = Instant::now();

        loop {
            let timeout = if cached_recognizer.is_some() || cached_vad.is_some() {
                let elapsed = last_activity_time.elapsed();
                if elapsed >= IDLE_MODEL_UNLOAD_TIMEOUT {
                    cached_recognizer = None;
                    cached_vad = None;
                    cached_punct = None;
                    last_asr_fingerprint = None;
                    info!(
                        "[语音会话] 语音模块已持续空闲超过 5 分钟，自动卸载模型权重以释放常驻内存"
                    );
                    Duration::from_secs(3600)
                } else {
                    IDLE_MODEL_UNLOAD_TIMEOUT.saturating_sub(elapsed)
                }
            } else {
                Duration::from_secs(3600)
            };

            let cmd = match cmd_rx.recv_timeout(timeout) {
                Ok(c) => c,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };

            last_activity_time = Instant::now();
            match cmd {
                WorkerCommand::Start { options, event_tx } => {
                    // 1. 若配置中引擎类型、目标服务、云端凭据或离线模型目录发生变更，主动失效识别器缓存以实现热切换
                    let current_fingerprint = compute_asr_fingerprint(&options.voice_config);
                    if last_asr_fingerprint.as_ref() != Some(&current_fingerprint) {
                        cached_recognizer = None;
                        last_asr_fingerprint = Some(current_fingerprint);
                    }

                    // 初始化或复用常驻语音识别、标点预测与 VAD 模型（仅在首次使用或配置切换时加载权重，后续 0 毫秒复用）
                    if cached_recognizer.is_none() {
                        cached_recognizer = Some(create_recognizer(&options.voice_config));
                    }
                    if cached_vad.is_none() {
                        cached_vad = Some(create_vad(&options.voice_config.vad));
                    }
                    if cached_punct.is_none() && options.voice_config.auto_punctuation {
                        cached_punct = Some(PunctuationRestorer::with_custom_path(
                            options.voice_config.punct_model_path.as_deref(),
                        ));
                    }

                    let (Some(recognizer), Some(vad)) =
                        (cached_recognizer.as_mut(), cached_vad.as_mut())
                    else {
                        warn!("[语音会话] 语音识别器或 VAD 模型未正确初始化，跳过本次录音");
                        is_active.store(false, Ordering::Release);
                        continue;
                    };
                    recognizer.reset();
                    vad.reset();
                    visualizer.reset();

                    // 2. 启动硬件麦克风捕获流（支持指定设备与热插拔优雅回退）
                    let (pcm_tx, pcm_rx) = sync_channel::<Vec<f32>>(32);
                    let capture = match AudioCapture::start_with_device(
                        pcm_tx,
                        options.voice_config.device_name.as_deref(),
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("[语音会话] 启动音频采集硬件失败: {}", e);
                            let _ = event_tx.send(VoiceEvent::Error(e.to_string()));
                            is_active.store(false, Ordering::Release);
                            continue;
                        }
                    };

                    info!("[语音会话] 录音捕获流与识别管道启动就绪，正在聆听用户语音...");
                    active_recording_loop(ActiveRecordingContext {
                        capture,
                        pcm_rx,
                        cmd_rx: &cmd_rx,
                        event_tx: &event_tx,
                        visualizer: &mut visualizer,
                        vad: vad.as_mut(),
                        recognizer: recognizer.as_mut(),
                        punct_restorer: cached_punct.as_ref(),
                        voice_config: &options.voice_config,
                        is_active: &is_active,
                    });
                }
                WorkerCommand::StopAndCommit | WorkerCommand::Cancel => {
                    is_active.store(false, Ordering::Release);
                }
                WorkerCommand::UnloadModels => {
                    cached_recognizer = None;
                    cached_vad = None;
                    cached_punct = None;
                    last_asr_fingerprint = None;
                    info!("[语音会话] 已卸载内存中缓存的语音识别、VAD与标点模型权重，完全释放常驻内存");
                }
                WorkerCommand::Shutdown => {
                    info!("[语音会话] 收到销毁指令，后台常驻语音工作线程平滑退出");
                    break;
                }
            }
        }

        is_active.store(false, Ordering::Release);
    }
}

impl Drop for VoiceSessionController {
    fn drop(&mut self) {
        self.is_active.store(false, Ordering::Release);
        if self.worker_handle.is_some() {
            let _ = self.cmd_tx.send(WorkerCommand::Shutdown);
            if let Some(handle) = self.worker_handle.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use self::pipeline::{
        extract_final_text, handle_stop_and_commit, process_recognition_feed,
        push_pre_speech_silence,
    };
    use self::types::{RecognitionFeedContext, StopCommitContext, PRE_SPEECH_BUFFER_CAPACITY};
    use super::*;
    use crate::afe::AudioFrontEndPipeline;
    use crate::recognizer::MockVoiceRecognizer;
    use std::collections::VecDeque;
    use waylan_core::VoiceConfig;

    #[test]
    fn test_voice_session_controller_degraded_mode() {
        // 手动构造一个工作线程派生失败时的降级控制器实例
        let (cmd_tx, _cmd_rx) = sync_channel::<WorkerCommand>(1);
        let mut controller = VoiceSessionController {
            is_active: Arc::new(AtomicBool::new(false)),
            cmd_tx,
            worker_handle: None,
        };

        // 验证处于降级模式时的各项接口安全表现
        assert!(
            !controller.is_recording(),
            "降级状态下录音状态应始终为 false"
        );

        let (event_tx, _event_rx) = sync_channel::<VoiceEvent>(4);
        let start_res = controller.start(VoiceSessionOptions::default(), event_tx);
        assert!(
            start_res.is_err(),
            "后台线程缺失时 start 应返回安全降级错误"
        );

        // 验证 stop 和 cancel 操作在降级模式下安全无副作用、不引发 panic
        controller.stop_and_commit();
        controller.cancel();
    }

    #[test]
    fn test_push_pre_speech_silence_capacity_limit() {
        let mut buffer = VecDeque::new();
        let chunk1 = vec![0.1; 2000];
        push_pre_speech_silence(&mut buffer, &chunk1);
        assert_eq!(buffer.len(), 2000);

        let chunk2 = vec![0.2; 4000];
        push_pre_speech_silence(&mut buffer, &chunk2);
        // 总计 6000 采样，超过 4800，应被限制在 PRE_SPEECH_BUFFER_CAPACITY (4800)
        assert_eq!(buffer.len(), PRE_SPEECH_BUFFER_CAPACITY);
    }

    #[test]
    fn test_process_recognition_feed_drains_buffer() {
        let mut recognizer = MockVoiceRecognizer::new();
        let mut pre_buffer = VecDeque::from(vec![0.05; 1000]);
        let mut last_partial = String::new();
        let (tx, _rx) = sync_channel(16);

        let chunk = vec![0.1; 500];
        let res = process_recognition_feed(
            RecognitionFeedContext {
                recognizer: &mut recognizer,
                pre_speech_buffer: &mut pre_buffer,
                last_partial_text: &mut last_partial,
                event_tx: &tx,
                real_time_preview: true,
            },
            &chunk,
        );

        assert!(res);
        // 预缓冲区应被全部取走注入识别器
        assert!(pre_buffer.is_empty());
    }

    #[test]
    fn test_extract_final_text_punctuation() {
        let itn_cfg = waylan_core::VoiceItnConfig::default();
        let mut recognizer = MockVoiceRecognizer::new();
        let text = extract_final_text(&mut recognizer, None, true, &itn_cfg);
        assert!(text.ends_with('。'));

        let mut recognizer2 = MockVoiceRecognizer::new();
        let text_no_punct = extract_final_text(&mut recognizer2, None, false, &itn_cfg);
        assert_eq!(text_no_punct, "微澜语音输入");
    }

    #[test]
    fn test_voice_session_controller_unload_models() {
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(4);
        let dummy_handle = thread::spawn(|| {});
        let controller = VoiceSessionController {
            is_active: Arc::new(AtomicBool::new(false)),
            cmd_tx,
            worker_handle: Some(dummy_handle),
        };

        controller.unload_models();
        // 验证通道已收到 UnloadModels 指令
        let received = cmd_rx.try_recv();
        assert!(matches!(received, Ok(WorkerCommand::UnloadModels)));
    }

    #[test]
    fn test_handle_stop_and_commit_drains_audio_and_flushes_silence() {
        let (pcm_tx, pcm_rx) = sync_channel::<Vec<f32>>(16);
        // 模拟松开按键瞬间，管道中积压了 2 帧有效音频
        pcm_tx.send(vec![0.1; 1600]).unwrap();
        pcm_tx.send(vec![0.2; 1600]).unwrap();

        let (event_tx, event_rx) = sync_channel::<VoiceEvent>(16);
        let mut recognizer = MockVoiceRecognizer::new();
        let voice_cfg = VoiceConfig::default();
        let mut afe = AudioFrontEndPipeline::new(&voice_cfg.afe);
        let mut pre_speech = VecDeque::new();
        let mut last_partial = String::new();
        let is_active = Arc::new(AtomicBool::new(true));

        let ctx = StopCommitContext {
            capture: None,
            pcm_rx: &pcm_rx,
            recognizer: &mut recognizer,
            afe: &mut afe,
            pre_speech_buffer: &mut pre_speech,
            last_partial_text: &mut last_partial,
            punct_restorer: None,
            voice_config: &voice_cfg,
            event_tx: &event_tx,
            is_active: &is_active,
        };

        handle_stop_and_commit(ctx);

        // 验证通道内积压音频已被彻底排空
        assert!(pcm_rx.try_recv().is_err(), "音频通道中的切片应已被全部排空");

        // 验证活跃标志已安全复位
        assert!(!is_active.load(Ordering::Acquire));

        // 验证成功收到包含文本的 FinalCommit 事件
        let mut final_commit_received = false;
        while let Ok(evt) = event_rx.try_recv() {
            if let VoiceEvent::FinalCommit(text) = evt {
                assert!(!text.is_empty(), "结算文本不应为空");
                final_commit_received = true;
            }
        }
        assert!(final_commit_received, "必须收到 FinalCommit 终态事件");
    }
}
