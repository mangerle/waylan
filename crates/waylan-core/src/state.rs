//! 输入法会话与托盘/指示器间共享的原子状态容器
//!
//! # 设计原理
//! - **实现初衷**：聚合散落在输入法会话（Wayland/IBus）与系统托盘之间的独立状态，消除分散状态传递与更新时序不一致隐患。
//! - **核心优势**：
//!   1. 内部采用单原子位图 (`AtomicU8`) 统一驱动，`snapshot()` 仅需单次原子指令，从物理上杜绝状态撕裂 (State Tearing)；
//!   2. 支持单次 CAS 原子批量更新中英模式与 CapsLock 状态，避免瞬时非法状态组合；
//!   3. 零多余堆内存分配，无需双写同步。
//! - **代价与局限**：状态受限于 8 位位图（最多 8 种布尔标志组合）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

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

/// 聚合的输入法状态原子标志容器（基于单原子位图与 UI 待消费掩码实现，杜绝状态撕裂与双端写覆盖）
///
/// # 设计原理
/// - 低 8 位存储当前共享状态位（中文、CapsLock、繁体、全角标点、全角字符）；
/// - 高 8 位存储外部 UI（系统托盘）刚发起修改但尚未被运行时 `consume_pending_snapshot` 消费的脏位掩码 (`pending_mask`)；
/// - 运行时主线程调用 `update_from_snapshot` 广播自身状态时，仅覆写尚未挂起外部 UI 修改的位，彻底消除双端写竞态覆盖。
#[derive(Debug, Clone)]
pub struct SharedInputFlags {
    /// 低 8 位为状态位，高 8 位为外部 UI 待消费掩码
    flags: Arc<AtomicU16>,
}

impl SharedInputFlags {
    /// 从原始低 8 位位图解码只读状态快照
    #[inline]
    fn decode_snapshot(raw: u16) -> InputStateSnapshot {
        let bits = (raw & 0x00FF) as u8;
        InputStateSnapshot {
            is_chinese: (bits & FLAG_CHINESE) != 0,
            is_caps_locked: (bits & FLAG_CAPS_LOCKED) != 0,
            traditional_chinese: (bits & FLAG_TRADITIONAL) != 0,
            full_width_punct: (bits & FLAG_FULL_WIDTH_PUNCT) != 0,
            full_width_char: (bits & FLAG_FULL_WIDTH_CHAR) != 0,
        }
    }

    /// 将状态快照编码为低 8 位位图
    #[inline]
    fn encode_snapshot(snap: &InputStateSnapshot) -> u8 {
        let mut bits = 0u8;
        if snap.is_chinese {
            bits |= FLAG_CHINESE;
        }
        if snap.is_caps_locked {
            bits |= FLAG_CAPS_LOCKED;
        }
        if snap.traditional_chinese {
            bits |= FLAG_TRADITIONAL;
        }
        if snap.full_width_punct {
            bits |= FLAG_FULL_WIDTH_PUNCT;
        }
        if snap.full_width_char {
            bits |= FLAG_FULL_WIDTH_CHAR;
        }
        bits
    }

    /// 创建新的共享输入状态标志容器
    pub fn new(
        init_chinese: bool,
        traditional_chinese: bool,
        full_width_punct: bool,
        full_width_char: bool,
    ) -> Self {
        let bits = Self::encode_snapshot(&InputStateSnapshot {
            is_chinese: init_chinese,
            is_caps_locked: false,
            traditional_chinese,
            full_width_punct,
            full_width_char,
        });

        Self {
            flags: Arc::new(AtomicU16::new(u16::from(bits))),
        }
    }

    /// 获取当前所有状态的一致性快照（单原子指令，物理杜绝状态撕裂）
    pub fn snapshot(&self) -> InputStateSnapshot {
        Self::decode_snapshot(self.flags.load(Ordering::Acquire))
    }

    /// 内部位操作辅助函数：更新目标状态位并在高 8 位标记对应的待消费掩码
    fn set_bit(&self, mask: u8, val: bool) {
        let bit = u16::from(mask);
        let pending_bit = bit << 8;
        let mut current = self.flags.load(Ordering::Acquire);
        loop {
            let next = if val {
                (current | bit) | pending_bit
            } else {
                (current & !bit) | pending_bit
            };
            match self.flags.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// 查询当前是否处于中文模式
    #[inline]
    pub fn is_chinese(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & u16::from(FLAG_CHINESE)) != 0
    }

    /// 设置中文模式标志
    #[inline]
    pub fn set_chinese(&self, val: bool) {
        self.set_bit(FLAG_CHINESE, val);
    }

    /// 查询当前是否处于大写锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & u16::from(FLAG_CAPS_LOCKED)) != 0
    }

    /// 设置大写锁定标志
    #[inline]
    pub fn set_caps_locked(&self, val: bool) {
        self.set_bit(FLAG_CAPS_LOCKED, val);
    }

    /// 原子化批量更新中英模式与 CapsLock 状态（CAS 循环保障单次原子更新并登记待消费掩码）
    pub fn update_chinese_and_caps(&self, is_chinese: bool, is_caps_locked: bool) {
        let pending_bits = u16::from(FLAG_CHINESE | FLAG_CAPS_LOCKED) << 8;
        let mut current = self.flags.load(Ordering::Acquire);
        loop {
            let mut target = current | pending_bits;
            if is_chinese {
                target |= u16::from(FLAG_CHINESE);
            } else {
                target &= !u16::from(FLAG_CHINESE);
            }
            if is_caps_locked {
                target |= u16::from(FLAG_CAPS_LOCKED);
            } else {
                target &= !u16::from(FLAG_CAPS_LOCKED);
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
        (self.flags.load(Ordering::Acquire) & u16::from(FLAG_TRADITIONAL)) != 0
    }

    /// 设置繁体输出标志
    #[inline]
    pub fn set_traditional(&self, val: bool) {
        self.set_bit(FLAG_TRADITIONAL, val);
    }

    /// 查询当前是否处于全角标点模式
    #[inline]
    pub fn is_full_width_punct(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & u16::from(FLAG_FULL_WIDTH_PUNCT)) != 0
    }

    /// 设置全角标点标志
    #[inline]
    pub fn set_full_width_punct(&self, val: bool) {
        self.set_bit(FLAG_FULL_WIDTH_PUNCT, val);
    }

    /// 获取全角字符标志
    #[inline]
    pub fn is_full_width_char(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & u16::from(FLAG_FULL_WIDTH_CHAR)) != 0
    }

    /// 设置全角字符标志
    #[inline]
    pub fn set_full_width_char(&self, val: bool) {
        self.set_bit(FLAG_FULL_WIDTH_CHAR, val);
    }

    /// 从完整输入状态快照原子更新非挂起标志位（保留尚未被运行时消费的外部 UI 更改，杜绝并发写覆盖）
    pub fn update_from_snapshot(&self, snap: &InputStateSnapshot) {
        let desired_bits = u16::from(Self::encode_snapshot(snap));
        let mut current = self.flags.load(Ordering::Acquire);
        loop {
            let pending_mask = (current >> 8) & 0x00FF;
            let preserved = current & pending_mask;
            let updated = (desired_bits & !pending_mask) | preserved;
            let next = (current & 0xFF00) | updated;
            match self.flags.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// 原子读取最新状态快照并清空高 8 位外部 UI 待消费掩码（供运行时主循环 `sync_shared_mode` 消费）
    pub fn consume_pending_snapshot(&self) -> InputStateSnapshot {
        let mut current = self.flags.load(Ordering::Acquire);
        loop {
            let next = current & 0x00FF;
            match self.flags.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Self::decode_snapshot(current),
                Err(actual) => current = actual,
            }
        }
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

        // 验证从完整快照原子全量更新
        let custom_snap = InputStateSnapshot {
            is_chinese: false,
            is_caps_locked: true,
            traditional_chinese: true,
            full_width_punct: false,
            full_width_char: true,
        };
        // 消费挂起标志后，运行时快照方可完整覆写所有位
        let _ = flags.consume_pending_snapshot();
        flags.update_from_snapshot(&custom_snap);
        assert_eq!(flags.snapshot(), custom_snap);
    }

    #[test]
    fn test_shared_input_flags_prevents_overwrite_before_consume() {
        let flags = SharedInputFlags::new(true, false, true, false);
        // 托盘线程触发繁体切换（置入 pending_mask）
        flags.set_traditional(true);

        // 此时运行时主线程尚未来得及消费该托盘事件，同时广播旧快照 traditional_chinese=false
        let stale_runtime_snap = InputStateSnapshot {
            is_chinese: true,
            is_caps_locked: false,
            traditional_chinese: false,
            full_width_punct: true,
            full_width_char: false,
        };
        flags.update_from_snapshot(&stale_runtime_snap);

        assert!(
            flags.is_traditional(),
            "未被消费的托盘 UI 切换位绝不能被运行时旧快照覆盖"
        );

        let consumed = flags.consume_pending_snapshot();
        assert!(consumed.traditional_chinese);

        flags.update_from_snapshot(&stale_runtime_snap);
        assert!(
            !flags.is_traditional(),
            "消费完挂起位后，运行时快照应正常生效"
        );
    }
}
