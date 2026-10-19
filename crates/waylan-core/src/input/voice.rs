//! 统一语音与多模态输入提供者领域抽象契约
//!
//! # 设计原理
//! - **实现初衷**：将语音按键拦截、音频录制、流式状态提取与 ASR 结算从核心会话引擎中解耦，
//!   消除对具体语音实现库的硬编码依赖与无语音特性下的空桩代码 (Stub)。
//! - **核心优势**：依赖倒置原则 (DIP) 与开闭原则 (OCP)；核心输入法运行时 (`ImeRuntime`)
//!   仅依赖抽象特型 [`VoiceProvider`]，支持多模态输入源动态插拔与平滑测试模拟。
//! - **代价与局限**：使用动态分派 (`Box<dyn VoiceProvider>`) 带来微不足道的虚表调用开销。

use crate::candidate::VoiceBoxState;
use crate::config::{VoiceConfig, VoiceEngineType};
use crate::input::modifiers::ConsumedKeyTracker;

/// 语音按键交互事件的处理决策动作
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

/// 统一外部语音交互与多模态输入提供者契约
pub trait VoiceProvider: Send {
    /// 查询当前语音输入是否处于开启/启用状态
    fn is_enabled(&self) -> bool;

    /// 设置语音输入的开启状态
    fn set_enabled(&mut self, enabled: bool);

    /// 获取当前语音配置的只读引用
    fn config(&self) -> &VoiceConfig;

    /// 获取当前语音配置的可变引用
    fn config_mut(&mut self) -> &mut VoiceConfig;

    /// 查询当前是否处于对讲机按键录音中
    fn is_push_to_talk_recording(&self) -> bool;

    /// 查询是否处于激活状态（录音中、等待识别结果或悬浮展示中）
    fn is_active(&self) -> bool;

    /// 查询当前是否正在录音
    fn is_recording(&self) -> bool;

    /// 取消语音录制会话
    fn cancel(&mut self);

    /// 清空语音状态与历史缓存
    fn clear_state(&mut self);

    /// 热重载语音配置
    fn update_config(&mut self, cfg: &VoiceConfig);

    /// 设置语音识别引擎类型（用于测试模拟或引擎热切换）
    fn set_engine_type(&mut self, engine_type: VoiceEngineType);

    /// 轮询异步识别事件流并返回提交文本（若有）
    fn poll_events(&mut self) -> Option<String>;

    /// 分发并拦截物理按键
    fn dispatch_key(
        &mut self,
        key: u32,
        is_press: bool,
        modifiers: u32,
        consumed_keys: &mut ConsumedKeyTracker,
    ) -> VoiceKeyAction;

    /// 获取展示文案（如 "正在聆听中..."）
    fn display_text(&self) -> Option<String>;

    /// 设置展示文案（用于测试模拟或前端注入）
    fn set_display_text(&mut self, text: Option<String>);

    /// 获取实时流式预览文字
    fn partial_text(&self) -> Option<String>;

    /// 设置实时流式预览文字（用于测试模拟或前端注入）
    fn set_partial_text(&mut self, text: Option<String>);

    /// 获取当前音频振幅波形可视化数据
    fn waveform(&self) -> Option<String>;

    /// 设置当前音频振幅波形数据（用于测试模拟或前端注入）
    fn set_waveform(&mut self, waveform: Option<String>);

    /// 提取候选框渲染快照（用于构建统一候选框状态，零侵入性）
    fn render_state(&self) -> Option<VoiceBoxState>;
}
