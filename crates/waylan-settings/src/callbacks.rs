//! 偏好设置窗口业务回调注册与桌面集成
//!
//! # 设计原理
//! - **实现初衷**：将短语增删、词库异步导入、配置持久化与桌面 GNOME 指示器同步等 UI 回调注册集中收敛。
//! - **核心优势**：使 `lib.rs` 保持极简的高层装配，将复杂事件交互隔离至专用模块。

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_algo::CompactMmapLexicon;
use waylan_core::config::AppConfig;

use crate::importer::{
    import_english_dict_file, import_pinyin_dict_file, import_symbols_dict_file,
};
use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::{PhraseItem, SettingsWindow};

/// 统计与装载词库信息到窗口
pub fn populate_lexicon_statistics(
    window: &SettingsWindow,
) -> (AuxiliaryDict, Rc<VecModel<PhraseItem>>, PathBuf) {
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

    (aux_dict, phrases_model, phrase_path)
}

/// 注册通用配置、自定义短语与外部词库导入回调事件
pub fn register_settings_callbacks(
    window: &SettingsWindow,
    loaded_config: Rc<RefCell<AppConfig>>,
    aux_dict: AuxiliaryDict,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    // 1. 保存设置回调
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
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

    // 2. 恢复默认设置回调
    let window_weak = window.as_weak();
    window.on_reset_defaults(move || {
        if let Some(win) = window_weak.upgrade() {
            let default_cfg = AppConfig::default();
            sync_config_to_window(&win, &default_cfg);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from("已恢复为默认配置（点击保存后生效）"));
        }
    });

    // 3. 拼音词库导入回调
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

    // 4. 英文词库导入回调
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

    // 5. 符号词库导入回调
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

    // 6. 新增短语回调
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

    // 7. 删除短语回调
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

    // 8. 清空自适应历史词模型回调
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
}

/// 同步更新 GNOME 输入源列表与指示器隐藏扩展
pub fn sync_gnome_indicator_mode(exclusive: bool) {
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
pub fn pick_file_dialog(title: &str, filter_desc: &str, filter_patterns: &str) -> Option<PathBuf> {
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
