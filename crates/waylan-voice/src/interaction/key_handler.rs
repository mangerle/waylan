//! 语音按键交互事件分发与快捷键匹配处理器

use std::time::{Duration, Instant};

use log::info;
use waylan_core::config::KeyModifiers;
use waylan_core::input::*;

use super::VoiceInteractionManager;
use super::types::{VoiceKeyAction, VoiceSessionState};

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

        // 1. 若处于对讲机长按录音态，检测主键或修饰键松开
        if !is_press {
            if let Some(action) = self.handle_ptt_release(evdev_key) {
                return action;
            }
            return VoiceKeyAction::PassThrough;
        }

        // 2. 录音或转录结算过程中的按键拦截（按下阶段）
        if self.is_recording() || self.is_finalizing() {
            return self.handle_active_key_press(evdev_key, mods_depressed);
        }

        // 3. 空闲或识别完成淡出态下的启动快捷键匹配；若用户按下普通键则立即打断淡出让出焦点
        let action = self.handle_idle_key_press(evdev_key, mods_depressed);
        if matches!(action, VoiceKeyAction::PassThrough) && self.is_completed() {
            self.clear_state();
        }
        action
    }

    /// 检测对讲机模式下的主键或修饰键松开事件并结算上屏
    fn handle_ptt_release(&mut self, evdev_key: u32) -> Option<VoiceKeyAction> {
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
                        return Some(VoiceKeyAction::Commit(trimmed));
                    }
                }
                return Some(VoiceKeyAction::Consumed);
            }
        }
        None
    }

    /// 请求结束录音并结算转录文本，返回非空的上屏文本
    fn request_stop_and_commit(&mut self) -> Option<VoiceKeyAction> {
        if let Some(text) = self.stop_and_resolve_commit() {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Some(VoiceKeyAction::Commit(trimmed));
            }
        }
        None
    }

    /// 匹配开关快捷键并触发停止录音，命中且非连发窗口内时返回消费动作
    fn handle_toggle_shortcut_press(
        &mut self,
        evdev_key: u32,
        mods_depressed: u32,
    ) -> Option<VoiceKeyAction> {
        let shift = (mods_depressed & MOD_SHIFT) != 0;
        let ctrl = (mods_depressed & MOD_CTRL) != 0;
        let alt = (mods_depressed & MOD_ALT) != 0;
        let super_mod = (mods_depressed & MOD_SUPER) != 0;
        let modifiers = KeyModifiers::new(ctrl, alt, shift, super_mod);
        let toggle_shortcut = self.toggle_shortcut;
        let is_toggle = toggle_shortcut
            .as_ref()
            .is_some_and(|s| s.matches(evdev_key, &modifiers));

        if !is_toggle {
            return None;
        }

        let now = Instant::now();
        if now.duration_since(self.last_toggle_time) < Duration::from_millis(300) {
            return Some(VoiceKeyAction::Consumed);
        }
        self.last_toggle_time = now;
        info!("[微澜语音] 触发开关快捷键停止录音并请求转录上屏");
        Some(
            self.request_stop_and_commit()
                .unwrap_or(VoiceKeyAction::Consumed),
        )
    }

    /// 录音或结算进行中的按键过滤与控制（Esc 取消、Enter 提交、Toggle 结束）
    fn handle_active_key_press(&mut self, evdev_key: u32, mods_depressed: u32) -> VoiceKeyAction {
        if evdev_key == KEY_ESC {
            info!("[微澜语音] 收到 Esc 键，取消语音录制或转录");
            self.cancel();
            return VoiceKeyAction::Consumed;
        }

        // 若处于转录结算阶段，非 Esc 按键一律静默拦截，防止按键误打字抢先上屏
        if self.is_finalizing() {
            return VoiceKeyAction::Consumed;
        }

        if evdev_key == KEY_ENTER {
            info!("[微澜语音] 收到 Enter 键，提前完成录音并请求转录上屏");
            return self
                .request_stop_and_commit()
                .unwrap_or(VoiceKeyAction::Consumed);
        }

        // 对讲机模式下若重复按下 PTT 主键（如键盘硬件 Auto-Repeat 连发），静默消费保持录音
        if let VoiceSessionState::PushToTalkRecording { trigger_key, .. } = self.state
            && trigger_key == evdev_key
        {
            return VoiceKeyAction::Consumed;
        }

        // 命中开关快捷键时停止录音并请求转录上屏
        if let Some(action) = self.handle_toggle_shortcut_press(evdev_key, mods_depressed) {
            return action;
        }

        // 录音期间其他任意按键均静默消费，防止误打字
        VoiceKeyAction::Consumed
    }

    /// 空闲状态下匹配对讲机或开关连续听写快捷键
    fn handle_idle_key_press(&mut self, evdev_key: u32, mods_depressed: u32) -> VoiceKeyAction {
        let modifiers = KeyModifiers::new(
            (mods_depressed & MOD_CTRL) != 0,
            (mods_depressed & MOD_ALT) != 0,
            (mods_depressed & MOD_SHIFT) != 0,
            (mods_depressed & MOD_SUPER) != 0,
        );

        // 1. 尝试匹配对讲机模式 (Push-to-Talk)
        if let Some(ref ptt) = self.ptt_shortcut
            && ptt.matches(evdev_key, &modifiers)
        {
            if !self.controller.is_recording() {
                info!("[微澜语音] 触发对讲机快捷键，启动语音采集");
                self.start_recording(true, Some(evdev_key));
            }
            return VoiceKeyAction::Consumed;
        }

        // 2. 尝试匹配开关模式开启录音 (Toggle)
        if let Some(ref toggle) = self.toggle_shortcut
            && toggle.matches(evdev_key, &modifiers)
        {
            let now = Instant::now();
            if now.duration_since(self.last_toggle_time) < Duration::from_millis(300) {
                return VoiceKeyAction::Consumed;
            }
            self.last_toggle_time = now;

            info!("[微澜语音] 触发开关快捷键开启连续听写录音");
            self.start_recording(false, None);
            return VoiceKeyAction::Consumed;
        }

        VoiceKeyAction::PassThrough
    }

    /// 驱动语音按键事件并自动同步物理按键记账器状态
    ///
    /// 事件上下文统一由核心契约参数对象 `waylan_core::input::VoiceKeyDispatchParams` 承载，
    /// 与 `VoiceProvider::dispatch_key` 共用同一份参数定义，避免两处字段语义漂移。
    pub fn dispatch_voice_key(&mut self, params: VoiceKeyDispatchParams<'_>) -> VoiceKeyAction {
        let VoiceKeyDispatchParams {
            key,
            is_press,
            modifiers,
            consumed_keys,
        } = params;
        let action = self.handle_key_event(key, is_press, modifiers);
        match action {
            VoiceKeyAction::Consumed | VoiceKeyAction::Commit(_) => {
                if is_press {
                    consumed_keys.record_press(key, true);
                } else {
                    consumed_keys.take_release(key);
                }
                action
            }
            VoiceKeyAction::PassThrough => {
                // 若该按键在按下阶段已被消费，其释放事件必须同样被拦截消费以保持成对一致性并清理记账
                if !is_press && consumed_keys.take_release(key) {
                    VoiceKeyAction::Consumed
                } else {
                    VoiceKeyAction::PassThrough
                }
            }
        }
    }
}
