//! IBus 按键处理核心逻辑测试套件
//!
//! # 模块说明
//! - `basic_keys`: 基础键值映射、拼音输入、Shift 模式切换、Tab 展开与屏幕/密码上下文恢复
//! - `caps_shortcuts`: CapsLock 大写锁定穿透、与 Shift 交互、托盘状态标志同步及全局快捷键
//! - `voice_keys`: 语音输入快捷键开关、对讲机长按模式、ESC 取消与文本提交

mod basic_keys;
mod caps_shortcuts;
mod voice_keys;
