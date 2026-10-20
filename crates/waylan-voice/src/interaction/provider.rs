//! `VoiceInteractionManager` 对 `waylan_core::input::VoiceProvider` 的特型实现
//!
//! # 设计原理
//! - **实现初衷**：将对核心输入法统一抽象特型 `VoiceProvider` 的适配胶水代码从交互管理器内部状态机解耦。
//! - **核心优势**：
//!   1. 降低 `interaction/mod.rs` 源码文件圈复杂度与单文件行数；
//!   2. 职责高度单一：本模块专门负责向打字引擎上下文（`ImeRuntime` 等）提供统一状态视图与按键代理；
//!   3. 零额外运行时开销：所有特型方法均为 `#[inline]` 转发。

use waylan_core::input::{ConsumedKeyTracker, VoiceKeyAction, VoiceProvider};
use waylan_core::{VoiceBoxState, VoiceConfig, VoiceEngineType};

use super::VoiceInteractionManager;

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
            visual_effect: self.config.interaction.visual_effect,
        })
    }
}
