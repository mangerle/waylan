//! Wayland 原生会话语音输入快捷键拦截器构建

use std::time::Instant;

use log::{info, warn};
use waylan_core::input::*;
use waylan_core::VoiceConfig;

/// 为 Wayland 原生会话构建语音按键拦截器
pub fn build_wayland_voice_filter(
    voice_config: &VoiceConfig,
) -> Option<waylan_wayland::KeyFilterCallback> {
    if !voice_config.enabled {
        return None;
    }

    let ptt_shortcut = voice_config.get_effective_ptt_shortcut();
    let toggle_shortcut = voice_config.get_effective_toggle_shortcut();
    if ptt_shortcut.is_none() && toggle_shortcut.is_none() {
        return None;
    }

    let mut controller = waylan_voice::VoiceSessionController::new();
    let voice_cfg = voice_config.clone();
    let mut is_push_to_talk_active = false;
    let mut push_to_talk_key = None;
    let mut last_toggle_time = Instant::now() - std::time::Duration::from_secs(10);
    let mut voice_rx: Option<std::sync::mpsc::Receiver<waylan_voice::VoiceEvent>> = None;
    let mut partial_text: Option<String> = None;

    let filter = move |evdev_key: u32,
                       is_press: bool,
                       mods_depressed: u32|
          -> waylan_wayland::KeyFilterResult {
        let shift_active = (mods_depressed & MOD_SHIFT) != 0;
        let ctrl_active = (mods_depressed & MOD_CTRL) != 0;
        let alt_active = (mods_depressed & MOD_ALT) != 0;
        let super_active = (mods_depressed & MOD_SUPER) != 0;

        // 持续消费后台语音事件，更新实时预览文字
        if let Some(ref rx) = voice_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    waylan_voice::VoiceEvent::PartialText(t) => {
                        partial_text = Some(t);
                    }
                    waylan_voice::VoiceEvent::FinalCommit(t)
                    | waylan_voice::VoiceEvent::SegmentCommit(t) => {
                        partial_text = Some(t);
                    }
                    _ => {}
                }
            }
        }

        // 1. 若对讲机模式正在录音，检测主键或修饰键松开
        if !is_press && is_push_to_talk_active && controller.is_recording() {
            let is_main_release = push_to_talk_key == Some(evdev_key);
            let is_mod_release = matches!(
                evdev_key,
                KEY_LEFTCTRL
                    | KEY_RIGHTCTRL
                    | KEY_LEFTALT
                    | KEY_RIGHTALT
                    | KEY_LEFTSHIFT
                    | KEY_RIGHTSHIFT
            );
            if is_main_release || is_mod_release {
                info!("[微澜 Wayland 语音] 对讲机快捷键松开，停止录音并请求转录上屏");
                controller.stop_and_commit();
                is_push_to_talk_active = false;
                push_to_talk_key = None;

                let mut commit_result = None;
                if let Some(ref rx) = voice_rx {
                    if let Ok(
                        waylan_voice::VoiceEvent::FinalCommit(t)
                        | waylan_voice::VoiceEvent::SegmentCommit(t),
                    ) = rx.recv_timeout(std::time::Duration::from_millis(30))
                    {
                        commit_result = Some(t);
                    }
                }
                voice_rx = None;
                let text = commit_result.or_else(|| partial_text.take());
                if let Some(txt) = text {
                    let trimmed = txt.trim().to_string();
                    if !trimmed.is_empty() {
                        return waylan_wayland::KeyFilterResult::Commit(trimmed);
                    }
                }
                return waylan_wayland::KeyFilterResult::Consumed;
            }
        }

        // 2. 录音过程中的控制键拦截
        if is_press && controller.is_recording() {
            if evdev_key == KEY_ESC {
                info!("[微澜 Wayland 语音] 收到 Esc 键，取消语音录制");
                controller.cancel();
                is_push_to_talk_active = false;
                push_to_talk_key = None;
                voice_rx = None;
                partial_text = None;
                return waylan_wayland::KeyFilterResult::Consumed;
            }
            if evdev_key == KEY_ENTER {
                info!("[微澜 Wayland 语音] 收到 Enter 键，提前完成录音并请求转录上屏");
                controller.stop_and_commit();
                is_push_to_talk_active = false;
                push_to_talk_key = None;

                let mut commit_result = None;
                if let Some(ref rx) = voice_rx {
                    if let Ok(
                        waylan_voice::VoiceEvent::FinalCommit(t)
                        | waylan_voice::VoiceEvent::SegmentCommit(t),
                    ) = rx.recv_timeout(std::time::Duration::from_millis(30))
                    {
                        commit_result = Some(t);
                    }
                }
                voice_rx = None;
                let text = commit_result.or_else(|| partial_text.take());
                if let Some(txt) = text {
                    let trimmed = txt.trim().to_string();
                    if !trimmed.is_empty() {
                        return waylan_wayland::KeyFilterResult::Commit(trimmed);
                    }
                }
                return waylan_wayland::KeyFilterResult::Consumed;
            }
        }

        // 3. 仅在按键按下时匹配启动快捷键
        if !is_press {
            return waylan_wayland::KeyFilterResult::PassThrough;
        }

        // 3.1 尝试匹配对讲机模式 (Push-to-Talk)
        if let Some(ref ptt) = ptt_shortcut {
            if ptt.matches(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                if !controller.is_recording() {
                    info!("[微澜 Wayland 语音] 触发对讲机快捷键，启动语音采集");
                    let (tx, rx) = std::sync::mpsc::sync_channel(32);
                    let opts = waylan_voice::VoiceSessionOptions {
                        voice_config: voice_cfg.clone(),
                    };
                    if let Err(e) = controller.start(opts, tx) {
                        warn!("[微澜 Wayland 语音] 启动录音失败: {}", e);
                    } else {
                        is_push_to_talk_active = true;
                        push_to_talk_key = Some(evdev_key);
                        voice_rx = Some(rx);
                        partial_text = None;
                    }
                }
                return waylan_wayland::KeyFilterResult::Consumed;
            }
        }

        // 3.2 尝试匹配开关模式 (Toggle)
        if let Some(ref toggle) = toggle_shortcut {
            if toggle.matches(
                evdev_key,
                shift_active,
                ctrl_active,
                alt_active,
                super_active,
            ) {
                let now = Instant::now();
                if now.duration_since(last_toggle_time) < std::time::Duration::from_millis(400) {
                    return waylan_wayland::KeyFilterResult::Consumed;
                }
                last_toggle_time = now;

                if controller.is_recording() {
                    info!("[微澜 Wayland 语音] 触发开关快捷键停止录音并请求转录上屏");
                    controller.stop_and_commit();
                    is_push_to_talk_active = false;
                    push_to_talk_key = None;

                    let mut commit_result = None;
                    if let Some(ref rx) = voice_rx {
                        if let Ok(
                            waylan_voice::VoiceEvent::FinalCommit(t)
                            | waylan_voice::VoiceEvent::SegmentCommit(t),
                        ) = rx.recv_timeout(std::time::Duration::from_millis(30))
                        {
                            commit_result = Some(t);
                        }
                    }
                    voice_rx = None;
                    let text = commit_result.or_else(|| partial_text.take());
                    if let Some(txt) = text {
                        let trimmed = txt.trim().to_string();
                        if !trimmed.is_empty() {
                            return waylan_wayland::KeyFilterResult::Commit(trimmed);
                        }
                    }
                } else {
                    info!("[微澜 Wayland 语音] 触发开关快捷键开启连续听写录音");
                    let (tx, rx) = std::sync::mpsc::sync_channel(32);
                    let opts = waylan_voice::VoiceSessionOptions {
                        voice_config: voice_cfg.clone(),
                    };
                    if let Err(e) = controller.start(opts, tx) {
                        warn!("[微澜 Wayland 语音] 启动录音失败: {}", e);
                    } else {
                        is_push_to_talk_active = false;
                        push_to_talk_key = None;
                        voice_rx = Some(rx);
                        partial_text = None;
                    }
                }
                return waylan_wayland::KeyFilterResult::Consumed;
            }
        }

        waylan_wayland::KeyFilterResult::PassThrough
    };

    Some(Box::new(filter))
}
