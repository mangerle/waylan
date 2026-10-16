//! IBus 语音录入事件轮询与流式断句保底提交
//!
//! # 设计原理
//! - **实现初衷**：统一处理语音录音状态查询、流式预览更新、VAD 自动分句以及手动/松开提交。
//! - **核心优势**：基于通道 recv_timeout 挂起等待终态结果，杜绝忙轮询；在网络或模型超时时自动启用流式预览保底上屏。
//! - **代价与局限**：极端超时保底可能缺乏端侧标点润色，但 100% 避免文字丢失。

use log::{error, info};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use super::core::IbusSessionCore;
use waylan_voice::VoiceEvent;

impl IbusSessionCore {
    /// 判断当前是否处于语音输入或语音展示激活状态
    pub fn is_voice_active(&self) -> bool {
        self.voice_controller.is_recording() || self.voice_display_text.is_some()
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    pub fn poll_voice_events(&mut self) -> Option<String> {
        let mut final_commit = None;
        if let Some(ref rx) = self.voice_event_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    VoiceEvent::WaveformUpdated { waveform, .. } => {
                        if self.voice_config.interaction.show_floating_bar {
                            self.voice_display_text = Some("正在聆听中...".to_string());
                            self.voice_waveform = Some(waveform);
                        } else {
                            self.voice_display_text = None;
                            self.voice_waveform = None;
                        }
                    }
                    VoiceEvent::PartialText(text) => {
                        if self.voice_config.interaction.real_time_preview {
                            self.voice_partial_text = Some(text);
                        } else {
                            self.voice_partial_text = None;
                        }
                    }
                    VoiceEvent::SegmentCommit(text) => {
                        final_commit = Some(text);
                        self.voice_partial_text = None;
                        break;
                    }
                    VoiceEvent::FinalCommit(text) => {
                        final_commit = Some(text);
                        self.clear_voice_state();
                        break;
                    }
                    VoiceEvent::Cancelled => {
                        self.clear_voice_state();
                        break;
                    }
                    VoiceEvent::Error(err) => {
                        error!("[微澜 IBus 语音] 识别发生异常: {}", err);
                        self.clear_voice_state();
                        break;
                    }
                }
            }
        }
        final_commit
    }

    /// 清空语音运行状态标志
    pub(crate) fn clear_voice_state(&mut self) {
        self.is_push_to_talk_active = false;
        self.push_to_talk_key = None;
        self.voice_display_text = None;
        self.voice_waveform = None;
        self.voice_partial_text = None;
        self.voice_event_rx = None;
    }

    /// 在指定截止时间内等待语音终态事件
    fn wait_for_voice_completion(&mut self, deadline: Instant) -> Option<String> {
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if remaining.is_zero() {
                break;
            }
            if let Some(text) = self.poll_voice_events() {
                if !text.is_empty() {
                    return Some(text);
                }
            }
            let Some(ref rx) = self.voice_event_rx else {
                break;
            };
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(VoiceEvent::FinalCommit(text)) | Ok(VoiceEvent::SegmentCommit(text)) => {
                    self.clear_voice_state();
                    if !text.is_empty() {
                        return Some(text);
                    }
                    break;
                }
                Ok(VoiceEvent::PartialText(text)) => {
                    if self.voice_config.interaction.real_time_preview {
                        self.voice_partial_text = Some(text);
                    }
                }
                Ok(VoiceEvent::Cancelled) => {
                    self.clear_voice_state();
                    break;
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        None
    }

    /// 决策最终文字并执行流式保底降级
    fn resolve_commit_with_fallback(
        &mut self,
        final_text: Option<String>,
        fallback_preview: Option<String>,
    ) -> Option<String> {
        let commit_text = final_text.or_else(|| {
            let candidate = fallback_preview.or_else(|| self.voice_partial_text.clone());
            if let Some(preview) = candidate {
                let trimmed = preview.trim().to_string();
                if !trimmed.is_empty() {
                    info!(
                        "[微澜 IBus 语音] 最终结算文本为空，启用实时流式预览保底上屏: \"{}\"",
                        trimmed
                    );
                    return Some(trimmed);
                }
            }
            None
        });

        self.clear_voice_state();
        if let Some(ref text) = commit_text {
            info!("[微澜 IBus 语音] 成功生成文字上屏: \"{}\"", text);
        }
        commit_text
    }

    /// 停止语音录音并尽可能提取最终确认文本，包含流式预览保底与短时等待
    pub fn stop_voice_recording_and_commit(&mut self) -> Option<String> {
        let fallback_preview = self.voice_partial_text.clone();
        self.voice_controller.stop_and_commit();
        self.is_push_to_talk_active = false;
        self.push_to_talk_key = None;

        let mut final_text = self.poll_voice_events();
        if final_text.is_none() {
            // 将持锁阻塞时长严格限制在 30ms 以内，既能捕获快速完成的识别，又彻底杜绝主事件循环与 D-Bus 卡死
            let deadline = Instant::now() + Duration::from_millis(30);
            final_text = self.wait_for_voice_completion(deadline);
        }
        if final_text.is_none() {
            if let Some(text) = self.poll_voice_events() {
                if !text.is_empty() {
                    final_text = Some(text);
                }
            }
        }

        self.resolve_commit_with_fallback(final_text, fallback_preview)
    }
}
