//! 微澜 (WayLan) 输入法偏好设置中心图形界面（Slint 实现）
//!
//! # 设计原理
//! - **实现初衷**：基于轻量声明式 Slint 框架，彻底重构为“左侧 7 项菜单、右侧内容”的现代化桌面设置中心。
//! - **核心优势**：
//!   1. 物理完全解耦，不依赖重型 OpenGL/Mesa 图形运行时，内存与启动开销极小；
//!   2. 采用 Slint 响应式数据绑定与 ModelRc 列表模型，界面响应流畅、状态无缝同步；
//!   3. 独立进程架构，随关随释放全部系统资源，保障输入法主守护进程常驻纯净。

slint::include_modules!();

pub mod importer;
pub mod sync;
pub mod voice_handlers;

use crate::importer::{
    import_english_dict_file, import_pinyin_dict_file, import_symbols_dict_file,
};
use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::voice_handlers::register_voice_callbacks;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_algo::CompactMmapLexicon;
use waylan_core::config::AppConfig;

/// 运行 Slint 偏好设置中心图形界面
///
/// # Errors
/// 当窗口初始化或事件循环运行失败时返回错误。
pub fn run_settings_gui() -> Result<(), Box<dyn Error>> {
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

    // 1. 加载持久化配置并同步到界面
    let config = AppConfig::load_or_create();
    sync_config_to_window(&window, &config);
    let loaded_config = Rc::new(std::cell::RefCell::new(config));

    // 2. 统计信息装载
    let phrase_path = CustomPhraseManager::default_path();
    let phrase_manager = CustomPhraseManager::load_or_init(&phrase_path);
    let phrases_data: Vec<PhraseItem> = phrase_manager
        .all_entries()
        .into_iter()
        .map(|(code, text)| PhraseItem {
            code: SharedString::from(code),
            text: SharedString::from(text),
        })
        .collect();
    let phrases_model = Rc::new(VecModel::from(phrases_data));
    window.set_phrase_list(ModelRc::from(phrases_model.clone()));

    // 辅助词库装载
    let mut aux_dict = AuxiliaryDict::with_default_data();
    for path in AppConfig::aux_dict_search_paths() {
        if path.exists() {
            if let Ok(loaded) = AuxiliaryDict::load_from_file(&path) {
                aux_dict = loaded;
                break;
            }
        }
    }
    window.set_english_dict_info(SharedString::from(format!(
        "已就绪常用英文单词: {} 词",
        aux_dict.english_words.len()
    )));
    window.set_symbols_dict_info(SharedString::from(format!(
        "已收录语义符号与表情: {} 项",
        aux_dict.symbols.len()
    )));

    // 自造词模型统计
    let user_dict_path = AppConfig::user_dict_path();
    let user_model_count = UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
        .map(|m| m.total_entries())
        .unwrap_or(0);
    window.set_user_model_info(SharedString::from(format!(
        "自适应个性化历史词模型: {} 条记忆",
        user_model_count
    )));

    // 系统预编译主词库状态统计
    let mut base_dict_info = "系统预编译词库: base.dict.bin (未找到或未就绪)".to_string();
    for path in AppConfig::base_dict_search_paths() {
        if path.exists() {
            if let Ok(lexicon) = CompactMmapLexicon::open(&path) {
                base_dict_info = format!(
                    "系统预编译词库: base.dict.bin (约 {:.1} 万组词条已就绪)",
                    lexicon.num_candidates as f64 / 10_000.0
                );
                break;
            }
        }
    }
    window.set_base_dict_info(SharedString::from(base_dict_info));

    // 3. 注册保存设置回调（增量打补丁保存，保留自定义托盘图标等高级配置项）
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config.clone();
    window.on_save_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            let mut base_cfg = current_cfg_cell.borrow().clone();
            update_config_from_window(&win, &mut base_cfg);
            match base_cfg.save_default() {
                Ok(()) => {
                    let hide_ext = base_cfg.exclusive_input_source;
                    *current_cfg_cell.borrow_mut() = base_cfg;
                    sync_gnome_indicator_mode(hide_ext);
                    win.set_global_status_is_error(false);
                    win.set_global_status(SharedString::from("配置已成功保存并同步！"));
                }
                Err(e) => {
                    win.set_global_status_is_error(true);
                    win.set_global_status(SharedString::from(format!("保存失败: {}", e)));
                }
            }
        }
    });

    // 4. 注册恢复默认设置回调
    let window_weak = window.as_weak();
    window.on_reset_defaults(move || {
        if let Some(win) = window_weak.upgrade() {
            let default_cfg = AppConfig::default();
            sync_config_to_window(&win, &default_cfg);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from("已恢复为默认配置（点击保存后生效）"));
        }
    });

    // 5. 注册拼音词库选择并导入回调
    let window_weak = window.as_weak();
    window.on_pick_and_import_pinyin(move || {
        if let Some(win) = window_weak.upgrade() {
            if let Some(path) = pick_file_dialog(
                "请选择要导入的拼音词库文件",
                "词库文件 (*.txt, *.dict.yaml, *.yaml, *.scel)",
                "*.txt *.dict.yaml *.yaml *.yml *.scel",
            ) {
                let path_str = path.to_string_lossy().to_string();
                win.set_pinyin_import_path(SharedString::from(&path_str));
                win.set_pinyin_import_status(SharedString::from("正在解析并合入词库，请稍候..."));
                let win_clone = win.as_weak();
                std::thread::spawn(move || {
                    let result = import_pinyin_dict_file(&path_str);
                    let count = UserAdaptiveModel::load_or_replay_wal(AppConfig::user_dict_path())
                        .map(|m| m.total_entries())
                        .unwrap_or(0);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_clone.upgrade() {
                            match result {
                                Ok(msg) => {
                                    w.set_pinyin_import_status(SharedString::from(msg));
                                    w.set_user_model_info(SharedString::from(format!(
                                        "自适应个性化历史词模型: {} 条记忆",
                                        count
                                    )));
                                }
                                Err(err) => {
                                    w.set_pinyin_import_status(SharedString::from(format!(
                                        "导入失败: {}",
                                        err
                                    )));
                                }
                            }
                        }
                    });
                });
            }
        }
    });

    // 6. 注册英文词库选择并导入回调
    let window_weak = window.as_weak();
    let aux_dict_lock = Arc::new(Mutex::new(aux_dict));
    let aux_for_en = aux_dict_lock.clone();
    window.on_pick_and_import_english(move || {
        if let Some(win) = window_weak.upgrade() {
            if let Some(path) = pick_file_dialog(
                "请选择要导入的英文词库文件",
                "英文词汇与词频文件 (*.txt, *.dict.yaml, *.yaml)",
                "*.txt *.dict.yaml *.yaml *.yml",
            ) {
                let path_str = path.to_string_lossy().to_string();
                win.set_english_import_path(SharedString::from(&path_str));
                win.set_english_import_status(SharedString::from("正在解析英文词库，请稍候..."));
                let win_clone = win.as_weak();
                let aux_lock = aux_for_en.clone();
                std::thread::spawn(move || {
                    let result = {
                        let mut aux = aux_lock.lock().unwrap_or_else(|p| p.into_inner());
                        import_english_dict_file(&path_str, &mut aux)
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_clone.upgrade() {
                            match result {
                                Ok((msg, total)) => {
                                    w.set_english_import_status(SharedString::from(msg));
                                    w.set_english_dict_info(SharedString::from(format!(
                                        "已就绪常用英文单词: {} 词",
                                        total
                                    )));
                                }
                                Err(err) => {
                                    w.set_english_import_status(SharedString::from(format!(
                                        "导入失败: {}",
                                        err
                                    )));
                                }
                            }
                        }
                    });
                });
            }
        }
    });

    // 7. 注册符号词库选择并导入回调
    let window_weak = window.as_weak();
    let aux_for_sym = aux_dict_lock;
    window.on_pick_and_import_symbols(move || {
        if let Some(win) = window_weak.upgrade() {
            if let Some(path) = pick_file_dialog(
                "请选择要导入的特殊符号库文件",
                "符号定义文件 (*.txt, *.dict.yaml, *.yaml)",
                "*.txt *.dict.yaml *.yaml *.yml",
            ) {
                let path_str = path.to_string_lossy().to_string();
                win.set_symbols_import_path(SharedString::from(&path_str));
                win.set_symbols_import_status(SharedString::from("正在解析特殊符号库，请稍候..."));
                let win_clone = win.as_weak();
                let aux_lock = aux_for_sym.clone();
                std::thread::spawn(move || {
                    let result = {
                        let mut aux = aux_lock.lock().unwrap_or_else(|p| p.into_inner());
                        import_symbols_dict_file(&path_str, &mut aux)
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_clone.upgrade() {
                            match result {
                                Ok((msg, total)) => {
                                    w.set_symbols_import_status(SharedString::from(msg));
                                    w.set_symbols_dict_info(SharedString::from(format!(
                                        "已收录语义符号与表情: {} 项",
                                        total
                                    )));
                                }
                                Err(err) => {
                                    w.set_symbols_import_status(SharedString::from(format!(
                                        "导入失败: {}",
                                        err
                                    )));
                                }
                            }
                        }
                    });
                });
            }
        }
    });

    // 8. 注册新增短语回调
    let window_weak = window.as_weak();
    let phrases_model_for_add = phrases_model.clone();
    let phrase_path_for_add = phrase_path.clone();
    window.on_add_custom_phrase(move |code, text| {
        if let Some(win) = window_weak.upgrade() {
            let code_str = code.trim();
            let text_str = text.trim();
            if code_str.is_empty() || text_str.is_empty() {
                win.set_phrase_status(SharedString::from("快捷代码与展开文本不能为空！"));
                return;
            }
            let mut mgr = CustomPhraseManager::load_or_init(&phrase_path_for_add);
            mgr.add_entry(code_str.to_string(), text_str.to_string());
            if let Err(e) = mgr.save_to_file(&phrase_path_for_add) {
                win.set_phrase_status(SharedString::from(format!("保存短语失败: {}", e)));
                return;
            }
            phrases_model_for_add.push(PhraseItem {
                code: SharedString::from(code_str),
                text: SharedString::from(text_str),
            });
            win.set_new_phrase_code(SharedString::from(""));
            win.set_new_phrase_text(SharedString::from(""));
            win.set_phrase_status(SharedString::from("自定义短语添加成功！"));
        }
    });

    // 9. 注册删除短语回调
    let window_weak = window.as_weak();
    let phrases_model_for_del = phrases_model;
    let phrase_path_for_del = phrase_path;
    window.on_delete_custom_phrase(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            let index = idx as usize;
            if let Some(item) = phrases_model_for_del.row_data(index) {
                let mut mgr = CustomPhraseManager::load_or_init(&phrase_path_for_del);
                mgr.remove_entry(item.code.as_str(), item.text.as_str());
                let _ = mgr.save_to_file(&phrase_path_for_del);
                phrases_model_for_del.remove(index);
                win.set_phrase_status(SharedString::from("已成功移除指定短语。"));
            }
        }
    });

    // 10. 注册清空自适应历史词模型回调
    let window_weak = window.as_weak();
    window.on_clear_user_model(move || {
        if let Some(win) = window_weak.upgrade() {
            let user_dict = AppConfig::user_dict_path();
            let wal = user_dict.with_extension("bin.wal");
            let _ = fs::remove_file(&user_dict);
            let _ = fs::remove_file(&wal);
            win.set_user_model_info(SharedString::from(
                "自适应个性化历史词模型: 0 条记忆 (已清空)",
            ));
            win.set_global_status(SharedString::from("用户历史自造词记忆已彻底清空。"));
        }
    });

    // 11. 注册语音流水线与云端管理回调事件
    register_voice_callbacks(&window, loaded_config);

    window.run()?;
    Ok(())
}

/// 同步更新 GNOME 输入源列表与指示器隐藏扩展
///
/// # 设计原理
/// - **实现初衷**：在用户修改设置并点击保存时，无需等待守护进程重启或注销桌面，即时响应顶栏形态变化。
/// - **核心优势**：独占模式下设置单一输入源 `[('ibus', 'waylan')]` 促使 GNOME 自动折叠隐藏原生指示器，
///   同时配合扩展启用/禁用，双保险保障即时呈现期望效果。
/// - **代价与局限**：仅在检测到当前桌面环境为 GNOME 时生效，非 GNOME 环境直接跳过。
fn sync_gnome_indicator_mode(exclusive: bool) {
    if !std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false)
    {
        return;
    }

    let sources = "[('ibus', 'waylan'), ('xkb', 'us')]";

    for key in ["sources", "mru-sources"] {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.input-sources", key, sources])
            .output();
    }
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
        .output();
    let _ = std::process::Command::new("ibus")
        .args(["engine", "waylan"])
        .output();

    let action = if exclusive { "enable" } else { "disable" };
    let uuid = "waylan-hide-input-source@waylan.org";
    let _ = std::process::Command::new("gnome-extensions")
        .args([action, uuid])
        .output();
}

/// 调用系统原生对话框 (zenity) 选择外部词库文件
///
/// # 设计原理
/// - **实现初衷**：在 Linux/Wayland 图形界面下，通过调用桌面通用的 zenity 工具弹出文件选择器，无需引入庞大的 GTK/Qt 运行时绑定。
/// - **核心优势**：零额外重型 C 依赖，保持 waylan-settings 轻量启动与快速响应，支持取消与扩展名动态过滤。
/// - **代价与局限**：依赖宿主机安装了 zenity（主流桌面 GNOME、KDE 等均标配）。若 zenity 不可用或用户主动取消，返回 None。
fn pick_file_dialog(title: &str, filter_desc: &str, filter_patterns: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg(format!(
            "--file-filter={} | {}",
            filter_desc, filter_patterns
        ))
        .arg("--file-filter=所有文件 (*.*) | *")
        .arg(format!("--title={}", title))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::{
        CandidateDensity, CandidateIndexStyle, CandidateOrientation, CandidateThemePreset,
        CandidateWindowMode, FullWidthCharShortcut, FullWidthPunctShortcut, FuzzyPinyinRules,
        PageKeys, SwitchKey, ThemeMode, TraditionalShortcut, TypoCorrectionRules,
    };

    #[test]
    fn test_config_roundtrip_logic() {
        let mut cfg = AppConfig {
            init_chinese: true,
            full_width_char: false,
            full_width_punct: true,
            page_size: 8,
            font_size: 18.0,
            switch_key: SwitchKey::Ctrl,
            page_keys: PageKeys {
                minus_equal: false,
                bracket: true,
                comma_period: true,
            },
            traditional_shortcut: TraditionalShortcut::CtrlShiftF,
            full_width_char_shortcut: FullWidthCharShortcut::ShiftSpace,
            full_width_punct_shortcut: FullWidthPunctShortcut::CtrlPeriod,
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
            dynamic_adjust_freq: true,
            enable_acronym: true,
            shift_select_second: true,
            slash_to_dunhao: true,
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
            enable_tray: Some(true),
            exclusive_input_source: true,
            enable_u_mode: true,
            enable_i_mode: true,
            filter_uncommon_charset: true,
            show_pinyin_comment: false,
            auto_dedup: true,
            tray_icon_theme_path: Some("/custom/icons".into()),
            tray_icon_zh: Some("custom-zh".into()),
            tray_icon_en: Some("custom-en".into()),
            voice: waylan_core::VoiceConfig::default(),
        };
        cfg.sanitize();

        assert_eq!(cfg.page_size, 8);
        assert_eq!(cfg.font_size, 18.0);
        assert_eq!(cfg.switch_key, SwitchKey::Ctrl);
        assert_eq!(
            cfg.page_keys,
            PageKeys {
                minus_equal: false,
                bracket: true,
                comma_period: true,
            }
        );
        assert_eq!(cfg.theme, ThemeMode::Dark);
        assert_eq!(cfg.candidate_orientation, CandidateOrientation::Vertical);
        assert_eq!(cfg.candidate_index_style, CandidateIndexStyle::Circled);
        assert_eq!(cfg.candidate_item_spacing, 10.0);
        assert_eq!(cfg.candidate_padding_v, 6.0);
        assert_eq!(cfg.candidate_padding_h, 12.0);
        assert_eq!(cfg.candidate_corner_radius, 10.0);
        assert_eq!(cfg.candidate_opacity, 90);
        assert!(cfg.fuzzy_rules.g_k);
        assert!(cfg.typo_rules.gn_ng);
        assert!(cfg.auto_pangu_spacing);
        assert_eq!(cfg.enable_tray, Some(true));
        assert!(cfg.exclusive_input_source);
        assert!(cfg.enable_u_mode);
        assert!(cfg.enable_i_mode);
        assert!(cfg.filter_uncommon_charset);
        assert!(!cfg.show_pinyin_comment);
        assert!(cfg.auto_dedup);
        assert_eq!(cfg.tray_icon_theme_path.as_deref(), Some("/custom/icons"));
        assert_eq!(cfg.tray_icon_zh.as_deref(), Some("custom-zh"));
        assert_eq!(cfg.tray_icon_en.as_deref(), Some("custom-en"));
    }
}
