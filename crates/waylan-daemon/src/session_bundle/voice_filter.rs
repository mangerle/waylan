//! Wayland 原生会话语音输入快捷键拦截器构建
//!
//! # 设计原理
//! - **实现初衷**：为 Wayland 原生会话提供统一的键盘拦截回调包装，将底层快捷键事件接入语音交互状态机。
//! - **核心优势**：极简薄包装，将复杂的录音启动、通道轮询、按键防抖与流式保底统一托管给 [`VoiceInteractionManager`]。
//! - **代价与局限**：受限于 Wayland 键盘捕获上下文，需将结果映射为 [`KeyFilterResult`]。

use waylan_core::VoiceConfig;
use waylan_voice::VoiceInteractionManager;

/// 为 Wayland 原生会话构建统一语音交互状态机
#[cfg_attr(not(test), allow(dead_code))]
pub fn build_wayland_voice_manager(voice_config: &VoiceConfig) -> Option<VoiceInteractionManager> {
    if !voice_config.enabled {
        return None;
    }

    let ptt_shortcut = voice_config.get_effective_ptt_shortcut();
    let toggle_shortcut = voice_config.get_effective_toggle_shortcut();
    if ptt_shortcut.is_none() && toggle_shortcut.is_none() {
        return None;
    }

    Some(VoiceInteractionManager::new(voice_config.clone()))
}
