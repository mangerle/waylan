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
