//! 统一语音按键交互控制状态机与事件流管理器
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland 原生协议层与 IBus 协议适配层在长按对讲机 (Push-to-Talk)、
//!   开关点按 (Toggle)、按键防抖、控制键中断 (Esc/Enter) 及流式保底提交上的样板逻辑重复。
//! - **核心优势**：单一领域状态源；集中处理音频采集启动、通道接收、实时波形/预览文本提取与故障降级；
//!   保证不同显示服务与输入协议下语音交互行为的完全一致性。
//! - **代价与局限**：按键分发需提供 evdev 物理键码与修饰键掩码，对协议层的按键抽象有轻量依赖。

use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use log::{error, info, warn};
use waylan_core::input::*;
use waylan_core::VoiceConfig;

use crate::session::{VoiceEvent, VoiceSessionController, VoiceSessionOptions};

/// 语音按键交互事件的处理决策输出
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum VoiceKeyAction {
    /// 按键未触发语音交互，放行原生透传给输入法或宿主应用
    #[default]
    PassThrough,
    /// 按键已被语音模块拦截消费，无文本上屏
    Consumed,
    /// 触发语音识别文本上屏提交
    Commit(String),
}

/// 统一语音按键交互状态机与控制器 (Voice Interaction Manager)
pub struct VoiceInteractionManager {
    /// 语音识别与采集底层会话控制器
    pub controller: VoiceSessionController,
    /// 语音识别与快捷键配置
    pub config: VoiceConfig,
    /// 是否处于长按对讲机 (Push-to-Talk) 录音模式
    pub is_push_to_talk_active: bool,
    /// 长按对讲机模式下的触发主键扫描码
    pub push_to_talk_key: Option<u32>,
    /// 上次切换开关模式的时间戳（用于按键防抖）
    pub last_toggle_time: Instant,
    /// 后台语音事件接收通道
    pub voice_rx: Option<Receiver<VoiceEvent>>,
    /// 当前实时识别的流式预览文字
    pub partial_text: Option<String>,
    /// 状态栏/悬浮条展示文案（如 "正在聆听中..."）
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
            is_push_to_talk_active: false,
            push_to_talk_key: None,
            last_toggle_time: Instant::now() - Duration::from_secs(10),
            voice_rx: None,
            partial_text: None,
            display_text: None,
            waveform: None,
        }
    }

    /// 查询当前是否处于语音激活状态（录音中或有展示浮条）
    #[inline]
    pub fn is_active(&self) -> bool {
        self.controller.is_recording() || self.display_text.is_some()
    }

    /// 查询当前是否正在执行底层音频录制
    #[inline]
    pub fn is_recording(&self) -> bool {
        self.controller.is_recording()
    }

    /// 清空语音运行状态与临时缓存
    pub fn clear_state(&mut self) {
        self.is_push_to_talk_active = false;
        self.push_to_talk_key = None;
        self.display_text = None;
        self.waveform = None;
        self.partial_text = None;
        self.voice_rx = None;
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

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    pub fn poll_events(&mut self) -> Option<String> {
        let mut final_commit = None;
        if let Some(ref rx) = self.voice_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    VoiceEvent::WaveformUpdated { waveform, .. } => {
                        if self.config.interaction.show_floating_bar {
                            self.display_text = Some("正在聆听中...".to_string());
                            self.waveform = Some(waveform);
                        } else {
                            self.display_text = None;
                            self.waveform = None;
                        }
                    }
                    VoiceEvent::PartialText(text) => {
                        if self.config.interaction.real_time_preview {
                            self.partial_text = Some(text);
                        } else {
                            self.partial_text = None;
                        }
                    }
                    VoiceEvent::SegmentCommit(text) => {
                        final_commit = Some(text);
                        self.partial_text = None;
                        break;
                    }
                    VoiceEvent::FinalCommit(text) => {
                        final_commit = Some(text);
                        self.clear_state();
                        break;
                    }
                    VoiceEvent::Cancelled => {
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
        }
        final_commit
    }

    /// 启动语音录音会话
    fn start_recording(&mut self, is_ptt: bool, main_key: Option<u32>) -> bool {
        let (tx, rx) = sync_channel(32);
        let opts = VoiceSessionOptions {
            voice_config: self.config.clone(),
        };
        if let Err(e) = self.controller.start(opts, tx) {
            warn!("[微澜语音] 启动语音录音失败: {}", e);
            false
        } else {
            self.is_push_to_talk_active = is_ptt;
            self.push_to_talk_key = main_key;
            self.voice_rx = Some(rx);
            self.partial_text = None;
            if self.config.interaction.show_floating_bar {
                self.display_text = Some("正在聆听中...".to_string());
            }
            true
        }
    }

    /// 在指定截止时间内等待语音终态事件
    fn wait_for_voice_completion(&mut self, deadline: Instant) -> Option<String> {
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if remaining.is_zero() {
                break;
            }
            if let Some(text) = self.poll_events() {
                if !text.is_empty() {
                    return Some(text);
                }
            }
            let Some(ref rx) = self.voice_rx else {
                break;
            };
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(VoiceEvent::FinalCommit(text)) | Ok(VoiceEvent::SegmentCommit(text)) => {
                    self.clear_state();
                    if !text.is_empty() {
                        return Some(text);
                    }
                    break;
                }
                Ok(VoiceEvent::PartialText(text)) => {
                    if self.config.interaction.real_time_preview {
                        self.partial_text = Some(text);
                    }
                }
                Ok(VoiceEvent::Cancelled) => {
                    self.clear_state();
                    break;
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        None
    }

    /// 停止录音并结算最终文本（带流式保底降级）
    pub fn stop_and_resolve_commit(&mut self) -> Option<String> {
        self.controller.stop_and_commit();
        let fallback_preview = self.partial_text.take();
        let deadline = Instant::now() + Duration::from_millis(50);
        let final_text = self.wait_for_voice_completion(deadline);

        let commit_text = final_text.or_else(|| {
            if let Some(preview) = fallback_preview {
                let trimmed = preview.trim().to_string();
                if !trimmed.is_empty() {
                    info!(
                        "[微澜语音] 最终结算文本为空，启用实时流式预览保底上屏: \"{}\"",
                        trimmed
                    );
                    return Some(trimmed);
                }
            }
            None
        });

        self.clear_state();
        if let Some(ref text) = commit_text {
            info!("[微澜语音] 成功生成文字上屏: \"{}\"", text);
        }
        commit_text
    }

    /// 取消当前录音并清空状态
    pub fn cancel(&mut self) {
        self.controller.cancel();
        self.clear_state();
    }

    /// 处理单个物理按键事件，返回针对语音交互的决策动作
    pub fn handle_key_event(
        &mut self,
        evdev_key: u32,
        is_press: bool,
        mods_depressed: u32,
    ) -> VoiceKeyAction {
        if !self.config.enabled {
            return VoiceKeyAction::PassThrough;
        }

        // 持续轮询后台事件刷新流式预览或自动分句上屏
        if let Some(segment) = self.poll_events() {
            let trimmed = segment.trim().to_string();
            if !trimmed.is_empty() {
                return VoiceKeyAction::Commit(trimmed);
            }
        }

        let shift_active = (mods_depressed & MOD_SHIFT) != 0;
        let ctrl_active = (mods_depressed & MOD_CTRL) != 0;
        let alt_active = (mods_depressed & MOD_ALT) != 0;
        let super_active = (mods_depressed & MOD_SUPER) != 0;

        // 1. 若处于对讲机长按录音态，检测主键或修饰键松开
        if !is_press && self.is_push_to_talk_active && self.controller.is_recording() {
            let is_main_release = self.push_to_talk_key == Some(evdev_key);
            let is_mod_release = matches!(
                evdev_key,
                KEY_LEFTCTRL
                    | KEY_RIGHTCTRL
                    | KEY_LEFTALT
                    | KEY_RIGHTALT
                    | KEY_LEFTSHIFT
                    | KEY_RIGHTSHIFT
                    | KEY_LEFTMETA
                    | KEY_RIGHTMETA
            );
            if is_main_release || is_mod_release {
                info!("[微澜语音] 对讲机快捷键松开，自动断句上屏提交");
                if let Some(text) = self.stop_and_resolve_commit() {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        return VoiceKeyAction::Commit(trimmed);
                    }
                }
                return VoiceKeyAction::Consumed;
            }
        }

        // 2. 录音过程中的按键拦截（按下阶段）
        if is_press && self.controller.is_recording() {
            if evdev_key == KEY_ESC {
                info!("[微澜语音] 收到 Esc 键，取消语音录制");
                self.cancel();
                return VoiceKeyAction::Consumed;
            }
            if evdev_key == KEY_ENTER {
                info!("[微澜语音] 收到 Enter 键，提前完成录音并请求转录上屏");
                if let Some(text) = self.stop_and_resolve_commit() {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        return VoiceKeyAction::Commit(trimmed);
                    }
                }
                return VoiceKeyAction::Consumed;
            }

            // 对讲机模式下若重复按下 PTT 主键（如键盘硬件 Auto-Repeat 连发），静默消费保持录音
            if self.is_push_to_talk_active && self.push_to_talk_key == Some(evdev_key) {
                return VoiceKeyAction::Consumed;
            }

            let toggle_shortcut = self.config.get_effective_toggle_shortcut();
            let is_toggle = toggle_shortcut.as_ref().is_some_and(|s| {
                s.matches(
                    evdev_key,
                    shift_active,
                    ctrl_active,
                    alt_active,
                    super_active,
                )
            });
            if is_toggle {
                let now = Instant::now();
                if now.duration_since(self.last_toggle_time) < Duration::from_millis(300) {
                    return VoiceKeyAction::Consumed;
                }
                self.last_toggle_time = now;
                info!("[微澜语音] 触发开关快捷键停止录音并请求转录上屏");
                if let Some(text) = self.stop_and_resolve_commit() {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        return VoiceKeyAction::Commit(trimmed);
                    }
                }
                return VoiceKeyAction::Consumed;
            }

            // 录音期间其他任意按键均静默消费，防止误打字
            return VoiceKeyAction::Consumed;
        }

        // 3. 仅在按键按下时匹配启动快捷键
        if !is_press {
            return VoiceKeyAction::PassThrough;
        }

        let ptt_shortcut = self.config.get_effective_ptt_shortcut();
        let toggle_shortcut = self.config.get_effective_toggle_shortcut();

        // 3.1 尝试匹配对讲机模式 (Push-to-Talk)
        if let Some(ref ptt) = ptt_shortcut {
            if ptt.matches(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                if !self.controller.is_recording() {
                    info!("[微澜语音] 触发对讲机快捷键，启动语音采集");
                    self.start_recording(true, Some(evdev_key));
                }
                return VoiceKeyAction::Consumed;
            }
        }

        // 3.2 尝试匹配开关模式开启录音 (Toggle)
        if let Some(ref toggle) = toggle_shortcut {
            if toggle.matches(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                let now = Instant::now();
                if now.duration_since(self.last_toggle_time) < Duration::from_millis(300) {
                    return VoiceKeyAction::Consumed;
                }
                self.last_toggle_time = now;

                info!("[微澜语音] 触发开关快捷键开启连续听写录音");
                self.start_recording(false, None);
                return VoiceKeyAction::Consumed;
            }
        }

        VoiceKeyAction::PassThrough
    }
}
