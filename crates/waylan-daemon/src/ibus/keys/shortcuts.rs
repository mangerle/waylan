//! IBus 全局快捷键判定与模式切换匹配
//!
//! # 设计原理
//! - **实现初衷**：集中处理简繁切换、全半角标点/字符切换、对讲机与连续语音听写等全局组合快捷键。
//! - **核心优势**：将各个快捷键分支拆分为独立的探测方法，降低模式匹配复杂度。
//! - **代价与局限**：快捷键事件由输入法直接消费，不向客户端下发。

use log::{error, info};
use std::sync::atomic::Ordering;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use waylan_core::{FullWidthCharShortcut, FullWidthPunctShortcut, TraditionalShortcut};
use waylan_voice::VoiceSessionOptions;
use waylan_wayland::input_constants::*;

use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::keyval::{KEY_F, KEY_T};

impl IbusSessionCore {
    /// 尝试匹配简繁切换快捷键
    pub(crate) fn try_toggle_traditional(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
    ) -> Option<IbusKeyOutcome> {
        let trad_match = match self.traditional_shortcut {
            TraditionalShortcut::CtrlShiftF => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_F
            }
            TraditionalShortcut::CtrlAltF => {
                ctrl_active && alt_active && !shift_active && evdev_key == KEY_F
            }
            TraditionalShortcut::CtrlShiftT => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_T
            }
            TraditionalShortcut::Disabled => false,
        };
        if trad_match {
            let next_val = !self.engine.config.traditional_chinese;
            self.engine.config.traditional_chinese = next_val;
            if let Some(ref flag) = self.shared_traditional_chinese {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                update_ui: !self.engine.raw_input().is_empty(),
                ..Default::default()
            });
        }
        None
    }

    /// 尝试匹配字符全半角切换快捷键
    pub(crate) fn try_toggle_full_width_char(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
    ) -> Option<IbusKeyOutcome> {
        let char_match = match self.full_width_char_shortcut {
            FullWidthCharShortcut::ShiftSpace => {
                shift_active && !ctrl_active && !alt_active && evdev_key == KEY_SPACE
            }
            FullWidthCharShortcut::CtrlShiftSpace => {
                ctrl_active && shift_active && !alt_active && evdev_key == KEY_SPACE
            }
            FullWidthCharShortcut::Disabled => false,
        };
        if char_match {
            let next_val = !self.is_full_width_char();
            if let Some(ref flag) = self.shared_full_width_char {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }
        None
    }

    /// 尝试匹配标点全半角切换快捷键
    pub(crate) fn try_toggle_full_width_punct(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
    ) -> Option<IbusKeyOutcome> {
        let punct_match = match self.full_width_punct_shortcut {
            FullWidthPunctShortcut::CtrlPeriod => {
                ctrl_active && !shift_active && !alt_active && evdev_key == KEY_DOT
            }
            FullWidthPunctShortcut::AltPeriod => {
                alt_active && !ctrl_active && !shift_active && evdev_key == KEY_DOT
            }
            FullWidthPunctShortcut::Disabled => false,
        };
        if punct_match {
            let next_val = !self.punct_processor.full_width_punct;
            self.punct_processor.full_width_punct = next_val;
            if let Some(ref flag) = self.shared_full_width_punct {
                flag.store(next_val, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }
        None
    }

    /// 尝试触发长按对讲机语音录入快捷键
    pub(crate) fn try_trigger_push_to_talk(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
        super_active: bool,
    ) -> Option<IbusKeyOutcome> {
        let ptt_match = self
            .voice_config
            .get_effective_ptt_shortcut()
            .is_some_and(|s| {
                s.matches(
                    evdev_key,
                    shift_active,
                    ctrl_active,
                    alt_active,
                    super_active,
                )
            });
        if !ptt_match {
            return None;
        }

        if self.voice_controller.is_recording() && self.is_push_to_talk_active {
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }
        if !self.voice_controller.is_recording() {
            info!("[微澜 IBus 语音] 对讲机模式快捷键按下，开始捕获语音");
            self.engine.reset();
            let (tx, rx) = sync_channel(32);
            let opts = VoiceSessionOptions {
                voice_config: self.voice_config.clone(),
            };
            if let Err(e) = self.voice_controller.start(opts, tx) {
                error!("[微澜 IBus 语音] 启动麦克风录音捕获失败: {}", e);
            } else {
                self.is_push_to_talk_active = true;
                self.push_to_talk_key = Some(evdev_key);
                self.voice_display_text = if self.voice_config.interaction.show_floating_bar {
                    Some("正在聆听中...".to_string())
                } else {
                    None
                };
                self.voice_waveform = if self.voice_config.interaction.show_floating_bar {
                    Some("     ".to_string())
                } else {
                    None
                };
                self.voice_partial_text = None;
                self.voice_event_rx = Some(rx);
            }
            return Some(IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            });
        }
        None
    }

    /// 尝试触发连续听写开关语音快捷键
    pub(crate) fn try_trigger_voice_toggle(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
        super_active: bool,
    ) -> Option<IbusKeyOutcome> {
        let toggle_match = self
            .voice_config
            .get_effective_toggle_shortcut()
            .is_some_and(|s| {
                s.matches(
                    evdev_key,
                    shift_active,
                    ctrl_active,
                    alt_active,
                    super_active,
                )
            });
        if !toggle_match {
            return None;
        }

        let now = Instant::now();
        if now.duration_since(self.last_voice_trigger_time) < Duration::from_millis(400) {
            return Some(IbusKeyOutcome {
                consumed: true,
                ..Default::default()
            });
        }
        self.last_voice_trigger_time = now;

        if self.voice_controller.is_recording() {
            info!("[微澜 IBus 语音] 开关模式快捷键触发停止录音并请求提交");
            let commit_text = self.stop_voice_recording_and_commit();
            Some(IbusKeyOutcome {
                consumed: true,
                commit_text,
                update_ui: true,
                ..Default::default()
            })
        } else {
            info!("[微澜 IBus 语音] 开关模式快捷键触发开启连续听写录音");
            self.engine.reset();
            let (tx, rx) = sync_channel(32);
            let opts = VoiceSessionOptions {
                voice_config: self.voice_config.clone(),
            };
            if let Err(e) = self.voice_controller.start(opts, tx) {
                error!("[微澜 IBus 语音] 启动麦克风录音捕获失败: {}", e);
            } else {
                self.is_push_to_talk_active = false;
                self.push_to_talk_key = None;
                self.voice_display_text = if self.voice_config.interaction.show_floating_bar {
                    Some("正在聆听中...".to_string())
                } else {
                    None
                };
                self.voice_waveform = if self.voice_config.interaction.show_floating_bar {
                    Some("     ".to_string())
                } else {
                    None
                };
                self.voice_partial_text = None;
                self.voice_event_rx = Some(rx);
            }
            Some(IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            })
        }
    }

    /// 匹配并执行用户配置的全局模式切换快捷键（简繁切换、字符全半角、标点全半角）
    pub(crate) fn handle_global_shortcuts(
        &mut self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
        super_active: bool,
    ) -> Option<IbusKeyOutcome> {
        if let Some(outcome) =
            self.try_toggle_traditional(evdev_key, shift_active, ctrl_active, alt_active)
        {
            return Some(outcome);
        }
        if let Some(outcome) =
            self.try_toggle_full_width_char(evdev_key, shift_active, ctrl_active, alt_active)
        {
            return Some(outcome);
        }
        if let Some(outcome) =
            self.try_toggle_full_width_punct(evdev_key, shift_active, ctrl_active, alt_active)
        {
            return Some(outcome);
        }

        if self.voice_config.enabled {
            if let Some(outcome) = self.try_trigger_push_to_talk(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                return Some(outcome);
            }
            if let Some(outcome) = self.try_trigger_voice_toggle(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                return Some(outcome);
            }
        }

        None
    }
}
