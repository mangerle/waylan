//! 语音录制停止结算、流式音频喂入与文本提取规整
//!
//! # 设计原理
//! - **实现初衷**：将麦克风硬件停止后的在途 PCM 排空、静音补充、尾部标点恢复与 ITN 文本规整逻辑
//!   从录音主循环解耦。
//! - **核心优势**：职责清晰，将异步录音流与结束阶段的同步收尾清算分离，提升模块内聚度与可测性。
//! - **代价与局限**：跨文件调用需传递上下文借用参数。

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use log::{debug, info, warn};

use super::types::{
    PRE_SPEECH_BUFFER_CAPACITY, RecognitionFeedContext, StopCommitContext, VoiceEvent,
};
use crate::recognizer::VoiceRecognizer;

/// 处理主动停止录音与优雅排空提交
pub(super) fn handle_stop_and_commit(mut ctx: StopCommitContext<'_>) {
    info!("[语音会话] 收到外部主动提交指令，正在执行音频管道排空与整句结算");

    // 1. 排空麦克风硬件驱动与管道中的在途音频，并释放音频捕获硬件
    drain_pending_audio(&mut ctx);

    // 2. 若此前尚未检测到人声起音（短句），将预存的静音缓冲倒出送入识别器
    if !ctx.pre_speech_buffer.is_empty() {
        let buffered: Vec<f32> = ctx.pre_speech_buffer.drain(..).collect();
        let _ = ctx.recognizer.feed_pcm(&buffered);
    }

    // 3. 注入约 200ms 的尾部静音 padding（3200 个 16kHz 零采样点）
    const TRAILING_SILENCE_SAMPLES: usize = 16_000 * 200 / 1000;
    let silence = vec![0.0f32; TRAILING_SILENCE_SAMPLES];
    feed_recognition(&mut ctx, &silence);

    // 4. 调用 finish 提取最终整句识别文字与标点预测
    let mut final_text = extract_final_text(
        ctx.recognizer,
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

/// 排空管道中残留的在途音频并彻底终止麦克风硬件录入
fn drain_pending_audio(ctx: &mut StopCommitContext<'_>) {
    // 1. 等待并排空麦克风硬件驱动与管道中的在途音频（最长 80ms，单次 40ms 无新数据即快速跳出）
    let drain_start = Instant::now();
    while drain_start.elapsed() < Duration::from_millis(80) {
        match ctx.pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(mut samples) => {
                ctx.afe.process(&mut samples);
                feed_recognition(ctx, &samples);
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
        feed_recognition(ctx, &samples);
    }
}

/// 将音频采样喂入识别器并推进流式解码
fn feed_recognition(ctx: &mut StopCommitContext<'_>, samples: &[f32]) {
    let _ = process_recognition_feed(
        RecognitionFeedContext {
            recognizer: ctx.recognizer,
            pre_speech_buffer: ctx.pre_speech_buffer,
            last_partial_text: ctx.last_partial_text,
            event_tx: ctx.event_tx,
            real_time_preview: ctx.voice_config.interaction.real_time_preview,
        },
        samples,
    );
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
                if ctx.real_time_preview
                    && let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                        ctx.event_tx.try_send(VoiceEvent::PartialText(partial))
                {
                    info!("[语音会话] 外部事件通道已关闭，退出录音循环");
                    return false;
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

/// 提取最终整句识别文字并按需进行 ITN 规整
pub(super) fn extract_final_text(
    recognizer: &mut dyn VoiceRecognizer,
    auto_punctuation: bool,
    itn_config: &waylan_core::VoiceItnConfig,
) -> String {
    let raw_text = recognizer.finish().unwrap_or_default();
    if raw_text.is_empty() {
        return String::new();
    }
    // SenseVoice-Small 与 Fun-ASR-Nano 端到端原生自带标点；
    // 若用户显式关闭自动标点，则滤除标点符号直接输出纯字符
    let final_text = if auto_punctuation {
        raw_text
    } else {
        raw_text
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    '。' | '，' | '！' | '？' | '；' | '：' | ',' | '.' | '?' | '!' | ';' | ':'
                )
            })
            .collect()
    };

    crate::itn::normalize_voice_text(&final_text, itn_config)
}
