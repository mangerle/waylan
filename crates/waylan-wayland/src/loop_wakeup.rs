//! 事件循环跨线程与信号唤醒机制模块
//!
//! 向前兼容重导出：核心实现已统一下沉至 `waylan_core::loop_wakeup`。

pub use waylan_core::loop_wakeup::LoopWakeup;
