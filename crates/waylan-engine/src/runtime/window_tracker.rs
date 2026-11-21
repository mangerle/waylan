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

/// 判定给定的应用标识（app_id / client / wm_class）是否属于终端类应用
pub fn is_terminal_app(app_identifier: &str) -> bool {
    let lower = app_identifier.to_ascii_lowercase();
    lower.contains("terminal")
        || lower.contains("pty")
        || lower.contains("alacritty")
        || lower.contains("kitty")
        || lower.contains("wezterm")
        || lower.contains("konsole")
        || lower.contains("terminator")
        || lower.contains("tilix")
        || lower.contains("tabby")
        || lower.contains("xterm")
        || lower.contains("foot")
        || lower.contains("urxvt")
}

/// 判定给定的应用标识是否命中用户自定义的默认英文应用列表（忽略大小写子串匹配）
pub fn matches_english_app_list(app_identifier: &str, custom_apps: &[String]) -> bool {
    if custom_apps.is_empty() {
        return false;
    }
    let lower = app_identifier.to_ascii_lowercase();
    custom_apps.iter().any(|pattern| {
        let trimmed = pattern.trim();
        !trimmed.is_empty() && lower.contains(&trimmed.to_ascii_lowercase())
    })
}

/// 窗口获焦时的默认输入模式判定策略参数
#[derive(Debug, Clone, Copy)]
pub struct FocusWindowPolicy<'a> {
    /// 可选的应用名称/类别（如 wm_class、app_id 或 client_name）
    pub app_name: Option<&'a str>,
    /// 终端类应用首次获焦时是否默认使用英文模式
    pub terminal_default_english: bool,
    /// 用户自定义的默认英文应用标识列表（如 code、zed、idea、obsidian）
    pub custom_english_apps: &'a [String],
    /// 是否启用多窗口输入模式独立记忆
    pub window_mode_memory: bool,
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
    ///
    /// - `window_id`: 窗口或输入上下文唯一标识
    /// - `app_name`: 可选的应用名称/类别（如 wm_class 或 client_name）
    /// - `terminal_default_english`: 当该窗口为终端且首次创建时，是否默认使用英文模式
    /// - `window_mode_memory`: 是否启用多窗口模式独立记忆
    pub fn focus_window(
        &mut self,
        window_id: &str,
        app_name: Option<&str>,
        terminal_default_english: bool,
        window_mode_memory: bool,
    ) -> InputMode {
        self.focus_window_with_policy(
            window_id,
            FocusWindowPolicy {
                app_name,
                terminal_default_english,
                custom_english_apps: &[],
                window_mode_memory,
            },
        )
    }

    /// 按策略参数切换活动窗口并返回该窗口记录的基础输入模式
    pub fn focus_window_with_policy(
        &mut self,
        window_id: &str,
        policy: FocusWindowPolicy<'_>,
    ) -> InputMode {
        self.active_window = Some(window_id.to_string());
        if !policy.window_mode_memory {
            return self.default_mode;
        }
        if let Some(&mode) = self.modes.get(window_id) {
            mode
        } else {
            // 防御性容量检查：若超过上限，清理最早遍历到的条目
            if self.modes.len() >= self.max_capacity {
                if let Some(first_key) = self.modes.keys().next().cloned() {
                    self.modes.remove(&first_key);
                }
            }
            let default_to_english = policy.app_name.is_some_and(|app| {
                (policy.terminal_default_english && is_terminal_app(app))
                    || matches_english_app_list(app, policy.custom_english_apps)
            });
            let initial_mode = if default_to_english {
                InputMode::English
            } else {
                self.default_mode
            };
            self.modes.insert(window_id.to_string(), initial_mode);
            initial_mode
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

        // 1. 普通应用窗口获焦，默认为中文
        let mode_browser = tracker.focus_window("win_1", Some("google-chrome"), true, true);
        assert_eq!(mode_browser, InputMode::Chinese);

        // 2. 切换为英文
        tracker.update_active_mode(InputMode::English);
        assert_eq!(tracker.current_base_mode(), InputMode::English);

        // 3. 终端窗口获焦，因 terminal_default_english=true，初始即为英文
        let mode_term = tracker.focus_window("win_2", Some("gnome-terminal-server"), true, true);
        assert_eq!(mode_term, InputMode::English);
        assert_eq!(tracker.current_base_mode(), InputMode::English);

        // 4. 切回浏览器窗口，恢复此前保存的英文
        let mode_browser_back = tracker.focus_window("win_1", Some("google-chrome"), true, true);
        assert_eq!(mode_browser_back, InputMode::English);

        // 5. 在浏览器中切为中文
        tracker.update_active_mode(InputMode::Chinese);
        assert_eq!(tracker.current_base_mode(), InputMode::Chinese);

        // 6. 切回终端窗口，依然保持英文
        let mode_term_back =
            tracker.focus_window("win_2", Some("gnome-terminal-server"), true, true);
        assert_eq!(mode_term_back, InputMode::English);

        // 7. 测试未开启终端默认英文时，终端初始为中文
        let mode_term_disabled = tracker.focus_window("win_3", Some("alacritty"), false, true);
        assert_eq!(mode_term_disabled, InputMode::Chinese);

        // 8. 测试未开启窗口模式记忆时，始终返回默认模式
        let mode_no_mem = tracker.focus_window("win_4", Some("code"), true, false);
        assert_eq!(mode_no_mem, InputMode::Chinese);

        // 9. 销毁当前活动窗口
        tracker.remove_window("win_4");
        assert_eq!(tracker.active_window(), None);
    }

    #[test]
    fn test_is_terminal_app_detection() {
        assert!(is_terminal_app("gnome-terminal-server"));
        assert!(is_terminal_app("Alacritty"));
        assert!(is_terminal_app("kitty"));
        assert!(is_terminal_app("wezterm-gui"));
        assert!(is_terminal_app("tabby"));
        assert!(is_terminal_app("konsole"));
        assert!(is_terminal_app("foot"));
        assert!(is_terminal_app("xterm"));
        assert!(is_terminal_app("pty/1"));
        assert!(!is_terminal_app("google-chrome"));
        assert!(!is_terminal_app("code"));
        assert!(!is_terminal_app("gedit"));
    }

    #[test]
    fn test_custom_default_english_apps_policy() {
        let mut tracker = WindowModeTracker::new(10, InputMode::Chinese);
        let custom = vec![
            "code".to_string(),
            "zed".to_string(),
            "idea".to_string(),
            "obsidian".to_string(),
        ];

        let mode_code = tracker.focus_window_with_policy(
            "win_code",
            FocusWindowPolicy {
                app_name: Some("Code - OSS"),
                terminal_default_english: false,
                custom_english_apps: &custom,
                window_mode_memory: true,
            },
        );
        assert_eq!(mode_code, InputMode::English);

        let mode_zed = tracker.focus_window_with_policy(
            "win_zed",
            FocusWindowPolicy {
                app_name: Some("dev.zed.Zed"),
                terminal_default_english: false,
                custom_english_apps: &custom,
                window_mode_memory: true,
            },
        );
        assert_eq!(mode_zed, InputMode::English);

        let mode_browser = tracker.focus_window_with_policy(
            "win_chrome",
            FocusWindowPolicy {
                app_name: Some("google-chrome"),
                terminal_default_english: false,
                custom_english_apps: &custom,
                window_mode_memory: true,
            },
        );
        assert_eq!(mode_browser, InputMode::Chinese);
    }
}
