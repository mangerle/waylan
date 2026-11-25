//! 语音录制与实时推理流水线处理逻辑

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::VoiceConfig;

pub(super) use super::commit::{
    extract_final_text, handle_stop_and_commit, process_recognition_feed, push_pre_speech_silence,
};
use super::types::{
    ActiveRecordingContext, PRE_SPEECH_BUFFER_CAPACITY, RecognitionFeedContext, StopCommitContext,
    VoiceEvent, WorkerCommand, clear_session_if_matches, send_critical_event,
};
use crate::afe::AudioFrontEndPipeline;
use crate::capture::AudioCapture;
use crate::vad::calculate_rms;

/// 活跃录音主循环内部运行时状态缓存
struct ActiveRecordingState {
    /// 前端音频增强流水线（降噪/滤波/AGC）
    afe: AudioFrontEndPipeline,
    /// 上次刷新波形可视化条的时间戳
    last_visual_time: Instant,
    /// 上次流式识别的部分文本缓存
    last_partial_text: String,
    /// 当前录音会话启动时间戳
    session_start_time: Instant,
    /// 起音前静音样本环形预缓冲区
    pre_speech_buffer: VecDeque<f32>,
}

impl ActiveRecordingState {
    /// 创建并初始化录音流水线状态
    fn new(voice_config: &VoiceConfig) -> Self {
        Self {
            afe: AudioFrontEndPipeline::new(&voice_config.afe),
            last_visual_time: Instant::now(),
            last_partial_text: String::new(),
            session_start_time: Instant::now(),
            pre_speech_buffer: VecDeque::with_capacity(PRE_SPEECH_BUFFER_CAPACITY),
        }
    }
}

/// 录音循环接收外部命令后的控制流决策
enum RecordingCommandOutcome {
    /// 继续录音
    Continue,
    /// 结束本次录音（Stop/Cancel/Unload），但保持后台工作线程就绪待命
    StopRecording,
    /// 收到全局销毁指令，彻底终止录音并终止常驻工作线程
    Shutdown,
}

/// 轮询外部控制命令并返回录音控制流决策
fn poll_recording_command(
    ctx: &mut ActiveRecordingContext<'_>,
    state: &mut ActiveRecordingState,
) -> RecordingCommandOutcome {
    match ctx.cmd_rx.try_recv() {
        Ok(cmd) => handle_recording_command(cmd, state, ctx),
        Err(std::sync::mpsc::TryRecvError::Empty) => RecordingCommandOutcome::Continue,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            info!("[语音会话] 控制命令通道已断开，主动结束录音会话");
            drop(ctx.capture.take());
            RecordingCommandOutcome::StopRecording
        }
    }
}

/// 接收并处理一块重采样后的 PCM 音频切片，返回是否继续维持录音循环
fn poll_pcm_chunk(ctx: &mut ActiveRecordingContext<'_>, state: &mut ActiveRecordingState) -> bool {
    match ctx.pcm_rx.recv_timeout(Duration::from_millis(50)) {
        Ok(samples) => process_pcm_chunk(samples, state, ctx),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            !check_stream_dead(&ctx.capture, ctx.event_tx)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            info!("[语音会话] 麦克风音频流已断开");
            false
        }
    }
}

/// 活跃录音与实时推理循环
pub(super) fn active_recording_loop(mut ctx: ActiveRecordingContext<'_>) -> bool {
    let mut state = ActiveRecordingState::new(ctx.voice_config);
    let mut is_shutdown = false;

    loop {
        if ctx.shutdown_requested.load(Ordering::Acquire) {
            info!("[语音会话] 录音过程中收到全局停机请求，立即释放音频采集并退出");
            drop(ctx.capture.take());
            is_shutdown = true;
            break;
        }

        // 1. 优先检查外部主动停止、提交或取消命令
        match poll_recording_command(&mut ctx, &mut state) {
            RecordingCommandOutcome::Continue => {}
            RecordingCommandOutcome::StopRecording => break,
            RecordingCommandOutcome::Shutdown => {
                is_shutdown = true;
                break;
            }
        }

        // 2. 接收重采样后的 16kHz PCM 音频切片（超时 50ms 防止阻塞）
        if !poll_pcm_chunk(&mut ctx, &mut state) {
            break;
        }
    }

    clear_session_if_matches(ctx.active_session_id, ctx.session_id);
    info!("[语音会话] 录音会话结束，音频捕获硬件已释放");
    is_shutdown
}

/// 释放音频采集句柄，并清空前置静音缓冲与识别器内部状态
fn release_recording_resources(
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) {
    drop(ctx.capture.take());
    state.pre_speech_buffer.clear();
    ctx.recognizer.reset();
}

/// 处理来自控制通道的外部指令，返回控制流决策
fn handle_recording_command(
    cmd: WorkerCommand,
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) -> RecordingCommandOutcome {
    match cmd {
        WorkerCommand::StopAndCommit { session_id } if session_id == ctx.session_id => {
            let commit_ctx = StopCommitContext {
                session_id: ctx.session_id,
                capture: ctx.capture.take(),
                pcm_rx: &ctx.pcm_rx,
                recognizer: ctx.recognizer,
                afe: &mut state.afe,
                pre_speech_buffer: &mut state.pre_speech_buffer,
                last_partial_text: &mut state.last_partial_text,
                voice_config: ctx.voice_config,
                event_tx: ctx.event_tx,
                active_session_id: ctx.active_session_id,
            };
            handle_stop_and_commit(commit_ctx);
            RecordingCommandOutcome::StopRecording
        }
        WorkerCommand::Cancel { session_id } if session_id == ctx.session_id => {
            info!("[语音会话] 收到外部取消指令，放弃本次录音");
            release_recording_resources(state, ctx);
            let _ = send_critical_event(ctx.event_tx, VoiceEvent::Cancelled);
            clear_session_if_matches(ctx.active_session_id, ctx.session_id);
            RecordingCommandOutcome::StopRecording
        }
        WorkerCommand::StopAndCommit { session_id } | WorkerCommand::Cancel { session_id } => {
            warn!(
                "[语音会话] 忽略与当前会话编号不匹配的过期控制指令: cmd_session={}, current={}",
                session_id, ctx.session_id
            );
            RecordingCommandOutcome::Continue
        }
        WorkerCommand::Shutdown => {
            info!("[语音会话] 录音过程中收到销毁指令，准备彻底退出语音工作线程");
            release_recording_resources(state, ctx);
            clear_session_if_matches(ctx.active_session_id, ctx.session_id);
            RecordingCommandOutcome::Shutdown
        }
        WorkerCommand::UnloadModels => {
            info!("[语音会话] 录音过程中收到卸载指令，中止当前录音并退出");
            release_recording_resources(state, ctx);
            clear_session_if_matches(ctx.active_session_id, ctx.session_id);
            RecordingCommandOutcome::StopRecording
        }
        WorkerCommand::Start { .. } => {
            // 录音进行期间忽略重复 Start
            RecordingCommandOutcome::Continue
        }
    }
}

/// 刷新动态波形条并上报音量数据，返回外部事件通道是否仍然可用
fn refresh_waveform_visual(
    rms: f32,
    now: Instant,
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) -> bool {
    if !ctx.voice_config.interaction.show_floating_bar
        || now.duration_since(state.last_visual_time) < Duration::from_millis(80)
    {
        return true;
    }

    let wave = ctx.visualizer.update(rms);
    if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
        ctx.event_tx.try_send(VoiceEvent::WaveformUpdated {
            waveform: wave,
            volume: rms,
        })
    {
        info!("[语音会话] 外部事件通道已关闭，退出录音循环");
        return false;
    }
    state.last_visual_time = now;
    true
}

/// 喂给 VAD 进行端点与人声活跃度判定，并按判定结果驱动识别推理或静音抑制
fn feed_vad_and_recognition(
    samples: &[f32],
    now: Instant,
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) -> bool {
    // 喂给 VAD 进行端点与人声活跃度判定
    let should_finish = ctx.vad.feed_chunk(samples, now);

    if ctx.vad.is_speech_started() {
        let feed_ctx = RecognitionFeedContext {
            recognizer: ctx.recognizer,
            pre_speech_buffer: &mut state.pre_speech_buffer,
            last_partial_text: &mut state.last_partial_text,
            event_tx: ctx.event_tx,
            real_time_preview: ctx.voice_config.interaction.real_time_preview,
        };
        if !process_recognition_feed(feed_ctx, samples) {
            return false;
        }
    } else {
        // 静音抑制期：麦克风切片仅存入前置环形队列，完全跳过 ASR 推理以极度节省 CPU 算力
        push_pre_speech_silence(&mut state.pre_speech_buffer, samples);
    }

    // 录音开启前 500ms 内不触发 VAD 自动截断，防止环境初始微弱底噪导致瞬间误切
    if should_finish
        && now.duration_since(state.session_start_time) >= Duration::from_millis(500)
        && !handle_vad_segment(state, ctx)
    {
        return false;
    }

    true
}

/// 处理接收到的单块 16kHz PCM 音频切片，返回是否继续录音
fn process_pcm_chunk(
    mut samples: Vec<f32>,
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) -> bool {
    // 执行前端音频增强流水线（80Hz高通滤波 + 人声智能降噪 + 动态数字AGC）
    state.afe.process(&mut samples);

    let now = Instant::now();
    let rms = calculate_rms(&samples);

    // 刷新动态波形条（约 80ms 间隔，仅在开启浮动条时上报）
    let mut keep_running = refresh_waveform_visual(rms, now, state, ctx);
    if keep_running {
        keep_running = feed_vad_and_recognition(&samples, now, state, ctx);
    }

    // 处理完毕后清空并归还缓冲区至音频采集线程复用池，避免实时音频回调持续分配内存
    samples.clear();
    let _ = ctx.recycle_tx.try_send(samples);
    keep_running
}

/// 处理 VAD 检测到的人声停顿自动断句提交，返回是否继续录音
fn handle_vad_segment(
    state: &mut ActiveRecordingState,
    ctx: &mut ActiveRecordingContext<'_>,
) -> bool {
    info!("[语音会话] VAD 检测到人声停顿并超时，分句自动断句上屏提交（保持持续听写）");
    match extract_final_text(
        ctx.recognizer,
        ctx.voice_config.auto_punctuation,
        &ctx.voice_config.itn,
    ) {
        Ok(seg_text) if !seg_text.is_empty() => {
            info!("[语音会话] 自动断句分句上屏结果: \"{}\"", seg_text);
            if !send_critical_event(ctx.event_tx, VoiceEvent::SegmentCommit(seg_text)) {
                info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                return false;
            }
        }
        Ok(_) => {}
        Err(e) => {
            warn!("[语音会话] 分句识别结算失败: {}", e);
            if !send_critical_event(ctx.event_tx, VoiceEvent::Error(e.to_string())) {
                info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                return false;
            }
        }
    }
    ctx.recognizer.reset();
    ctx.vad.reset();
    state.afe.reset();
    state.pre_speech_buffer.clear();
    state.last_partial_text.clear();
    true
}

/// 检查底层麦克风音频流是否发生异常中断，若中断则通知外部事件通道并返回 true
fn check_stream_dead(
    capture_opt: &Option<AudioCapture>,
    event_tx: &SyncSender<VoiceEvent>,
) -> bool {
    if capture_opt
        .as_ref()
        .map(|c| c.is_stream_dead())
        .unwrap_or(false)
    {
        warn!("[语音会话] 检测到底层麦克风音频流已异常中断（设备可能已拔出），立即终止会话");
        let _ = send_critical_event(
            event_tx,
            VoiceEvent::Error("麦克风音频设备已断开".to_string()),
        );
        true
    } else {
        false
    }
}
