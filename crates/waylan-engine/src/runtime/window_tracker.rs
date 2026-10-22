//! 窗口级输入模式状态跟踪与上下文隔离模块
//!
//! # 设计原理
//! - **实现初衷**：为不同应用窗口（如终端、代码编辑器、浏览器）提供独立的输入模式记忆，
//!   解决跨窗口频繁手动按键切换中英文的痛点。
//! - **核心优势**：窗口隔离与防内存泄漏（固定容量淘汰机制，默认最多保留 256 个窗口状态）；
//!   对新窗口提供一致的默认模式，窗口失焦/关闭时安全清理。
//! - **代价与局限**：依赖平台前端提供稳定的上下文标识（如 IBus client_path 或 Wayland surface）。

use std::collections::HashMap;
use waylan_core::InputMode;

/// 窗口级输入状态上下文跟踪器
#[derive(Debug, Clone)]
pub struct WindowModeTracker {
    /// 窗口唯一标识到基础输入模式的映射表
    modes: HashMap<String, InputMode>,
    /// 当前获焦的活动窗口标识
    active_window: Option<String>,
    /// 最大缓存窗口数（防止长期运行无上限内存积压）
    max_capacity: usize,
    /// 默认初始输入模式
    default_mode: InputMode,
}

impl Default for WindowModeTracker {
    fn default() -> Self {
        Self::new(256, InputMode::Chinese)
    }
}

impl WindowModeTracker {
    /// 创建带有指定容量与默认模式的窗口跟踪器
    pub fn new(max_capacity: usize, default_mode: InputMode) -> Self {
        Self {
            modes: HashMap::with_capacity(max_capacity.min(64)),
            active_window: None,
            max_capacity,
            default_mode,
        }
    }

    /// 获取当前活动窗口标识
    #[inline]
    pub fn active_window(&self) -> Option<&str> {
        self.active_window.as_deref()
    }

    /// 切换当前活动窗口并返回该窗口记录的基础输入模式
    pub fn focus_window(&mut self, window_id: &str) -> InputMode {
        self.active_window = Some(window_id.to_string());
        if let Some(&mode) = self.modes.get(window_id) {
            mode
        } else {
            // 防御性容量检查：若超过上限，清理最早遍历到的条目
            if self.modes.len() >= self.max_capacity {
                if let Some(first_key) = self.modes.keys().next().cloned() {
                    self.modes.remove(&first_key);
                }
            }
            self.modes.insert(window_id.to_string(), self.default_mode);
            self.default_mode
        }
    }

    /// 窗口失去焦点时通知
    pub fn blur_window(&mut self, window_id: &str) {
        if self.active_window.as_deref() == Some(window_id) {
            self.active_window = None;
        }
    }

    /// 窗口销毁时清理状态映射
    pub fn remove_window(&mut self, window_id: &str) {
        self.modes.remove(window_id);
        if self.active_window.as_deref() == Some(window_id) {
            self.active_window = None;
        }
    }

    /// 更新当前活动窗口的基础输入模式
    pub fn update_active_mode(&mut self, mode: InputMode) {
        if let Some(ref win) = self.active_window {
            self.modes.insert(win.clone(), mode);
        }
    }

    /// 获取当前活动窗口记录的模式，若无活动窗口则返回默认模式
    pub fn current_base_mode(&self) -> InputMode {
        if let Some(ref win) = self.active_window {
            self.modes.get(win).copied().unwrap_or(self.default_mode)
        } else {
            self.default_mode
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_mode_tracker_isolation() {
        let mut tracker = WindowModeTracker::new(10, InputMode::Chinese);

        // 1. 窗口 A 获焦，默认为中文
        let mode_a = tracker.focus_window("win_terminal");
        assert_eq!(mode_a, InputMode::Chinese);

        // 2. 在窗口 A 切换为英文
        tracker.update_active_mode(InputMode::English);
        assert_eq!(tracker.current_base_mode(), InputMode::English);

        // 3. 切换至窗口 B，默认为中文
        let mode_b = tracker.focus_window("win_browser");
        assert_eq!(mode_b, InputMode::Chinese);
        assert_eq!(tracker.current_base_mode(), InputMode::Chinese);

        // 4. 切回窗口 A，恢复此前保存的英文
        let mode_a_back = tracker.focus_window("win_terminal");
        assert_eq!(mode_a_back, InputMode::English);
        assert_eq!(tracker.current_base_mode(), InputMode::English);

        // 5. 销毁窗口 A
        tracker.remove_window("win_terminal");
        assert_eq!(tracker.active_window(), None);
    }
}
