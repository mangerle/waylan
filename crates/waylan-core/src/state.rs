//! 输入法会话与托盘/指示器间共享的原子状态容器
//!
//! # 设计原理
//! - **实现初衷**：聚合散落在输入法会话（Wayland/IBus）与系统托盘之间的独立 `Arc<AtomicBool>`，消除分散状态传递与更新时序不一致隐患。
//! - **核心优势**：单对象持有所有关键输入模式标志（中英、CapsLock、简繁、全半角标点、全半角字符），支持快速一致性快照与整体传递。
//! - **代价与局限**：底层状态变迁仍基于各字段的独立原子操作，跨多状态修改需注意读取顺序。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 共享输入状态快照（不可变只读值对象）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStateSnapshot {
    /// 中文模式
    pub is_chinese: bool,
    /// CapsLock 大写锁定
    pub is_caps_locked: bool,
    /// 繁体输出
    pub traditional_chinese: bool,
    /// 全角标点
    pub full_width_punct: bool,
    /// 全角字符
    pub full_width_char: bool,
}

/// 聚合的输入法状态原子标志容器
#[derive(Debug, Clone)]
pub struct SharedInputFlags {
    /// 中文模式标志
    pub is_chinese: Arc<AtomicBool>,
    /// CapsLock 锁定标志
    pub is_caps_locked: Arc<AtomicBool>,
    /// 简繁体状态标志
    pub traditional_chinese: Arc<AtomicBool>,
    /// 全半角标点标志
    pub full_width_punct: Arc<AtomicBool>,
    /// 全半角字符标志
    pub full_width_char: Arc<AtomicBool>,
}

impl SharedInputFlags {
    /// 创建新的共享输入状态标志容器
    pub fn new(
        init_chinese: bool,
        traditional_chinese: bool,
        full_width_punct: bool,
        full_width_char: bool,
    ) -> Self {
        Self {
            is_chinese: Arc::new(AtomicBool::new(init_chinese)),
            is_caps_locked: Arc::new(AtomicBool::new(false)),
            traditional_chinese: Arc::new(AtomicBool::new(traditional_chinese)),
            full_width_punct: Arc::new(AtomicBool::new(full_width_punct)),
            full_width_char: Arc::new(AtomicBool::new(full_width_char)),
        }
    }

    /// 获取当前所有状态的一致性快照
    pub fn snapshot(&self) -> InputStateSnapshot {
        InputStateSnapshot {
            is_chinese: self.is_chinese.load(Ordering::Acquire),
            is_caps_locked: self.is_caps_locked.load(Ordering::Acquire),
            traditional_chinese: self.traditional_chinese.load(Ordering::Acquire),
            full_width_punct: self.full_width_punct.load(Ordering::Acquire),
            full_width_char: self.full_width_char.load(Ordering::Acquire),
        }
    }

    /// 查询当前是否处于中文模式
    #[inline]
    pub fn is_chinese(&self) -> bool {
        self.is_chinese.load(Ordering::Acquire)
    }

    /// 设置中文模式标志
    #[inline]
    pub fn set_chinese(&self, val: bool) {
        self.is_chinese.store(val, Ordering::Release);
    }

    /// 查询当前是否处于大写锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        self.is_caps_locked.load(Ordering::Acquire)
    }

    /// 设置大写锁定标志
    #[inline]
    pub fn set_caps_locked(&self, val: bool) {
        self.is_caps_locked.store(val, Ordering::Release);
    }

    /// 查询当前是否处于繁体输出模式
    #[inline]
    pub fn is_traditional(&self) -> bool {
        self.traditional_chinese.load(Ordering::Acquire)
    }

    /// 设置繁体输出标志
    #[inline]
    pub fn set_traditional(&self, val: bool) {
        self.traditional_chinese.store(val, Ordering::Release);
    }

    /// 查询当前是否处于全角标点模式
    #[inline]
    pub fn is_full_width_punct(&self) -> bool {
        self.full_width_punct.load(Ordering::Acquire)
    }

    /// 设置全角标点标志
    #[inline]
    pub fn set_full_width_punct(&self, val: bool) {
        self.full_width_punct.store(val, Ordering::Release);
    }

    /// 获取全角字符标志
    #[inline]
    pub fn is_full_width_char(&self) -> bool {
        self.full_width_char.load(Ordering::Acquire)
    }

    /// 设置全角字符标志
    #[inline]
    pub fn set_full_width_char(&self, val: bool) {
        self.full_width_char.store(val, Ordering::Release);
    }

    /// 获取中文模式原子标志句柄
    pub fn is_chinese_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_chinese)
    }

    /// 获取 CapsLock 大写锁定原子标志句柄
    pub fn is_caps_locked_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_caps_locked)
    }

    /// 获取繁体输出原子标志句柄
    pub fn traditional_chinese_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.traditional_chinese)
    }

    /// 获取全角标点原子标志句柄
    pub fn full_width_punct_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.full_width_punct)
    }

    /// 获取全角字符原子标志句柄
    pub fn full_width_char_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.full_width_char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_input_flags_snapshot_and_updates() {
        let flags = SharedInputFlags::new(true, false, true, false);
        let snap = flags.snapshot();
        assert!(snap.is_chinese);
        assert!(!snap.is_caps_locked);
        assert!(!snap.traditional_chinese);
        assert!(snap.full_width_punct);
        assert!(!snap.full_width_char);

        flags.set_caps_locked(true);
        flags.set_chinese(false);
        let snap2 = flags.snapshot();
        assert!(!snap2.is_chinese);
        assert!(snap2.is_caps_locked);
    }
}
