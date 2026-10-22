//! 微澜 (WayLan) 输入法偏好设置中心图形界面（Slint 实现）
//!
//! # 设计原理
//! - **实现初衷**：基于轻量声明式 Slint 框架，彻底重构为“左侧 7 项菜单、右侧内容”的现代化桌面设置中心。
//! - **核心优势**：
//!   1. 物理完全解耦，不依赖重型 OpenGL/Mesa 图形运行时，内存与启动开销极小；
//!   2. 采用 Slint 响应式数据绑定与 ModelRc 列表模型，界面响应流畅、状态无缝同步；
//!   3. 独立进程架构，随关随释放全部系统资源，保障输入法主守护进程常驻纯净。

slint::include_modules!();

pub mod callbacks;
pub mod importer;
pub mod sync;
pub mod voice;
pub use voice as voice_handlers;

pub use importer::ImportError;

use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;

use crate::callbacks::{
    populate_lexicon_statistics, register_settings_callbacks, SettingsCallbacksContext,
};
use crate::sync::sync_config_to_window;
use crate::voice::register_voice_callbacks;

/// 设置中心运行与初始化强类型错误枚举
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// Slint 窗口或平台渲染层初始化失败
    #[error("Slint 图形界面平台错误: {0}")]
    Platform(#[from] slint::PlatformError),

    /// 词库或配置文件底层 I/O 错误
    #[error("设置中心 I/O 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 运行 Slint 偏好设置中心图形界面
///
/// # Errors
/// 当窗口初始化或事件循环运行失败时返回错误。
pub fn run_settings_gui() -> Result<(), SettingsError> {
    let window = SettingsWindow::new()?;

    // 设置 XDG 应用标识（app_id / WM_CLASS），必须在窗口实例初始化后、首次呈现到桌面之前配置。
    // 确保 Wayland 合成器（如 GNOME Mutter、KWin、Sway）将运行期窗口与 waylan-settings.desktop 权威关联，
    // 从而使桌面环境 Dock 栏、Alt-Tab 任务切换器即刻匹配并显示微澜专属矢量应用图标，消除默认灰色齿轮。
    let _ = slint::set_xdg_app_id("waylan-settings");

    // 显式预设窗口初始逻辑尺寸（920x640），为多列自适应卡片与紧凑主题微调提供舒展视口，避免产生横向挤压与不必要滚动条
    window
        .window()
        .set_size(slint::LogicalSize::new(920.0, 640.0));

    window.set_app_version(SharedString::from(env!("CARGO_PKG_VERSION")));

    // 注册全局快捷键按键捕获与格式化回调
    let shortcut_helper = window.global::<ShortcutHelper>();
    shortcut_helper.on_format_key(|ctrl, alt, shift, meta, text| {
        let modifiers = waylan_core::config::KeyModifiers::new(ctrl, alt, shift, meta);
        match waylan_core::config::format_key_combination(&modifiers, text.as_str()) {
            Some(formatted) => SharedString::from(formatted),
            None => SharedString::from(""),
        }
    });

    // 1. 加载持久化配置并同步到界面
    let config = AppConfig::load_or_create();
    sync_config_to_window(&window, &config);
    let loaded_config = Arc::new(Mutex::new(config));

    // 2. 统计信息装载
    let stats = populate_lexicon_statistics(&window);

    // 3. 注册常规配置、短语与词库事件回调
    let callbacks_context = SettingsCallbacksContext {
        loaded_config: loaded_config.clone(),
        stats,
    };
    register_settings_callbacks(&window, callbacks_context);

    // 4. 注册语音流水线与云端管理回调事件
    register_voice_callbacks(&window, loaded_config);

    window.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::{
        CandidateDensity, CandidateIndexStyle, CandidateOrientation, CandidateThemePreset,
        CandidateWindowMode, DesktopConfig, FuzzyPinyinRules, PageKeys, PinyinConfig,
        ShortcutConfig, SwitchKey, ThemeMode, TypoCorrectionRules, UiConfig,
    };

    #[test]
    fn test_config_roundtrip_logic() {
        let mut cfg = AppConfig {
            ui: UiConfig {
                page_size: 8,
                font_size: 18.0,
                theme: ThemeMode::Dark,
                candidate_orientation: CandidateOrientation::Vertical,
                candidate_index_style: CandidateIndexStyle::Circled,
                candidate_window_mode: CandidateWindowMode::WaylanCustom,
                candidate_density: CandidateDensity::Normal,
                candidate_theme: CandidateThemePreset::Emerald,
                candidate_item_spacing: 10.0,
                candidate_padding_v: 6.0,
                candidate_padding_h: 12.0,
                candidate_corner_radius: 10.0,
                candidate_opacity: 90,
                show_pinyin_comment: false,
            },
            shortcuts: ShortcutConfig {
                switch_key: SwitchKey::Ctrl,
                page_keys: PageKeys {
                    minus_equal: false,
                    bracket: true,
                    comma_period: true,
                },
                traditional_shortcut: "Ctrl + Shift + F".to_string(),
                full_width_char_shortcut: "Shift + Space".to_string(),
                full_width_punct_shortcut: "Ctrl + .".to_string(),
                shift_select_second: true,
            },
            pinyin: PinyinConfig {
                init_chinese: true,
                full_width_char: false,
                full_width_punct: true,
                dynamic_adjust_freq: true,
                enable_acronym: true,
                slash_to_dunhao: true,
                backslash_to_dunhao: true,
                protect_digits_punctuation: true,
                auto_complete_pairs: true,
                v_mode: true,
                fuzzy_pinyin: true,
                fuzzy_rules: FuzzyPinyinRules {
                    c_ch: true,
                    g_k: true,
                    ..Default::default()
                },
                typo_rules: TypoCorrectionRules {
                    enable: true,
                    gn_ng: true,
                    ..Default::default()
                },
                traditional_chinese: false,
                auto_pangu_spacing: true,
                enable_u_mode: true,
                enable_i_mode: true,
                filter_uncommon_charset: true,
                auto_dedup: true,
            },
            desktop: DesktopConfig {
                enable_tray: waylan_core::TrayDisplayMode::Always,
                exclusive_input_source: true,
                tray_icon_theme_path: Some("/custom/icons".into()),
                tray_icon_zh: Some("custom-zh".into()),
                tray_icon_en: Some("custom-en".into()),
                ..Default::default()
            },
            voice: waylan_core::VoiceConfig::default(),
        };
        cfg.sanitize();

        assert_eq!(cfg.ui.page_size, 8);
        assert_eq!(cfg.ui.font_size, 18.0);
        assert_eq!(cfg.shortcuts.switch_key, SwitchKey::Ctrl);
        assert_eq!(
            cfg.shortcuts.page_keys,
            PageKeys {
                minus_equal: false,
                bracket: true,
                comma_period: true,
            }
        );
        assert_eq!(cfg.ui.theme, ThemeMode::Dark);
        assert_eq!(cfg.ui.candidate_orientation, CandidateOrientation::Vertical);
        assert_eq!(cfg.ui.candidate_index_style, CandidateIndexStyle::Circled);
        assert_eq!(cfg.ui.candidate_item_spacing, 10.0);
        assert_eq!(cfg.ui.candidate_padding_v, 6.0);
        assert_eq!(cfg.ui.candidate_padding_h, 12.0);
        assert_eq!(cfg.ui.candidate_corner_radius, 10.0);
        assert_eq!(cfg.ui.candidate_opacity, 90);
        assert!(cfg.pinyin.fuzzy_rules.g_k);
        assert!(cfg.pinyin.typo_rules.gn_ng);
        assert!(cfg.pinyin.auto_pangu_spacing);
        assert_eq!(
            cfg.desktop.enable_tray,
            waylan_core::TrayDisplayMode::Always
        );
        assert!(cfg.desktop.exclusive_input_source);
        assert!(cfg.pinyin.enable_u_mode);
        assert!(cfg.pinyin.enable_i_mode);
        assert!(cfg.pinyin.filter_uncommon_charset);
        assert!(!cfg.ui.show_pinyin_comment);
        assert!(cfg.pinyin.auto_dedup);
        assert_eq!(
            cfg.desktop.tray_icon_theme_path.as_deref(),
            Some("/custom/icons")
        );
        assert_eq!(cfg.desktop.tray_icon_zh.as_deref(), Some("custom-zh"));
        assert_eq!(cfg.desktop.tray_icon_en.as_deref(), Some("custom-en"));
    }
}
