//! IBus 语音录入事件轮询与流式断句保底提交
//!
//! # 设计原理
//! - **实现初衷**：统一委托给 [`waylan_voice::VoiceInteractionManager`] 处理语音录音状态查询、流式预览更新、VAD 自动分句以及手动/松开提交。
//! - **核心优势**：单一领域状态源驱动，消除与 Wayland 原生协议层的样板重复。
//! - **代价与局限**：IBus 会话层仅作为轻量外观转调。

use super::core::IbusSessionCore;

impl IbusSessionCore {
    /// 判断当前是否处于语音输入或语音展示激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.runtime.is_voice_active()
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    #[inline]
    pub fn poll_voice_events(&mut self) -> Option<String> {
        if let Some(ref mut vm) = self.runtime.voice_manager {
            vm.poll_events()
        } else {
            None
        }
    }
}
