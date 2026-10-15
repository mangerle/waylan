//! 语音输入生命周期会话控制器
//!
//! # 设计原理
//! - **实现初衷**：统一纳管音频捕获流、端点静音检测、Unicode 波形生成与识别引擎管道。
//! - **核心优势**：神经网络模型（Paraformer 与 Silero VAD）在后台专用工作线程常驻复用，避免每次录音重复加载
//!   高达 200MB 的模型权重文件；消除近 1 秒的冷启动延迟，实现快捷键按下后 0 延迟秒开录音与识别。
//! - **代价与局限**：后台常驻线程需通过命令通道安全纳管状态流转，并在 Drop 时平滑停机。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::VoiceConfig;

use crate::capture::AudioCapture;
use crate::error::VoiceError;
use crate::recognizer::{create_recognizer, PunctuationRestorer, VoiceRecognizer};
use crate::vad::{calculate_rms, create_vad, VadProcessor, WaveformVisualizer};

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
    /// 创建会话控制器并拉起常驻工作线程
    pub fn new() -> Self {
        let is_active = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = sync_channel::<WorkerCommand>(16);

        let active_clone = Arc::clone(&is_active);
        let handle = thread::Builder::new()
            .name("waylan-voice-worker".to_string())
            .spawn(move || {
                Self::worker_main(cmd_rx, active_clone);
            })
            .expect("创建后台常驻语音工作线程必须成功");

        Self {
            is_active,
            cmd_tx,
            worker_handle: Some(handle),
        }
    }

    /// 当前是否正处于录音识别中
    pub fn is_recording(&self) -> bool {
        self.is_active.load(Ordering::Acquire)
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
        if self.is_recording() {
            self.is_active.store(false, Ordering::Release);
            let _ = self.cmd_tx.send(WorkerCommand::StopAndCommit);
        }
    }

    /// 取消当前录音并丢弃未完成的识别
    pub fn cancel(&mut self) {
        if self.is_recording() {
            self.is_active.store(false, Ordering::Release);
            let _ = self.cmd_tx.send(WorkerCommand::Cancel);
        }
    }

    /// 常驻工作线程主入口：纳管模型单例生命周期与会话状态机
    fn worker_main(cmd_rx: Receiver<WorkerCommand>, is_active: Arc<AtomicBool>) {
        let mut visualizer = WaveformVisualizer::new();
        let mut cached_vad: Option<Box<dyn VadProcessor>> = None;
        let mut cached_recognizer: Option<Box<dyn VoiceRecognizer>> = None;
        let mut cached_punct: Option<PunctuationRestorer> = None;
        let mut last_engine_type: Option<waylan_core::VoiceEngineType> = None;
        let mut last_cloud_endpoint: Option<String> = None;

        info!("[语音会话] 后台常驻语音工作线程启动完成，进入就绪待命状态");

        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                WorkerCommand::Start { options, event_tx } => {
                    // 若在工作线程处理前主线程已触发取消，直接忽略本次启动
                    if !is_active.load(Ordering::Acquire) {
                        info!("[语音会话] 录音启动前已被主线程取消，跳过本次录制");
                        continue;
                    }

                    // 1. 若配置中引擎类型或线上接口发生变更，主动失效识别器缓存以实现热切换
                    if last_engine_type != Some(options.voice_config.engine_type)
                        || last_cloud_endpoint.as_deref()
                            != Some(&options.voice_config.cloud.endpoint)
                    {
                        cached_recognizer = None;
                        last_engine_type = Some(options.voice_config.engine_type);
                        last_cloud_endpoint = Some(options.voice_config.cloud.endpoint.clone());
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
        let mut last_visual_time = Instant::now();
        let mut last_partial_text = String::new();
        let session_start_time = Instant::now();

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
                        let final_text = Self::extract_final_text(
                            recognizer,
                            punct_restorer,
                            voice_config.auto_punctuation,
                        );
                        info!("[语音会话] 主动提交识别结果: \"{}\"", final_text);
                        let _ = event_tx.send(VoiceEvent::FinalCommit(final_text));
                        break;
                    }
                    WorkerCommand::Cancel => {
                        info!("[语音会话] 收到外部取消指令，放弃本次录音");
                        recognizer.reset();
                        let _ = event_tx.send(VoiceEvent::Cancelled);
                        break;
                    }
                    WorkerCommand::Shutdown => {
                        recognizer.reset();
                        break;
                    }
                    WorkerCommand::Start { .. } => {
                        // 录音期间忽略重复 Start
                    }
                }
            }

            // 2. 接收重采样后的 16kHz PCM 音频切片（超时 50ms 防止阻塞）
            match pcm_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(samples) => {
                    let now = Instant::now();
                    let rms = calculate_rms(&samples);

                    // 刷新动态波形条（约 80ms 间隔）
                    if now.duration_since(last_visual_time) >= Duration::from_millis(80) {
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

                    // 喂给流式 ASR 识别引擎
                    match recognizer.feed_pcm(&samples) {
                        Ok(Some(partial)) => {
                            if !partial.is_empty() && partial != last_partial_text {
                                info!("[语音会话] 实时流式识别出字: \"{}\"", partial);
                                last_partial_text = partial.clone();
                                if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                                    event_tx.try_send(VoiceEvent::PartialText(partial))
                                {
                                    info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                                    break;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("[语音会话] 语音识别推理遇到异常: {}", e);
                            let _ = event_tx.try_send(VoiceEvent::Error(e.to_string()));
                            break;
                        }
                    }

                    // 录音开启前 500ms 内不触发 VAD 自动截断，防止环境初始微弱底噪导致瞬间误切
                    if now.duration_since(session_start_time) >= Duration::from_millis(500) {
                        let should_finish = vad.feed_chunk(&samples, now);
                        if should_finish {
                            info!("[语音会话] VAD 检测到人声停顿并超时，分句自动断句上屏提交（保持持续听写）");
                            let seg_text = Self::extract_final_text(
                                recognizer,
                                punct_restorer,
                                voice_config.auto_punctuation,
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
                            last_partial_text.clear();
                            // 注意：不 break，录音循环继续运行，等待下一句说话！
                        }
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

    /// 提取最终整句识别文字并按需自动预测补全标点符号
    fn extract_final_text(
        recognizer: &mut dyn VoiceRecognizer,
        punct_restorer: Option<&PunctuationRestorer>,
        auto_punctuation: bool,
    ) -> String {
        let raw_text = recognizer.finish().unwrap_or_default();
        if raw_text.is_empty() {
            return String::new();
        }
        if !auto_punctuation {
            return raw_text;
        }

        if let Some(restorer) = punct_restorer {
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
        }
    }
}

impl Drop for VoiceSessionController {
    fn drop(&mut self) {
        self.is_active.store(false, Ordering::Release);
        let _ = self.cmd_tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}
