//! 语音录制与实时推理流水线处理逻辑

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use waylan_core::VoiceConfig;

use super::types::{
    ActiveRecordingContext, RecognitionFeedContext, StopCommitContext, VoiceEvent, WorkerCommand,
    PRE_SPEECH_BUFFER_CAPACITY,
};
use crate::afe::AudioFrontEndPipeline;
use crate::recognizer::{PunctuationRestorer, VoiceRecognizer};
use crate::vad::calculate_rms;

/// 活跃录音与实时推理循环
pub(super) fn active_recording_loop(ctx: ActiveRecordingContext<'_>) {
    let ActiveRecordingContext {
        capture: _capture,
        pcm_rx,
        cmd_rx,
        event_tx,
        visualizer,
        vad,
        recognizer,
        punct_restorer,
        voice_config,
        is_active,
    } = ctx;
    let mut afe = AudioFrontEndPipeline::new(&voice_config.afe);
    let mut last_visual_time = Instant::now();
    let mut last_partial_text = String::new();
    let session_start_time = Instant::now();
    let mut pre_speech_buffer: VecDeque<f32> = VecDeque::with_capacity(PRE_SPEECH_BUFFER_CAPACITY);
    let mut capture_opt = Some(_capture);

    loop {
        // 1. 优先检查外部主动停止、提交或取消命令，确保指令不因 is_active 竞态而被跳过
        if let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                WorkerCommand::StopAndCommit => {
                    let ctx = StopCommitContext {
                        capture: capture_opt.take(),
                        pcm_rx: &pcm_rx,
                        recognizer,
                        afe: &mut afe,
                        pre_speech_buffer: &mut pre_speech_buffer,
                        last_partial_text: &mut last_partial_text,
                        punct_restorer,
                        voice_config,
                        event_tx,
                        is_active,
                    };
                    handle_stop_and_commit(ctx);
                    break;
                }
                WorkerCommand::Cancel => {
                    info!("[语音会话] 收到外部取消指令，放弃本次录音");
                    drop(capture_opt.take());
                    pre_speech_buffer.clear();
                    recognizer.reset();
                    let _ = event_tx.send(VoiceEvent::Cancelled);
                    is_active.store(false, Ordering::Release);
                    break;
                }
                WorkerCommand::Shutdown => {
                    drop(capture_opt.take());
                    pre_speech_buffer.clear();
                    recognizer.reset();
                    is_active.store(false, Ordering::Release);
                    break;
                }
                WorkerCommand::UnloadModels => {
                    info!("[语音会话] 录音过程中收到卸载指令，中止当前录音并退出");
                    drop(capture_opt.take());
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

        if !is_active.load(Ordering::Acquire) {
            info!("[语音会话] 检测到外部会话已取消或关闭，平滑退出录音循环");
            break;
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
                    if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                        event_tx.try_send(VoiceEvent::WaveformUpdated {
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
                    let should_continue = process_recognition_feed(
                        RecognitionFeedContext {
                            recognizer,
                            pre_speech_buffer: &mut pre_speech_buffer,
                            last_partial_text: &mut last_partial_text,
                            event_tx,
                            real_time_preview: voice_config.interaction.real_time_preview,
                        },
                        &samples,
                    );
                    if !should_continue {
                        break;
                    }
                } else {
                    // 静音抑制期：麦克风切片仅存入前置环形队列，完全跳过 ASR 推理以极度节省 CPU 算力
                    push_pre_speech_silence(&mut pre_speech_buffer, &samples);
                }

                // 录音开启前 500ms 内不触发 VAD 自动截断，防止环境初始微弱底噪导致瞬间误切
                if should_finish
                    && now.duration_since(session_start_time) >= Duration::from_millis(500)
                {
                    info!(
                        "[语音会话] VAD 检测到人声停顿并超时，分句自动断句上屏提交（保持持续听写）"
                    );
                    let seg_text = extract_final_text(
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
                // 空闲时检查底层硬件流是否因设备拔出或严重错误而死亡
                if capture_opt
                    .as_ref()
                    .map(|c| c.is_stream_dead())
                    .unwrap_or(false)
                {
                    warn!("[语音会话] 检测到底层麦克风音频流已异常中断（设备可能已拔出），立即终止会话");
                    let _ = event_tx.send(VoiceEvent::Error("麦克风音频设备已断开".to_string()));
                    break;
                }
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

/// 处理主动停止录音与优雅排空提交
pub(super) fn handle_stop_and_commit(mut ctx: StopCommitContext<'_>) {
    info!("[语音会话] 收到外部主动提交指令，正在执行音频管道排空与整句结算");

    // 1. 等待并排空麦克风硬件驱动与管道中的在途音频（最长 80ms，单次 40ms 无新数据即快速跳出）
    let drain_start = Instant::now();
    while drain_start.elapsed() < Duration::from_millis(80) {
        match ctx.pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(mut samples) => {
                ctx.afe.process(&mut samples);
                let _ = process_recognition_feed(
                    RecognitionFeedContext {
                        recognizer: ctx.recognizer,
                        pre_speech_buffer: ctx.pre_speech_buffer,
                        last_partial_text: ctx.last_partial_text,
                        event_tx: ctx.event_tx,
                        real_time_preview: ctx.voice_config.interaction.real_time_preview,
                    },
                    &samples,
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 2. 显式释放音频捕获硬件，彻底终止麦克风录入
    drop(ctx.capture.take());

    // 3. 将通道中可能剩余的任何切片彻底清空并处理
    while let Ok(mut samples) = ctx.pcm_rx.try_recv() {
        ctx.afe.process(&mut samples);
        let _ = process_recognition_feed(
            RecognitionFeedContext {
                recognizer: ctx.recognizer,
                pre_speech_buffer: ctx.pre_speech_buffer,
                last_partial_text: ctx.last_partial_text,
                event_tx: ctx.event_tx,
                real_time_preview: ctx.voice_config.interaction.real_time_preview,
            },
            &samples,
        );
    }

    // 若此前尚未检测到人声起音（短句），将预存的静音缓冲倒出送入识别器
    if !ctx.pre_speech_buffer.is_empty() {
        let buffered: Vec<f32> = ctx.pre_speech_buffer.drain(..).collect();
        let _ = ctx.recognizer.feed_pcm(&buffered);
    }

    // 4. 注入约 200ms 的尾部静音 padding（3200 个 16kHz 零采样点）
    const TRAILING_SILENCE_SAMPLES: usize = 16_000 * 200 / 1000;
    let silence = vec![0.0f32; TRAILING_SILENCE_SAMPLES];
    let _ = process_recognition_feed(
        RecognitionFeedContext {
            recognizer: ctx.recognizer,
            pre_speech_buffer: ctx.pre_speech_buffer,
            last_partial_text: ctx.last_partial_text,
            event_tx: ctx.event_tx,
            real_time_preview: ctx.voice_config.interaction.real_time_preview,
        },
        &silence,
    );

    // 5. 调用 finish 提取最终整句识别文字与标点预测
    let mut final_text = extract_final_text(
        ctx.recognizer,
        ctx.punct_restorer,
        ctx.voice_config.auto_punctuation,
        &ctx.voice_config.itn,
    );

    if final_text.is_empty() && !ctx.last_partial_text.is_empty() {
        info!(
            "[语音会话] finish 结果为空，采用实时流式识别出的预览文本保底: \"{}\"",
            ctx.last_partial_text
        );
        final_text = ctx.last_partial_text.clone();
    }

    info!("[语音会话] 主动提交识别结果: \"{}\"", final_text);
    let _ = ctx.event_tx.send(VoiceEvent::FinalCommit(final_text));
    ctx.is_active.store(false, Ordering::Release);
}

/// 将静音音频采样压入前置环形队列，并维持最大容量限制
#[inline]
pub(super) fn push_pre_speech_silence(buffer: &mut VecDeque<f32>, samples: &[f32]) {
    buffer.extend(samples.iter().copied());
    if buffer.len() > PRE_SPEECH_BUFFER_CAPACITY {
        let excess = buffer.len() - PRE_SPEECH_BUFFER_CAPACITY;
        buffer.drain(0..excess);
    }
}

/// 推进 ASR 解码流并向外部通道发送流式文字
pub(super) fn process_recognition_feed(ctx: RecognitionFeedContext<'_>, samples: &[f32]) -> bool {
    let feed_result = if !ctx.pre_speech_buffer.is_empty() {
        let mut combined = Vec::with_capacity(ctx.pre_speech_buffer.len() + samples.len());
        combined.extend(ctx.pre_speech_buffer.drain(..));
        combined.extend_from_slice(samples);
        ctx.recognizer.feed_pcm(&combined)
    } else {
        ctx.recognizer.feed_pcm(samples)
    };

    match feed_result {
        Ok(Some(partial)) => {
            if !partial.is_empty() && partial != *ctx.last_partial_text {
                debug!("[语音会话] 实时流式识别出字: \"{}\"", partial);
                *ctx.last_partial_text = partial.clone();
                if ctx.real_time_preview {
                    if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                        ctx.event_tx.try_send(VoiceEvent::PartialText(partial))
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
            let _ = ctx.event_tx.try_send(VoiceEvent::Error(e.to_string()));
            return false;
        }
    }

    true
}

/// 提取最终整句识别文字并按需自动预测补全标点符号及进行 ITN 规整
pub(super) fn extract_final_text(
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

    crate::itn::normalize_voice_text(&punct_text, itn_config)
}

/// 提取当前语音识别器的关键配置指纹，用于识别器缓存失效判定
pub(super) fn compute_asr_fingerprint(
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
