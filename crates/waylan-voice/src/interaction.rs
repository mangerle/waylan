//! 统一语音按键交互控制状态机与事件流管理器
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland 原生协议层与 IBus 协议适配层在长按对讲机 (Push-to-Talk)、
//!   开关点按 (Toggle)、按键防抖、控制键中断 (Esc/Enter) 及流式保底提交上的样板逻辑重复。
//! - **核心优势**：单一领域状态源；集中处理音频采集启动、通道接收、实时波形/预览文本提取与故障降级；
//!   保证不同显示服务与输入协议下语音交互行为的完全一致性。
//! - **代价与局限**：按键分发需提供 evdev 物理键码与修饰键掩码，对协议层的按键抽象有轻量依赖。

use std::sync::mpsc::{sync_channel, Receiver};
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
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
    /// 是否处于停止录音后等待最终结算状态
    pub is_finalizing: bool,
    /// 是否已将保底流式文本提交上屏（防止最终结算到达时重复提交）
    pub is_fallback_committed: bool,
    /// 进入最终结算状态的时间戳（用于超时防挂死保底）
    pub finalize_start_time: Option<Instant>,
    /// 上次切换开关模式的时间戳（用于按键防抖）
    pub last_toggle_time: Instant,
    /// 后台语音事件接收通道
    pub voice_rx: Option<Receiver<VoiceEvent>>,
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
            is_push_to_talk_active: false,
            push_to_talk_key: None,
            is_finalizing: false,
            is_fallback_committed: false,
            finalize_start_time: None,
            last_toggle_time: Instant::now() - Duration::from_secs(10),
            voice_rx: None,
            partial_text: None,
            display_text: None,
            waveform: None,
        }
    }

    /// 查询当前是否处于语音激活状态（录音中、等待转录结算中或有展示浮条）
    #[inline]
    pub fn is_active(&self) -> bool {
        self.controller.is_recording()
            || self.is_finalizing
            || self.display_text.is_some()
            || self.voice_rx.is_some()
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
        self.is_finalizing = false;
        self.is_fallback_committed = false;
        self.finalize_start_time = None;
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

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字（完全非阻塞）
    pub fn poll_events(&mut self) -> Option<String> {
        let mut final_commit = None;
        if let Some(ref rx) = self.voice_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    VoiceEvent::WaveformUpdated { waveform, .. } => {
                        if !self.is_finalizing && self.config.interaction.show_floating_bar {
                            self.display_text = Some("正在聆听中...".to_string());
                            self.waveform = Some(waveform);
                        } else if self.is_finalizing {
                            self.waveform = None;
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
                        let trimmed = text.trim().to_string();
                        if !trimmed.is_empty() {
                            final_commit = Some(trimmed);
                            self.partial_text = None;
                            break;
                        }
                    }
                    VoiceEvent::FinalCommit(text) => {
                        let trimmed = text.trim().to_string();
                        // 若此前停止录音时已经把保底文本上屏过，静默吸收最终事件，避免重复出字
                        if self.is_fallback_committed {
                            debug!("[微澜语音] 收到后台最终结算，此前已保底上屏，静默同步状态");
                            self.clear_state();
                            break;
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
                            final_commit = Some(commit_text);
                        }
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
        }

        // 超时防假死保底检测：若处于等待最终结算状态超过 2 秒未收到 FinalCommit 则安全降级
        if self.is_finalizing {
            let is_timeout = self
                .finalize_start_time
                .map(|t| t.elapsed() >= Duration::from_millis(2000))
                .unwrap_or(false);
            if is_timeout {
                warn!("[微澜语音] 等待最终结算超时(2s)，启用流式预览兜底并强制重置状态");
                let fallback = self.partial_text.take();
                let already_committed = self.is_fallback_committed;
                self.clear_state();
                if !already_committed {
                    if let Some(fb) = fallback {
                        let trimmed = fb.trim().to_string();
                        if !trimmed.is_empty() {
                            return Some(trimmed);
                        }
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
            self.is_finalizing = false;
            self.is_fallback_committed = false;
            self.finalize_start_time = None;
            self.voice_rx = Some(rx);
            self.partial_text = None;
            if self.config.interaction.show_floating_bar {
                self.display_text = Some("正在聆听中...".to_string());
            }
            true
        }
    }

    /// 停止录音并触发非阻塞最终结算
    pub fn stop_and_resolve_commit(&mut self) -> Option<String> {
        self.controller.stop_and_commit();
        self.is_push_to_talk_active = false;
        self.push_to_talk_key = None;
        self.waveform = None;
        self.display_text = None;

        // 立即尝试一次非阻塞轮询，若后台工作线程已就绪则直接返回上屏文字
        if let Some(text) = self.poll_events() {
            return Some(text);
        }

        // 若当前已存在流式部分识别文本，立即作为保底文字上屏提交（零延迟交互），并保持通道开启等待结算吸收
        if let Some(preview) = self.partial_text.take() {
            let trimmed = preview.trim().to_string();
            if !trimmed.is_empty() {
                self.is_fallback_committed = true;
                self.is_finalizing = true;
                self.finalize_start_time = Some(Instant::now());
                info!(
                    "[微澜语音] 录音停止立即启用流式预览保底上屏: \"{}\"",
                    trimmed
                );
                return Some(trimmed);
            }
        }

        // 若当前尚无流式文字（如整句云端识别），进入结算等待状态
        self.is_fallback_committed = false;
        self.is_finalizing = true;
        self.finalize_start_time = Some(Instant::now());
        None
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
                info!("[微澜语音] 对讲机快捷键松开，进入转录结算");
                if let Some(text) = self.stop_and_resolve_commit() {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        return VoiceKeyAction::Commit(trimmed);
                    }
                }
                return VoiceKeyAction::Consumed;
            }
        }

        // 2. 录音或转录结算过程中的按键拦截（按下阶段）
        if is_press && (self.controller.is_recording() || self.is_finalizing) {
            if evdev_key == KEY_ESC {
                info!("[微澜语音] 收到 Esc 键，取消语音录制或转录");
                self.cancel();
                return VoiceKeyAction::Consumed;
            }

            // 若处于转录结算阶段，非 Esc 按键一律静默拦截，防止按键误打字抢先上屏
            if self.is_finalizing {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn test_voice_interaction_manager_initial_state() {
        let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
        assert!(!mgr.is_active());
        assert!(!mgr.is_recording());
        assert!(!mgr.is_finalizing);
        assert!(mgr.poll_events().is_none());

        mgr.clear_state();
        assert!(!mgr.is_active());
    }

    #[test]
    fn test_voice_interaction_manager_poll_events() {
        let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
        let (tx, rx) = sync_channel(16);
        mgr.voice_rx = Some(rx);

        // 1. 测试波形与文案更新
        tx.send(VoiceEvent::WaveformUpdated {
            volume: 0.5,
            waveform: "||||".to_string(),
        })
        .unwrap();
        assert!(mgr.poll_events().is_none());
        assert_eq!(mgr.waveform.as_deref(), Some("||||"));
        assert_eq!(mgr.display_text.as_deref(), Some("正在聆听中..."));

        // 2. 测试流式预览文字
        tx.send(VoiceEvent::PartialText("测试预览".to_string()))
            .unwrap();
        assert!(mgr.poll_events().is_none());
        assert_eq!(mgr.partial_text.as_deref(), Some("测试预览"));

        // 3. 测试自动分句提交
        tx.send(VoiceEvent::SegmentCommit("分句一".to_string()))
            .unwrap();
        assert_eq!(mgr.poll_events().as_deref(), Some("分句一"));
        assert!(mgr.partial_text.is_none());

        // 4. 测试最终完成结算
        tx.send(VoiceEvent::FinalCommit("最终识别文本。".to_string()))
            .unwrap();
        assert_eq!(mgr.poll_events().as_deref(), Some("最终识别文本。"));
        assert!(!mgr.is_active());
        assert!(mgr.voice_rx.is_none());
    }

    #[test]
    fn test_voice_interaction_manager_finalize_and_fallback() {
        let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
        let (tx, rx) = sync_channel(16);
        mgr.voice_rx = Some(rx);
        mgr.is_finalizing = true;
        mgr.finalize_start_time = Some(Instant::now());
        mgr.partial_text = Some("保底预览文字".to_string());

        // 当最终结果为空时，自动启用流式预览作为保底上屏
        tx.send(VoiceEvent::FinalCommit("   ".to_string())).unwrap();
        assert_eq!(mgr.poll_events().as_deref(), Some("保底预览文字"));
        assert!(!mgr.is_active());
        assert!(!mgr.is_finalizing);
    }

    #[test]
    fn test_voice_interaction_manager_finalize_timeout_fallback() {
        let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
        let (_tx, rx) = sync_channel(16);
        mgr.voice_rx = Some(rx);
        mgr.is_finalizing = true;
        // 模拟 3 秒前进入结算（超过 2 秒超时限制）
        mgr.finalize_start_time = Some(Instant::now() - Duration::from_secs(3));
        mgr.partial_text = Some("超时保底文本".to_string());

        assert_eq!(mgr.poll_events().as_deref(), Some("超时保底文本"));
        assert!(!mgr.is_active());
        assert!(!mgr.is_finalizing);
    }

    #[test]
    fn test_voice_interaction_manager_key_interception_during_finalizing() {
        let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
        mgr.config.enabled = true;
        mgr.is_finalizing = true;

        // 1. 转录结算期间按普通键应被静默拦截
        let action = mgr.handle_key_event(KEY_SPACE, true, 0);
        assert_eq!(action, VoiceKeyAction::Consumed);

        // 2. 转录结算期间按 Esc 键应取消并清理状态
        let action = mgr.handle_key_event(KEY_ESC, true, 0);
        assert_eq!(action, VoiceKeyAction::Consumed);
        assert!(!mgr.is_finalizing);
        assert!(!mgr.is_active());
    }
}
