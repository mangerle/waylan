//! 输入法动作指令与工作模式领域模型

/// 输入法向外部分发的核心动作指令
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAction {
    /// 设置应用文本框内的带下划线预编辑文本
    SetPreedit(String),
    /// 更新当前候选词列表
    SetCandidates(Vec<super::entity::Candidate>),
    /// 向当前焦点应用正式提交上屏文本
    Commit(String),
    /// 请求系统移动光标（例如成对符号补全时光标左移一格）
    MoveCursor(i32),
    /// 清空预编辑与候选，重置状态
    Reset,
}

/// 输入法当前的工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// 中文拼音输入模式
    #[default]
    Chinese,
    /// 英文直接穿透模式
    English,
    /// V 模式（计算器、特殊符号等快捷工具）
    VMode,
}
