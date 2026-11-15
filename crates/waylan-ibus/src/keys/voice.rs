//! IBus 语音录入事件轮询与流式断句保底提交
//!
//! # 设计原理
//! - **实现初衷**：统一委托给 [`waylan_core::input::VoiceProvider`] 抽象特型处理语音录音状态查询、流式预览更新、VAD 自动分句以及手动/松开提交。
//! - **核心优势**：单一领域状态源驱动，消除与 Wayland 原生协议层的样板重复。
//! - **代价与局限**：IBus 会话层仅作为轻量外观转调。

use super::core::{IbusKeyOutcome, IbusSessionCore};
use super::dispatcher::IbusFrontend;

impl IbusSessionCore {
    /// 判断当前是否处于语音输入或语音展示激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.runtime.is_voice_active()
    }

    /// 轮询并消费后台语音引擎上报的事件，通过统一 [`ImeRuntime::poll_voice_cycle`] 驱动前端快照与上屏
    pub fn poll_voice_cycle(&mut self) -> IbusKeyOutcome {
        let mut outcome = IbusKeyOutcome::default();
        let page_size = self.runtime.core.engine.config.page_size;
        let orientation = self.candidate_orientation;
        let index_style = self.candidate_index_style;
        let commit_opt = {
            let mut frontend = IbusFrontend {
                outcome: &mut outcome,
                page_size,
                orientation,
                index_style,
                raw_v_preedit: None,
            };
            self.runtime.poll_voice_cycle(&mut frontend, false)
        };
        if outcome.commit_text.is_none() {
            outcome.commit_text = commit_opt;
        }
        outcome
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字（向后兼容测试接口）
    #[inline]
    pub fn poll_voice_events(&mut self) -> Option<String> {
        self.poll_voice_cycle().commit_text
    }
}
