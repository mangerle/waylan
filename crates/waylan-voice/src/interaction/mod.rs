//! 统一语音按键交互控制状态机与事件流管理器
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland 原生协议层与 IBus 协议适配层在长按对讲机 (Push-to-Talk)、
//!   开关点按 (Toggle)、按键防抖、控制键中断 (Esc/Enter) 及流式保底提交上的样板逻辑重复。
//! - **核心优势**：单一领域状态源；集中处理音频采集启动、通道接收、实时波形/预览文本提取与故障降级；
//!   保证不同显示服务与输入协议下语音交互行为的完全一致性。
//! - **代价与局限**：按键分发需提供 evdev 物理键码与修饰键掩码，对协议层的按键抽象有轻量依赖。

mod key_handler;
mod types;

use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use waylan_core::input::{ConsumedKeyTracker, VoiceProvider};
use waylan_core::{VoiceBoxState, VoiceConfig, VoiceEngineType};

use crate::session::{VoiceEvent, VoiceSessionController, VoiceSessionOptions};

pub use types::{VoiceKeyAction, VoiceSessionState};

/// 统一语音按键交互状态机与控制器 (Voice Interaction Manager)
pub struct VoiceInteractionManager {
    /// 语音识别与采集底层会话控制器
    pub controller: VoiceSessionController,
    /// 语音识别与快捷键配置
    pub config: VoiceConfig,
    /// 显式语音会话状态机
    pub state: VoiceSessionState,
    /// 上次切换开关模式的时间戳（用于按键防抖）
    pub last_toggle_time: Instant,
    /// 当前实时识别的流式预览文字
    pub partial_text: Option<String>,
    /// 状态栏/悬浮条展示文案（如 "正在聆听中..." 或 "正在转录中..."）
    pub display_text: Option<String>,
    /// 当前音频振幅波形可视化数据
    pub waveform: Option<String>,
}

impl Default for VoiceInteractionManager {
    fn default() -> Self {
        Self::new(VoiceConfig::default())
    }
}

impl VoiceInteractionManager {
    /// 使用指定的语音配置创建交互管理器实例
    pub fn new(config: VoiceConfig) -> Self {
        Self {
            controller: VoiceSessionController::new(),
            config,
            state: VoiceSessionState::Idle,
            last_toggle_time: Instant::now() - Duration::from_secs(10),
            partial_text: None,
            display_text: None,
            waveform: None,
        }
    }

    /// 查询当前是否处于语音激活状态（录音中、等待转录结算中或有展示浮条）
    #[inline]
    pub fn is_active(&self) -> bool {
        !matches!(self.state, VoiceSessionState::Idle)
            || self.controller.is_recording()
            || self.display_text.is_some()
    }

    /// 查询当前是否正在执行底层音频录制
    #[inline]
    pub fn is_recording(&self) -> bool {
        matches!(
            self.state,
            VoiceSessionState::PushToTalkRecording { .. }
                | VoiceSessionState::ToggleRecording { .. }
        )
    }

    /// 查询当前是否处于长按说话模式录音中
    #[inline]
    pub fn is_push_to_talk_recording(&self) -> bool {
        matches!(self.state, VoiceSessionState::PushToTalkRecording { .. })
    }

    /// 查询当前是否正处于转录结算等待状态
    #[inline]
    pub fn is_finalizing(&self) -> bool {
        matches!(self.state, VoiceSessionState::Finalizing { .. })
    }

    /// 清空语音运行状态与临时缓存
    pub fn clear_state(&mut self) {
        self.state = VoiceSessionState::Idle;
        self.display_text = None;
        self.waveform = None;
        self.partial_text = None;
    }

    /// 卸载内存中预载的模型权重并释放资源
    pub fn unload_models(&mut self) {
        self.controller.unload_models();
    }

    /// 更新语音配置，若禁用语音则自动卸载常驻模型
    pub fn update_config(&mut self, config: VoiceConfig) {
        self.config = config;
        if !self.config.enabled {
            self.unload_models();
        }
    }

    /// 从当前会话通道非阻塞提取所有已就绪事件
    fn drain_session_events(&self) -> Vec<VoiceEvent> {
        match &self.state {
            VoiceSessionState::PushToTalkRecording { rx, .. }
            | VoiceSessionState::ToggleRecording { rx }
            | VoiceSessionState::Finalizing { rx, .. } => {
                let mut evts = Vec::new();
                while let Ok(event) = rx.try_recv() {
                    evts.push(event);
                }
                evts
            }
            VoiceSessionState::Idle => Vec::new(),
        }
    }

    /// 处理波形更新事件
    fn handle_voice_waveform_event(&mut self, waveform: String, is_finalizing_before: bool) {
        if !is_finalizing_before && self.config.interaction.show_floating_bar {
            self.display_text = Some("正在聆听中...".to_string());
            self.waveform = Some(waveform);
        } else if is_finalizing_before {
            self.waveform = None;
        } else {
            self.display_text = None;
            self.waveform = None;
        }
    }

    /// 处理最终提交事件并进行兜底计算
    fn handle_voice_final_commit(&mut self, text: String) -> Option<String> {
        let trimmed = text.trim().to_string();
        let fallback_already = match self.state {
            VoiceSessionState::Finalizing {
                fallback_committed, ..
            } => fallback_committed,
            _ => false,
        };

        if fallback_already {
            debug!("[微澜语音] 收到后台最终结算，此前已保底上屏，静默同步状态");
            self.clear_state();
            return None;
        }

        let commit_text = if !trimmed.is_empty() {
            trimmed
        } else if let Some(fallback) = self.partial_text.take() {
            let fb_trimmed = fallback.trim().to_string();
            if !fb_trimmed.is_empty() {
                debug!(
                    "[微澜语音] 最终识别文本为空，启用流式预览保底上屏: \"{}\"",
                    fb_trimmed
                );
            }
            fb_trimmed
        } else {
            String::new()
        };

        self.clear_state();
        if !commit_text.is_empty() {
            info!("[微澜语音] 成功生成文字上屏: \"{}\"", commit_text);
            Some(commit_text)
        } else {
            None
        }
    }

    /// 检查处于 Finalizing 状态是否超过 2 秒防假死，若超时启用流式预览兜底
    fn check_finalizing_timeout_fallback(&mut self) -> Option<String> {
        if let VoiceSessionState::Finalizing {
            start_time,
            fallback_committed,
            ..
        } = self.state
        {
            if start_time.elapsed() >= Duration::from_millis(2000) {
                warn!("[微澜语音] 等待最终结算超时(2s)，启用流式预览兜底并强制重置状态");
                let fallback = self.partial_text.take();
                self.clear_state();
                if !fallback_committed {
                    if let Some(fb) = fallback {
                        let trimmed = fb.trim().to_string();
                        if !trimmed.is_empty() {
                            return Some(trimmed);
                        }
                    }
                }
            }
        }
        None
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字（完全非阻塞）
    pub fn poll_events(&mut self) -> Option<String> {
        let is_finalizing_before = self.is_finalizing();
        let mut final_commit = None;

        for event in self.drain_session_events() {
            match event {
                VoiceEvent::WaveformUpdated { waveform, .. } => {
                    self.handle_voice_waveform_event(waveform, is_finalizing_before);
                }
                VoiceEvent::PartialText(text) => {
                    self.partial_text = self.config.interaction.real_time_preview.then_some(text);
                }
                VoiceEvent::SegmentCommit(text) => {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        final_commit = Some(trimmed);
                        self.partial_text = None;
                        break;
                    }
                }
                VoiceEvent::FinalCommit(text) => {
                    final_commit = self.handle_voice_final_commit(text);
                    break;
                }
                VoiceEvent::Cancelled => {
                    debug!("[微澜语音] 语音会话已取消");
                    self.clear_state();
                    break;
                }
                VoiceEvent::Error(err) => {
                    error!("[微澜语音] 识别发生异常: {}", err);
                    self.clear_state();
                    break;
                }
            }
        }

        if let Some(fallback) = self.check_finalizing_timeout_fallback() {
            return Some(fallback);
        }

        final_commit
    }

    /// 启动语音录音会话
    pub(crate) fn start_recording(&mut self, is_ptt: bool, main_key: Option<u32>) -> bool {
        let (tx, rx) = sync_channel(32);
        let opts = VoiceSessionOptions {
            voice_config: self.config.clone(),
        };
        if let Err(e) = self.controller.start(opts, tx) {
            warn!("[微澜语音] 启动语音录音失败: {}", e);
            false
        } else {
            self.partial_text = None;
            if self.config.interaction.show_floating_bar {
                self.display_text = Some("正在聆听中...".to_string());
            }
            self.state = if is_ptt {
                VoiceSessionState::PushToTalkRecording {
                    trigger_key: main_key.unwrap_or(0),
                    rx,
                }
            } else {
                VoiceSessionState::ToggleRecording { rx }
            };
            true
        }
    }

    /// 停止录音并触发非阻塞最终结算
    pub fn stop_and_resolve_commit(&mut self) -> Option<String> {
        self.controller.stop_and_commit();
        self.waveform = None;
        self.display_text = None;

        // 立即尝试一次非阻塞轮询，若后台工作线程已就绪则直接返回上屏文字
        if let Some(text) = self.poll_events() {
            return Some(text);
        }

        // 提取当前的事件通道，将录音状态安全转换为结算等待状态
        let active_rx = match std::mem::take(&mut self.state) {
            VoiceSessionState::PushToTalkRecording { rx, .. }
            | VoiceSessionState::ToggleRecording { rx } => Some(rx),
            VoiceSessionState::Finalizing { rx, .. } => Some(rx),
            VoiceSessionState::Idle => None,
        };

        if let Some(rx) = active_rx {
            // 若当前已存在流式部分识别文本，立即作为保底文字上屏提交（零延迟交互），并保持通道开启等待结算吸收
            if let Some(preview) = self.partial_text.take() {
                let trimmed = preview.trim().to_string();
                if !trimmed.is_empty() {
                    self.state = VoiceSessionState::Finalizing {
                        rx,
                        start_time: Instant::now(),
                        fallback_committed: true,
                    };
                    info!(
                        "[微澜语音] 录音停止立即启用流式预览保底上屏: \"{}\"",
                        trimmed
                    );
                    return Some(trimmed);
                }
            }

            self.state = VoiceSessionState::Finalizing {
                rx,
                start_time: Instant::now(),
                fallback_committed: false,
            };
        }

        None
    }

    /// 取消当前录音并清空状态
    pub fn cancel(&mut self) {
        self.controller.cancel();
        self.clear_state();
    }
}

impl VoiceProvider for VoiceInteractionManager {
    #[inline]
    fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    #[inline]
    fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    #[inline]
    fn config(&self) -> &VoiceConfig {
        &self.config
    }

    #[inline]
    fn config_mut(&mut self) -> &mut VoiceConfig {
        &mut self.config
    }

    #[inline]
    fn is_push_to_talk_recording(&self) -> bool {
        self.is_push_to_talk_recording()
    }

    #[inline]
    fn is_active(&self) -> bool {
        self.is_active()
    }

    #[inline]
    fn is_recording(&self) -> bool {
        self.is_recording()
    }

    #[inline]
    fn cancel(&mut self) {
        self.cancel();
    }

    #[inline]
    fn clear_state(&mut self) {
        self.clear_state();
    }

    #[inline]
    fn update_config(&mut self, cfg: &VoiceConfig) {
        self.update_config(cfg.clone());
    }

    #[inline]
    fn set_engine_type(&mut self, engine_type: VoiceEngineType) {
        self.config.engine_type = engine_type;
    }

    #[inline]
    fn poll_events(&mut self) -> Option<String> {
        self.poll_events()
    }

    #[inline]
    fn dispatch_key(
        &mut self,
        key: u32,
        is_press: bool,
        modifiers: u32,
        consumed_keys: &mut ConsumedKeyTracker,
    ) -> VoiceKeyAction {
        self.dispatch_voice_key(key, is_press, modifiers, consumed_keys)
    }

    #[inline]
    fn display_text(&self) -> Option<String> {
        self.display_text.clone()
    }

    #[inline]
    fn set_display_text(&mut self, text: Option<String>) {
        self.display_text = text;
    }

    #[inline]
    fn partial_text(&self) -> Option<String> {
        self.partial_text.clone()
    }

    #[inline]
    fn set_partial_text(&mut self, text: Option<String>) {
        self.partial_text = text;
    }

    #[inline]
    fn waveform(&self) -> Option<String> {
        self.waveform.clone()
    }

    #[inline]
    fn set_waveform(&mut self, waveform: Option<String>) {
        self.waveform = waveform;
    }

    #[inline]
    fn render_state(&self) -> Option<VoiceBoxState> {
        if !self.is_active() {
            return None;
        }
        Some(VoiceBoxState {
            partial_text: self.partial_text.clone().unwrap_or_default(),
            waveform: self.waveform.clone().unwrap_or_default(),
            display_text: self.display_text.clone(),
            is_listening: true,
        })
    }
}

#[cfg(test)]
mod tests;
