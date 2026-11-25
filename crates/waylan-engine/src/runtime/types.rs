//! 统一交互运行时参数与上下文定义模块

/// 统一按键事件输入参数对象
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessKeyEventParams<'a> {
    /// 物理扫描码或协议键值
    pub key: u32,
    /// 是否为按下事件（true 为按下，false 为释放）
    pub is_press: bool,
    /// 当前是否处于密码框或锁屏保护上下文
    pub is_password_or_locked: bool,
    /// 客户端环境上下文文本与选区元组（全文、光标锚点、光标焦点）
    pub surrounding_text: Option<&'a (String, u32, u32)>,
    /// 是否开启客户端内联嵌入式预编辑（Inline Preedit）
    pub inline_preedit: bool,
}

/// 统一候选词选词目标模式（供鼠标点击、IBus 候选点击等直接选词路径共用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSelectTarget {
    /// 自动依据当前视口展开态分流（大网格全量 / 二级展开卡片子项 / 普通分页候选）
    Auto,
    /// 普通分页或首页候选索引
    PageCandidate,
    /// 5 行大网格全量候选索引
    GridAll,
}

/// 统一候选词选词上屏执行结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateCommitOutcome {
    /// 最终已应用全角转换并登记标点记录的上屏文本
    pub committed_text: String,
    /// 若为部分选词尚有剩余拼音，返回格式化后的剩余预编辑文本；全部上屏则为 `None`
    pub remaining_preedit: Option<String>,
}
