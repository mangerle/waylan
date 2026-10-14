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

use crate::importer::{
    import_english_dict_file, import_pinyin_dict_file, import_symbols_dict_file,
};
use crate::sync::{sync_config_to_window, update_config_from_window};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::error::Error;
use std::fs;
use std::rc::Rc;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::AppConfig;

/// 运行 Slint 偏好设置中心图形界面
///
/// # Errors
/// 当窗口初始化或事件循环运行失败时返回错误。
pub fn run_settings_gui() -> Result<(), Box<dyn Error>> {
    let window = SettingsWindow::new()?;

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
                    win.set_global_status(SharedString::from("配置已成功保存并同步！"));
                }
                Err(e) => {
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
            win.set_global_status(SharedString::from("已恢复为默认配置（点击保存后生效）"));
        }
    });

    // 5. 注册拼音词库导入回调
    let window_weak = window.as_weak();
    window.on_import_pinyin_dict(move |path| {
        if let Some(win) = window_weak.upgrade() {
            match import_pinyin_dict_file(path.as_str()) {
                Ok(msg) => {
                    win.set_pinyin_import_status(SharedString::from(msg));
                    let count = UserAdaptiveModel::load_or_replay_wal(AppConfig::user_dict_path())
                        .map(|m| m.total_entries())
                        .unwrap_or(0);
                    win.set_user_model_info(SharedString::from(format!(
                        "自适应个性化历史词模型: {} 条记忆",
                        count
                    )));
                }
                Err(err) => {
                    win.set_pinyin_import_status(SharedString::from(format!("导入失败: {}", err)));
                }
            }
        }
    });

    // 6. 注册英文词库导入回调
    let window_weak = window.as_weak();
    let aux_dict_cell = Rc::new(std::cell::RefCell::new(aux_dict));
    let aux_for_en = aux_dict_cell.clone();
    window.on_import_english_dict(move |path| {
        if let Some(win) = window_weak.upgrade() {
            let mut aux = aux_for_en.borrow_mut();
            match import_english_dict_file(path.as_str(), &mut aux) {
                Ok((msg, total)) => {
                    win.set_english_import_status(SharedString::from(msg));
                    win.set_english_dict_info(SharedString::from(format!(
                        "已就绪常用英文单词: {} 词",
                        total
                    )));
                }
                Err(err) => {
                    win.set_english_import_status(SharedString::from(format!("导入失败: {}", err)));
                }
            }
        }
    });

    // 7. 注册符号词库导入回调
    let window_weak = window.as_weak();
    let aux_for_sym = aux_dict_cell;
    window.on_import_symbols_dict(move |path| {
        if let Some(win) = window_weak.upgrade() {
            let mut aux = aux_for_sym.borrow_mut();
            match import_symbols_dict_file(path.as_str(), &mut aux) {
                Ok((msg, total)) => {
                    win.set_symbols_import_status(SharedString::from(msg));
                    win.set_symbols_dict_info(SharedString::from(format!(
                        "已收录语义符号与表情: {} 项",
                        total
                    )));
                }
                Err(err) => {
                    win.set_symbols_import_status(SharedString::from(format!("导入失败: {}", err)));
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::{
        CandidateIndexStyle, CandidateOrientation, FuzzyPinyinRules, PageKeys, SwitchKey,
        ThemeMode, TypoCorrectionRules,
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
            theme: ThemeMode::Dark,
            candidate_orientation: CandidateOrientation::Vertical,
            candidate_index_style: CandidateIndexStyle::Circled,
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
