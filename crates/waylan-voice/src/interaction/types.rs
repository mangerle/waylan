//! 语音按键交互控制状态与决策动作类型定义

use std::sync::mpsc::Receiver;
use std::time::Instant;

use crate::session::VoiceEvent;

pub use waylan_core::input::VoiceKeyAction;

/// 语音交互会话显式状态机枚举
///
/// # 设计原理
/// - **非法状态不可表示**：彻底消除散落的互斥布尔值标志位，确保状态组合在编译期具备数学确定性。
/// - **明确状态闭环**：录音通道 (`Receiver<VoiceEvent>`) 与活跃阶段绑定，空闲时生命周期自然结束。
#[derive(Debug, Default)]
pub enum VoiceSessionState {
    /// 空闲就绪状态，无活跃录音或结算任务
    #[default]
    Idle,
    /// 长按对讲机 (Push-to-Talk) 录音中
    PushToTalkRecording {
        /// 触发对讲机录音的物理扫描码
        trigger_key: u32,
        /// 后台事件接收通道
        rx: Receiver<VoiceEvent>,
    },
    /// 开关点按 (Toggle) 连续听写录音中
    ToggleRecording {
        /// 后台事件接收通道
        rx: Receiver<VoiceEvent>,
    },
    /// 录音已结束，等待后台最终识别文本结算 (FinalCommit) 或超时
    Finalizing {
        /// 后台事件接收通道
        rx: Receiver<VoiceEvent>,
        /// 进入结算状态的时间戳（用于 2 秒超时防挂死保底）
        start_time: Instant,
        /// 是否已在录音停止瞬间提前将流式预览文字提交上屏
        fallback_committed: bool,
    },
    /// 识别已完成，短暂展示「识别完成」提示并平滑淡出
    Completed {
        /// 进入完成态的时间戳（用于到期自动收起）
        start_time: Instant,
    },
}
