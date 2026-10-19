//! 输入法运行态与外部系统托盘共享状态同步与单向广播模块
//!
//! # 设计原理
//! - **实现初衷**：将中英输入模式、CapsLock 锁定、简繁转换、全角/半角标点及字符的状态同步与外部广播逻辑内聚收敛。
//! - **核心优势**：单一真实源（Single Source of Truth）；核心会话对状态变更具备绝对权威控制，
//!   通过封装方法保证内存一致性并向共享内存标志与通知通道单向发布快照。
//! - **代价与局限**：跨线程同步依赖原子标志读取与非阻塞 channel 脉冲。

use waylan_core::state::InputStateSnapshot;
use waylan_core::InputMode;

use super::ImeRuntime;

impl ImeRuntime {
    /// 从底层权威核心状态提取当前完整的不可变输入状态快照
    #[inline]
    pub fn state_snapshot(&self) -> InputStateSnapshot {
        InputStateSnapshot {
            is_chinese: matches!(
                self.core.input_mode(),
                InputMode::Chinese | InputMode::VMode
            ),
            is_caps_locked: self.core.is_caps_locked(),
            traditional_chinese: self.core.is_traditional(),
            full_width_punct: self.core.is_full_width_punct(),
            full_width_char: self.core.is_full_width_char(),
        }
    }

    /// 向外部共享标志容器与系统托盘单向广播最新状态快照（单一真实源广播出口）
    pub fn broadcast_state_snapshot(&self) {
        let snap = self.state_snapshot();
        if let Some(ref flags) = self.shared_flags {
            flags.update_from_snapshot(&snap);
        }
        if let Some(ref tx) = self.mode_notify_tx {
            let _ = tx.try_send(());
        }
    }

    /// 设置输入语言模式并单向广播状态快照
    pub fn set_input_mode(&mut self, mode: InputMode) {
        if self.core.input_mode() != mode {
            self.core.set_input_mode(mode);
            self.broadcast_state_snapshot();
        }
    }

    /// 切换中英文模式并单向广播状态快照
    pub fn toggle_input_mode(&mut self) -> InputMode {
        let next = match self.core.input_mode() {
            InputMode::Chinese | InputMode::VMode => InputMode::English,
            InputMode::English => InputMode::Chinese,
        };
        self.set_input_mode(next);
        next
    }

    /// 设置简繁输出模式并单向广播状态快照
    pub fn set_traditional(&mut self, val: bool) {
        if self.core.is_traditional() != val {
            self.core.set_traditional(val);
            self.broadcast_state_snapshot();
        }
    }

    /// 切换简繁输出模式并单向广播状态快照
    pub fn toggle_traditional(&mut self) -> bool {
        let next = !self.core.is_traditional();
        self.set_traditional(next);
        next
    }

    /// 设置标点全半角并单向广播状态快照
    pub fn set_full_width_punct(&mut self, val: bool) {
        if self.core.is_full_width_punct() != val {
            self.core.set_full_width_punct(val);
            self.broadcast_state_snapshot();
        }
    }

    /// 切换标点全半角并单向广播状态快照
    pub fn toggle_full_width_punct(&mut self) -> bool {
        let next = !self.core.is_full_width_punct();
        self.set_full_width_punct(next);
        next
    }

    /// 设置字符全半角并单向广播状态快照
    pub fn set_full_width_char(&mut self, val: bool) {
        if self.core.is_full_width_char() != val {
            self.core.set_full_width_char(val);
            self.broadcast_state_snapshot();
        }
    }

    /// 切换字符全半角并单向广播状态快照
    pub fn toggle_full_width_char(&mut self) -> bool {
        let next = !self.core.is_full_width_char();
        self.set_full_width_char(next);
        next
    }

    /// 同步 CapsLock 大写锁定状态至核心状态机并向外部单向广播快照
    pub fn update_caps_lock_state(&mut self, is_locked: bool) {
        if self.core.is_caps_locked() != is_locked {
            self.core.set_caps_lock(is_locked);
            self.broadcast_state_snapshot();
        }
    }

    /// 同步来自系统托盘的共享中英文、大写锁定、简繁及全半角状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        let mut mode_changed = None;
        if let Some(ref flags) = self.shared_flags {
            let snap = flags.snapshot();

            // 1. 同步 CapsLock 大写锁定状态
            if self.core.is_caps_locked() != snap.is_caps_locked {
                self.core.set_caps_lock(snap.is_caps_locked);
            }

            // 2. 同步中英文输入语言模式
            let current_zh = matches!(
                self.core.input_mode(),
                InputMode::Chinese | InputMode::VMode
            );
            if snap.is_chinese != current_zh {
                let new_mode = if snap.is_chinese {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
                self.core.set_input_mode(new_mode);
                mode_changed = Some(new_mode);
            }

            // 3. 同步简繁输出模式
            if self.core.is_traditional() != snap.traditional_chinese {
                self.core.set_traditional(snap.traditional_chinese);
            }

            // 4. 同步全角/半角标点
            if self.core.is_full_width_punct() != snap.full_width_punct {
                self.core.set_full_width_punct(snap.full_width_punct);
            }

            // 5. 同步全角/半角字符
            if self.core.is_full_width_char() != snap.full_width_char {
                self.core.set_full_width_char(snap.full_width_char);
            }
        }

        if let Some(new_mode) = mode_changed {
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(new_mode);
        }

        None
    }
}
