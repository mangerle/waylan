//! 语音按键交互事件分发与快捷键匹配处理器

use std::time::{Duration, Instant};

use log::info;
use waylan_core::input::*;

use super::types::{VoiceKeyAction, VoiceSessionState};
use super::VoiceInteractionManager;

impl VoiceInteractionManager {
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
        if !is_press {
            if let VoiceSessionState::PushToTalkRecording { trigger_key, .. } = self.state {
                let is_main_release = trigger_key == evdev_key;
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
        }

        // 2. 录音或转录结算过程中的按键拦截（按下阶段）
        let is_recording = self.is_recording();
        let is_finalizing = self.is_finalizing();

        if is_press && (is_recording || is_finalizing) {
            if evdev_key == KEY_ESC {
                info!("[微澜语音] 收到 Esc 键，取消语音录制或转录");
                self.cancel();
                return VoiceKeyAction::Consumed;
            }

            // 若处于转录结算阶段，非 Esc 按键一律静默拦截，防止按键误打字抢先上屏
            if is_finalizing {
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
            if let VoiceSessionState::PushToTalkRecording { trigger_key, .. } = self.state {
                if trigger_key == evdev_key {
                    return VoiceKeyAction::Consumed;
                }
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

    /// 驱动语音按键事件并自动同步物理按键记账器状态
    pub fn dispatch_voice_key(
        &mut self,
        evdev_key: u32,
        is_press: bool,
        effective_mods: u32,
        consumed_keys: &mut ConsumedKeyTracker,
    ) -> VoiceKeyAction {
        let action = self.handle_key_event(evdev_key, is_press, effective_mods);
        match action {
            VoiceKeyAction::Consumed | VoiceKeyAction::Commit(_) => {
                if is_press {
                    consumed_keys.record_press(evdev_key, true);
                } else {
                    consumed_keys.take_release(evdev_key);
                }
                action
            }
            VoiceKeyAction::PassThrough => {
                // 若该按键在按下阶段已被消费，其释放事件必须同样被拦截消费以保持成对一致性并清理记账
                if !is_press && consumed_keys.take_release(evdev_key) {
                    VoiceKeyAction::Consumed
                } else {
                    VoiceKeyAction::PassThrough
                }
            }
        }
    }
}
