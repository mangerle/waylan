//! 输入法会话事件与统一决策动作契约
//!
//! # 设计原理
//! - **实现初衷**：将输入法会话状态机对外产出的动作枚举与快捷键切换动作从巨型 `mod.rs` 中分离。
//! - **核心优势**：职责单一（SRP），契约独立，方便上层协议（Wayland、IBus）与内部测试按需引用。
//! - **代价与局限**：调用方若需全量导入需通过 `use crate::session::*` 或显式导入 `types::*`。

use waylan_core::InputMode;

/// 全局模式与功能快捷键切换动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutToggleAction {
    /// 简繁即时切换，包含切换后的布尔值（true 为繁体中文）
    Traditional(bool),
    /// 字符全角/半角切换，包含切换后的布尔值（true 为全角字符）
    FullWidthChar(bool),
    /// 标点全角/半角切换，包含切换后的布尔值（true 为全角标点）
    FullWidthPunct(bool),
}

/// 核心输入法按键与会话事件统一决策动作 (IME Output Action)
///
/// # 设计原理
/// - **消除概念膨胀**：统一拼音组词状态机、标点处理器、快捷键流转与各协议适配层的动作契约，
///   彻底清除原 `KeyProcessAction`、`ImeAction` 与 `KeyProcessResult` 之间的多层机械映射与中间层。
/// - **单一真相源**：单一动作枚举完整表达所有可能的输入法决策，上层协议（Wayland V1、V2、IBus）直接消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeOutputAction {
    /// 原生透传：按键未被输入法拦截，协议层需将硬件按键原生发送给客户端/应用程序
    PassThrough,
    /// 内部已消费并处理：协议层需消费该按键，update_ui 指示是否需要刷新预编辑与候选框
    Handled { update_ui: bool },
    /// 请求展开大候选词网格卡片
    ExpandGrid,
    /// 触发纯文本直接上屏提交（如空格选词、数字选词等，按键已被消费）
    Commit(String),
    /// CapsLock 截断提交：有未上屏拼音需提交，但按键本身不消费，需透传给系统切换大小写
    CommitAndPassThrough(String),
    /// 成对标点符号上屏并附带光标相对位移（如引号/括号自动配对后光标向左移动若干字符）
    CommitPunctuation { text: String, cursor_offset: i32 },
    /// 分段选词部分上屏，包含本次已提交文本与剩余预编辑拼音
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 取消/重置当前输入（如 Escape 取消）
    Reset,
    /// 模式或功能快捷键切换（简繁切换、字符全半角、标点全半角）
    ShortcutToggled(ShortcutToggleAction),
    /// 输入模式发生切换（如按 Shift 切换中英文）
    ModeChanged {
        new_mode: InputMode,
        committed: Option<String>,
        was_consumed: bool,
    },
}
