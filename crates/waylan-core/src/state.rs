//! 输入法会话与托盘/指示器间共享的原子状态容器
//!
//! # 设计原理
//! - **实现初衷**：聚合散落在输入法会话（Wayland/IBus）与系统托盘之间的独立状态，消除分散状态传递与更新时序不一致隐患。
//! - **核心优势**：
//!   1. 内部采用单原子位图 (`AtomicU8`) 统一驱动，`snapshot()` 仅需单次原子指令，从物理上杜绝状态撕裂 (State Tearing)；
//!   2. 支持单次 CAS 原子批量更新中英模式与 CapsLock 状态，避免瞬时非法状态组合；
//!   3. 零多余堆内存分配，无需双写同步。
//! - **代价与局限**：状态受限于 8 位位图（最多 8 种布尔标志组合）。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// 中文输入模式状态位 (bit 0)
pub const FLAG_CHINESE: u8 = 1 << 0;
/// CapsLock 大写锁定状态位 (bit 1)
pub const FLAG_CAPS_LOCKED: u8 = 1 << 1;
/// 繁体中文输出状态位 (bit 2)
pub const FLAG_TRADITIONAL: u8 = 1 << 2;
/// 全角标点状态位 (bit 3)
pub const FLAG_FULL_WIDTH_PUNCT: u8 = 1 << 3;
/// 全角字符状态位 (bit 4)
pub const FLAG_FULL_WIDTH_CHAR: u8 = 1 << 4;

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

/// 聚合的输入法状态原子标志容器（基于单原子位图实现，杜绝状态撕裂与冗余堆分配）
#[derive(Debug, Clone)]
pub struct SharedInputFlags {
    /// 核心单原子位图
    flags: Arc<AtomicU8>,
}

impl SharedInputFlags {
    /// 创建新的共享输入状态标志容器
    pub fn new(
        init_chinese: bool,
        traditional_chinese: bool,
        full_width_punct: bool,
        full_width_char: bool,
    ) -> Self {
        let mut bits = 0u8;
        if init_chinese {
            bits |= FLAG_CHINESE;
        }
        if traditional_chinese {
            bits |= FLAG_TRADITIONAL;
        }
        if full_width_punct {
            bits |= FLAG_FULL_WIDTH_PUNCT;
        }
        if full_width_char {
            bits |= FLAG_FULL_WIDTH_CHAR;
        }

        Self {
            flags: Arc::new(AtomicU8::new(bits)),
        }
    }

    /// 获取当前所有状态的一致性快照（单原子指令，物理杜绝状态撕裂）
    pub fn snapshot(&self) -> InputStateSnapshot {
        let bits = self.flags.load(Ordering::Acquire);
        InputStateSnapshot {
            is_chinese: (bits & FLAG_CHINESE) != 0,
            is_caps_locked: (bits & FLAG_CAPS_LOCKED) != 0,
            traditional_chinese: (bits & FLAG_TRADITIONAL) != 0,
            full_width_punct: (bits & FLAG_FULL_WIDTH_PUNCT) != 0,
            full_width_char: (bits & FLAG_FULL_WIDTH_CHAR) != 0,
        }
    }

    /// 内部位操作辅助函数
    fn set_bit(&self, mask: u8, val: bool) {
        if val {
            self.flags.fetch_or(mask, Ordering::Release);
        } else {
            self.flags.fetch_and(!mask, Ordering::Release);
        }
    }

    /// 查询当前是否处于中文模式
    #[inline]
    pub fn is_chinese(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & FLAG_CHINESE) != 0
    }

    /// 设置中文模式标志
    #[inline]
    pub fn set_chinese(&self, val: bool) {
        self.set_bit(FLAG_CHINESE, val);
    }

    /// 查询当前是否处于大写锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & FLAG_CAPS_LOCKED) != 0
    }

    /// 设置大写锁定标志
    #[inline]
    pub fn set_caps_locked(&self, val: bool) {
        self.set_bit(FLAG_CAPS_LOCKED, val);
    }

    /// 原子化批量更新中英模式与 CapsLock 状态（CAS 循环保障单次原子更新，杜绝瞬时组合撕裂）
    pub fn update_chinese_and_caps(&self, is_chinese: bool, is_caps_locked: bool) {
        let mut current = self.flags.load(Ordering::Acquire);
        loop {
            let mut target = current;
            if is_chinese {
                target |= FLAG_CHINESE;
            } else {
                target &= !FLAG_CHINESE;
            }
            if is_caps_locked {
                target |= FLAG_CAPS_LOCKED;
            } else {
                target &= !FLAG_CAPS_LOCKED;
            }
            match self.flags.compare_exchange_weak(
                current,
                target,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// 查询当前是否处于繁体输出模式
    #[inline]
    pub fn is_traditional(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & FLAG_TRADITIONAL) != 0
    }

    /// 设置繁体输出标志
    #[inline]
    pub fn set_traditional(&self, val: bool) {
        self.set_bit(FLAG_TRADITIONAL, val);
    }

    /// 查询当前是否处于全角标点模式
    #[inline]
    pub fn is_full_width_punct(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & FLAG_FULL_WIDTH_PUNCT) != 0
    }

    /// 设置全角标点标志
    #[inline]
    pub fn set_full_width_punct(&self, val: bool) {
        self.set_bit(FLAG_FULL_WIDTH_PUNCT, val);
    }

    /// 获取全角字符标志
    #[inline]
    pub fn is_full_width_char(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & FLAG_FULL_WIDTH_CHAR) != 0
    }

    /// 设置全角字符标志
    #[inline]
    pub fn set_full_width_char(&self, val: bool) {
        self.set_bit(FLAG_FULL_WIDTH_CHAR, val);
    }

    /// 获取底层单原子位图引用句柄
    pub fn raw_flag(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.flags)
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

        // 验证原子批量更新
        flags.update_chinese_and_caps(true, false);
        let snap3 = flags.snapshot();
        assert!(snap3.is_chinese);
        assert!(!snap3.is_caps_locked);
        assert!(flags.is_chinese());
        assert!(!flags.is_caps_locked());
    }
}
