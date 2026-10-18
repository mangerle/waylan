//! 无语音特性下的空桩交互模型
//!
//! # 设计原理
//! - **实现初衷**：在极简或轻量构建（`--no-default-features`）下彻底剥离语音子系统与神经网络推理引擎依赖。
//! - **核心优势**：零开销桩类型，保持 `ImeRuntime` 公共接口与内部调度完全一致，杜绝条件编译宏入侵主业务流程。

use waylan_core::input::ConsumedKeyTracker;
use waylan_core::VoiceConfig;

/// 语音按键动作空桩
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

/// 统一语音按键交互与录音流式管理器空桩
#[derive(Debug, Default)]
pub struct VoiceInteractionManager {
    pub partial_text: Option<String>,
    pub waveform: Option<String>,
    pub display_text: Option<String>,
}

impl VoiceInteractionManager {
    /// 构造空桩实例
    pub fn new(_cfg: VoiceConfig) -> Self {
        Self::default()
    }

    /// 查询是否处于语音激活状态
    #[inline]
    pub fn is_active(&self) -> bool {
        false
    }

    /// 查询是否正在录音
    #[inline]
    pub fn is_recording(&self) -> bool {
        false
    }

    /// 取消语音录制
    #[inline]
    pub fn cancel(&mut self) {}

    /// 重置语音状态
    #[inline]
    pub fn clear_state(&mut self) {}

    /// 更新语音配置
    #[inline]
    pub fn update_config(&mut self, _cfg: VoiceConfig) {}

    /// 轮询语音事件流
    #[inline]
    pub fn poll_events(&mut self) -> Option<String> {
        None
    }

    /// 尝试分发语音按键
    #[inline]
    pub fn dispatch_voice_key(
        &mut self,
        _key: u32,
        _is_press: bool,
        _modifiers: u32,
        _consumed_keys: &mut ConsumedKeyTracker,
    ) -> VoiceKeyAction {
        VoiceKeyAction::PassThrough
    }
}
