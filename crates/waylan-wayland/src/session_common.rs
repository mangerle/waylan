//! V1 与 V2 输入法会话通用状态管理与辅助同步逻辑
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 与 V2 会话中重复的状态感知、修饰键跟踪与输入模式同步样板代码。
//! - **核心优势**：将热重载检测、原子状态同步与修饰键掩码判定收敛为单一真相源（Single Source of Truth）。
//! - **代价与局限**：具体文件监控与候选框装配已解耦至 `watcher` 与 `surface::state_builder` 独立模块。

use std::sync::atomic::{AtomicBool, Ordering};

use waylan_core::input::{update_modifier_masks, ModifierState, MOD_CAPS};

pub use crate::surface::state_builder::*;
pub use crate::watcher::*;
pub use waylan_core::input::sync_mode_from_shared_flags;

/// 判断热重载原子标志是否被触发（用于按键热路径轻量检测，避免高频系统调用）
#[inline]
pub fn is_reload_requested(reload_requested: Option<&AtomicBool>) -> bool {
    reload_requested
        .map(|flag| flag.load(Ordering::Acquire))
        .unwrap_or(false)
}

/// 根据按键事件更新修饰键位掩码
#[inline]
pub fn update_session_modifiers(
    mods_depressed: &mut u32,
    phys_mods: &mut u8,
    keycode: u32,
    is_press: bool,
) {
    update_modifier_masks(mods_depressed, phys_mods, keycode, is_press);
}

/// 判断给定的修饰键状态是否为 Shift 活跃
#[inline]
pub fn is_shift_active_state(mods_depressed: u32, phys_mods: u8) -> bool {
    ModifierState {
        depressed: mods_depressed,
        phys_mods,
        locked: 0,
    }
    .is_shift_active()
}

/// 判断给定的修饰键状态是否包含 Ctrl / Alt / Super
#[inline]
pub fn has_ctrl_or_alt_state(mods_depressed: u32, phys_mods: u8) -> bool {
    ModifierState {
        depressed: mods_depressed,
        phys_mods,
        locked: 0,
    }
    .has_ctrl_or_alt()
}

/// 判断给定的锁定掩码是否处于 CapsLock 大写锁定状态
#[inline]
pub fn is_caps_locked_state(mods_locked: u32) -> bool {
    (mods_locked & MOD_CAPS) != 0
}

/// 外部按键事件拦截处理结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFilterResult {
    /// 未拦截该按键，继续按默认输入法流程处理
    PassThrough,
    /// 该按键已被外部处理程序消费，不进入拼音/标点管线，亦不透传给客户端应用
    Consumed,
    /// 拦截消费并要求直接向当前客户端提交文本
    Commit(String),
}

/// 外部按键拦截回调函数类型，入参为 (evdev_keycode, is_press, mods_depressed)
pub type KeyFilterCallback = Box<dyn FnMut(u32, bool, u32) -> KeyFilterResult + Send>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use waylan_core::state::SharedInputFlags;
    use waylan_core::InputMode;

    #[test]
    fn test_sync_mode_from_shared_flags() {
        let flags = SharedInputFlags::new(true, false, false, false);
        let mut mode = InputMode::Chinese;

        flags.set_chinese(false);
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
        assert_eq!(changed, Some(InputMode::English));
        assert_eq!(mode, InputMode::English);

        // 再次同步，无变化
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
        assert_eq!(changed, None);
        assert_eq!(mode, InputMode::English);

        // 切换回中文
        flags.set_chinese(true);
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
        assert_eq!(changed, Some(InputMode::Chinese));
        assert_eq!(mode, InputMode::Chinese);
    }

    #[test]
    fn test_check_reload_requested() {
        let flag = AtomicBool::new(true);
        let mut watcher = None;
        let paths: Vec<std::path::PathBuf> = Vec::new();
        let mut mtime = None;

        let needed =
            check_reload_requested_or_files_changed(Some(&flag), &mut watcher, &paths, &mut mtime);
        assert!(needed);
        assert!(!flag.load(Ordering::Relaxed)); // 验证已重置为 false
    }
}
