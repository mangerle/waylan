//! 语音输入生命周期会话控制器
//!
//! # 设计原理
//! - **实现初衷**：统一纳管音频捕获流、端点静音检测、Unicode 波形生成与识别引擎管道。
//! - **核心优势**：神经网络模型（Paraformer 与 Silero VAD）在后台专用工作线程常驻复用，避免每次录音重复加载
//!   高达 200MB 的模型权重文件；消除近 1 秒的冷启动延迟，实现快捷键按下后 0 延迟秒开录音与识别。
//! - **代价与局限**：后台常驻线程需通过命令通道安全纳管状态流转，并在 Drop 时平滑停机。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::VoiceConfig;

use crate::afe::AudioFrontEndPipeline;
use crate::capture::AudioCapture;
use crate::error::VoiceError;
use crate::recognizer::{create_recognizer, PunctuationRestorer, VoiceRecognizer};
use crate::vad::{calculate_rms, create_vad, VadProcessor, WaveformVisualizer};

/// 起音前置环形预缓冲区容量（以 16kHz 单声道采样点计）：300ms = 4800 采样点
/// 用于在 VAD 判定为说话前的静音抑制期间暂存最近 300ms 音频，确保起音瞬间不被切断微弱辅音声母
const PRE_SPEECH_BUFFER_CAPACITY: usize = 16_000 * 300 / 1000;

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
enum WorkerCommand {
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

        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                WorkerCommand::Start { options, event_tx } => {
                    // 若在工作线程处理前主线程已触发取消，直接忽略本次启动
                    if !is_active.load(Ordering::Acquire) {
                        info!("[语音会话] 录音启动前已被主线程取消，跳过本次录制");
                        continue;
                    }

                    // 1. 若配置中引擎类型、目标服务、云端凭据或离线模型目录发生变更，主动失效识别器缓存以实现热切换
                    let current_fingerprint = Self::compute_asr_fingerprint(&options.voice_config);
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

                    let recognizer = cached_recognizer.as_mut().unwrap();
                    let vad = cached_vad.as_mut().unwrap();
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
                    Self::active_recording_loop(
                        capture,
                        pcm_rx,
                        &cmd_rx,
                        &event_tx,
                        &mut visualizer,
                        vad.as_mut(),
                        recognizer.as_mut(),
                        cached_punct.as_ref(),
                        &options.voice_config,
                        &is_active,
                    );
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

    /// 活跃录音与实时推理循环
    #[allow(clippy::too_many_arguments)]
    fn active_recording_loop(
        _capture: AudioCapture,
        pcm_rx: Receiver<Vec<f32>>,
        cmd_rx: &Receiver<WorkerCommand>,
        event_tx: &SyncSender<VoiceEvent>,
        visualizer: &mut WaveformVisualizer,
        vad: &mut dyn VadProcessor,
        recognizer: &mut dyn VoiceRecognizer,
        punct_restorer: Option<&PunctuationRestorer>,
        voice_config: &VoiceConfig,
        is_active: &Arc<AtomicBool>,
    ) {
        let mut afe = AudioFrontEndPipeline::new(&voice_config.afe);
        let mut last_visual_time = Instant::now();
        let mut last_partial_text = String::new();
        let session_start_time = Instant::now();
        let mut pre_speech_buffer: VecDeque<f32> =
            VecDeque::with_capacity(PRE_SPEECH_BUFFER_CAPACITY);

        loop {
            if !is_active.load(Ordering::Acquire) {
                info!("[语音会话] 检测到外部会话已取消或关闭，平滑退出录音循环");
                break;
            }

            // 1. 检查外部主动停止或取消命令
            if let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    WorkerCommand::StopAndCommit => {
                        info!("[语音会话] 收到外部主动提交指令，正在生成整句文字");
                        if !pre_speech_buffer.is_empty() {
                            let buffered: Vec<f32> = pre_speech_buffer.drain(..).collect();
                            let _ = recognizer.feed_pcm(&buffered);
                        }
                        let final_text = Self::extract_final_text(
                            recognizer,
                            punct_restorer,
                            voice_config.auto_punctuation,
                            &voice_config.itn,
                        );
                        info!("[语音会话] 主动提交识别结果: \"{}\"", final_text);
                        let _ = event_tx.send(VoiceEvent::FinalCommit(final_text));
                        break;
                    }
                    WorkerCommand::Cancel => {
                        info!("[语音会话] 收到外部取消指令，放弃本次录音");
                        pre_speech_buffer.clear();
                        recognizer.reset();
                        let _ = event_tx.send(VoiceEvent::Cancelled);
                        break;
                    }
                    WorkerCommand::Shutdown => {
                        pre_speech_buffer.clear();
                        recognizer.reset();
                        break;
                    }
                    WorkerCommand::UnloadModels => {
                        info!("[语音会话] 录音过程中收到卸载指令，中止当前录音并退出");
                        pre_speech_buffer.clear();
                        recognizer.reset();
                        is_active.store(false, Ordering::Release);
                        break;
                    }
                    WorkerCommand::Start { .. } => {
                        // 录音期间忽略重复 Start
                    }
                }
            }

            // 2. 接收重采样后的 16kHz PCM 音频切片（超时 50ms 防止阻塞）
            match pcm_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(mut samples) => {
                    // 执行前端音频增强流水线（80Hz高通滤波 + 人声智能降噪 + 动态数字AGC）
                    afe.process(&mut samples);

                    let now = Instant::now();
                    let rms = calculate_rms(&samples);

                    // 刷新动态波形条（约 80ms 间隔，仅在开启浮动条时上报）
                    if voice_config.interaction.show_floating_bar
                        && now.duration_since(last_visual_time) >= Duration::from_millis(80)
                    {
                        let wave = visualizer.update(rms);
                        if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) = event_tx
                            .try_send(VoiceEvent::WaveformUpdated {
                                waveform: wave,
                                volume: rms,
                            })
                        {
                            info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                            break;
                        }
                        last_visual_time = now;
                    }

                    // 喂给 VAD 进行端点与人声活跃度判定
                    let should_finish = vad.feed_chunk(&samples, now);

                    // 静音算力抑制与起音回溯门禁机制：
                    // 若用户尚未开口说话，则跳过耗费算力的 ASR 推理，仅将音频暂存于 300ms 环形预缓冲区；
                    // 一旦检测到人声起音，将预缓冲区内的前置辅音连同当前音频一并送入 ASR 激活解码。
                    if vad.is_speech_started() {
                        let should_continue = Self::process_recognition_feed(
                            recognizer,
                            &samples,
                            &mut pre_speech_buffer,
                            &mut last_partial_text,
                            event_tx,
                            voice_config.interaction.real_time_preview,
                        );
                        if !should_continue {
                            break;
                        }
                    } else {
                        // 静音抑制期：麦克风切片仅存入前置环形队列，完全跳过 ASR 推理以极度节省 CPU 算力
                        Self::push_pre_speech_silence(&mut pre_speech_buffer, &samples);
                    }

                    // 录音开启前 500ms 内不触发 VAD 自动截断，防止环境初始微弱底噪导致瞬间误切
                    if should_finish
                        && now.duration_since(session_start_time) >= Duration::from_millis(500)
                    {
                        info!("[语音会话] VAD 检测到人声停顿并超时，分句自动断句上屏提交（保持持续听写）");
                        let seg_text = Self::extract_final_text(
                            recognizer,
                            punct_restorer,
                            voice_config.auto_punctuation,
                            &voice_config.itn,
                        );
                        if !seg_text.is_empty() {
                            info!("[语音会话] 自动断句分句上屏结果: \"{}\"", seg_text);
                            if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                                event_tx.try_send(VoiceEvent::SegmentCommit(seg_text))
                            {
                                info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                                break;
                            }
                        }
                        recognizer.reset();
                        vad.reset();
                        afe.reset();
                        pre_speech_buffer.clear();
                        last_partial_text.clear();
                        // 注意：不 break，录音循环继续运行，等待下一句说话！
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 空闲时继续监听控制命令
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    info!("[语音会话] 麦克风音频流已断开");
                    break;
                }
            }
        }

        is_active.store(false, Ordering::Release);
        info!("[语音会话] 录音会话结束，音频捕获硬件已释放");
    }

    /// 将静音音频采样压入前置环形队列，并维持最大容量限制
    #[inline]
    fn push_pre_speech_silence(buffer: &mut VecDeque<f32>, samples: &[f32]) {
        buffer.extend(samples.iter().copied());
        if buffer.len() > PRE_SPEECH_BUFFER_CAPACITY {
            let excess = buffer.len() - PRE_SPEECH_BUFFER_CAPACITY;
            buffer.drain(0..excess);
        }
    }

    /// 推进 ASR 解码流并向外部通道发送流式文字
    ///
    /// # 返回值
    /// - `true`: 继续录音循环；
    /// - `false`: 外部通道已断开或遇到致命异常，应退出录音循环。
    fn process_recognition_feed(
        recognizer: &mut dyn VoiceRecognizer,
        samples: &[f32],
        pre_speech_buffer: &mut VecDeque<f32>,
        last_partial_text: &mut String,
        event_tx: &SyncSender<VoiceEvent>,
        real_time_preview: bool,
    ) -> bool {
        let feed_result = if !pre_speech_buffer.is_empty() {
            let mut combined = Vec::with_capacity(pre_speech_buffer.len() + samples.len());
            combined.extend(pre_speech_buffer.drain(..));
            combined.extend_from_slice(samples);
            recognizer.feed_pcm(&combined)
        } else {
            recognizer.feed_pcm(samples)
        };

        match feed_result {
            Ok(Some(partial)) => {
                if !partial.is_empty() && partial != *last_partial_text {
                    info!("[语音会话] 实时流式识别出字: \"{}\"", partial);
                    *last_partial_text = partial.clone();
                    if real_time_preview {
                        if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                            event_tx.try_send(VoiceEvent::PartialText(partial))
                        {
                            info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                            return false;
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!("[语音会话] 语音识别推理遇到异常: {}", e);
                let _ = event_tx.try_send(VoiceEvent::Error(e.to_string()));
                return false;
            }
        }

        true
    }

    /// 提取最终整句识别文字并按需自动预测补全标点符号及进行 ITN 规整
    fn extract_final_text(
        recognizer: &mut dyn VoiceRecognizer,
        punct_restorer: Option<&PunctuationRestorer>,
        auto_punctuation: bool,
        itn_config: &waylan_core::VoiceItnConfig,
    ) -> String {
        let raw_text = recognizer.finish().unwrap_or_default();
        if raw_text.is_empty() {
            return String::new();
        }
        let punct_text = if !auto_punctuation {
            raw_text
        } else if let Some(restorer) = punct_restorer {
            restorer.restore(&raw_text)
        } else {
            let mut fallback = raw_text;
            if !fallback.ends_with('。')
                && !fallback.ends_with('，')
                && !fallback.ends_with('！')
                && !fallback.ends_with('？')
            {
                fallback.push('。');
            }
            fallback
        };

        // 阶段五：文本规整与语气词过滤 (ITN)
        crate::itn::normalize_voice_text(&punct_text, itn_config)
    }

    /// 提取当前语音识别器的关键配置指纹，用于识别器缓存失效判定
    ///
    /// # 设计原理
    /// - **实现初衷**：在单次会话启动时比对当前生效的识别器核心参数（引擎类型、选中的 Profile、实际云端配置与离线模型路径）。
    /// - **核心优势**：只要用户在设置界面变更了云端 API Key、接口地址、切换了候选服务或离线目录，即刻触发热切换重建，无需重启进程。
    /// - **代价与局限**：仅比对影响识别器初始化的关键元数据，开销极小（纳秒级元组比对）。
    fn compute_asr_fingerprint(
        cfg: &VoiceConfig,
    ) -> (
        waylan_core::VoiceEngineType,
        Option<String>,
        Option<waylan_core::CloudAsrConfig>,
        Option<String>,
    ) {
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
    use super::*;
    use crate::recognizer::MockVoiceRecognizer;

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
        VoiceSessionController::push_pre_speech_silence(&mut buffer, &chunk1);
        assert_eq!(buffer.len(), 2000);

        let chunk2 = vec![0.2; 4000];
        VoiceSessionController::push_pre_speech_silence(&mut buffer, &chunk2);
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
        let res = VoiceSessionController::process_recognition_feed(
            &mut recognizer,
            &chunk,
            &mut pre_buffer,
            &mut last_partial,
            &tx,
            true,
        );

        assert!(res);
        // 预缓冲区应被全部取走注入识别器
        assert!(pre_buffer.is_empty());
    }

    #[test]
    fn test_extract_final_text_punctuation() {
        let itn_cfg = waylan_core::VoiceItnConfig::default();
        let mut recognizer = MockVoiceRecognizer::new();
        let text =
            VoiceSessionController::extract_final_text(&mut recognizer, None, true, &itn_cfg);
        assert!(text.ends_with('。'));

        let mut recognizer2 = MockVoiceRecognizer::new();
        let text_no_punct =
            VoiceSessionController::extract_final_text(&mut recognizer2, None, false, &itn_cfg);
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
}
