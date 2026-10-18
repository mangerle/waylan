//! 偏好设置窗口业务回调注册与装配
//!
//! # 设计原理
//! - **实现初衷**：收敛短语增删、词库异步导入挂载、配置持久化与桌面 GNOME 指示器同步等 UI 回调注册。
//! - **核心优势**：按职责裂变为子模块（desktop / tasks），本模块仅保留轻量高层回调装配，复杂度严格受控。
//! - **代价与局限**：回调函数需捕获 Slint 窗口的 Weak 引用进行跨线程安全更新。

pub mod desktop;
pub mod tasks;

pub use desktop::{
    pick_directory_dialog, pick_directory_dialog_async, pick_file_dialog, pick_file_dialog_async,
    sync_gnome_indicator_mode,
};
pub(crate) use tasks::TaskRunningGuard;

use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_algo::CompactMmapLexicon;
use waylan_core::config::AppConfig;

use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::{PhraseItem, SettingsWindow};
use tasks::{spawn_english_import_task, spawn_pinyin_import_task, spawn_symbols_import_task};

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
    loaded_config: Arc<Mutex<AppConfig>>,
    aux_dict: AuxiliaryDict,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    // 1. 保存设置回调
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_save_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            let mut base_cfg = current_cfg_cell
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            update_config_from_window(&win, &mut base_cfg);
            match base_cfg.save_default() {
                Ok(()) => {
                    let hide_ext = base_cfg.desktop.exclusive_input_source;
                    if let Ok(mut g) = current_cfg_cell.lock() {
                        *g = base_cfg;
                    }
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
    let is_pinyin_importing = Arc::new(AtomicBool::new(false));
    let pinyin_running = is_pinyin_importing;
    let window_weak = window.as_weak();
    window.on_pick_and_import_pinyin(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_pinyin(&win, &pinyin_running);
        }
    });

    // 4. 英文与符号辅助词库导入回调（共享同一个互斥锁与防重入标志）
    let is_aux_importing = Arc::new(AtomicBool::new(false));
    let aux_dict_lock = Arc::new(Mutex::new(aux_dict));

    let window_weak = window.as_weak();
    let aux_for_en = aux_dict_lock.clone();
    let aux_running_for_en = is_aux_importing.clone();
    window.on_pick_and_import_english(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_english(&win, &aux_for_en, &aux_running_for_en);
        }
    });

    // 5. 符号词库导入回调
    let window_weak = window.as_weak();
    let aux_for_sym = aux_dict_lock;
    let aux_running_for_sym = is_aux_importing;
    window.on_pick_and_import_symbols(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_symbols(&win, &aux_for_sym, &aux_running_for_sym);
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

/// 处理拼音词库文件选取与异步导入派发
fn handle_pick_and_import_pinyin(win: &SettingsWindow, running_flag: &Arc<AtomicBool>) {
    if running_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_pinyin_import_status(SharedString::from(
            "已有拼音词库导入任务正在执行，请稍候...",
        ));
        return;
    }
    win.set_pinyin_import_status(SharedString::from("正在等待选择文件..."));
    let win_weak = win.as_weak();
    let running_flag = running_flag.clone();

    pick_file_dialog_async(
        "请选择要导入的拼音词库文件",
        "词库文件 (*.txt, *.dict.yaml, *.yaml, *.scel)",
        "*.txt *.dict.yaml *.yaml *.yml *.scel",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = win_weak.upgrade() else {
                    running_flag.store(false, Ordering::Release);
                    return;
                };
                if let Some(path) = selected {
                    let path_str = path.to_string_lossy().to_string();
                    win.set_pinyin_import_path(SharedString::from(&path_str));
                    win.set_pinyin_import_status(SharedString::from(
                        "正在解析并合入词库，请稍候...",
                    ));
                    spawn_pinyin_import_task(win.as_weak(), path_str, running_flag);
                } else {
                    win.set_pinyin_import_status(SharedString::from("已取消文件选择"));
                    running_flag.store(false, Ordering::Release);
                }
            });
        },
    );
}

/// 处理英文辅助词库文件选取与异步导入派发
fn handle_pick_and_import_english(
    win: &SettingsWindow,
    aux_dict: &Arc<Mutex<AuxiliaryDict>>,
    running_flag: &Arc<AtomicBool>,
) {
    if running_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_english_import_status(SharedString::from(
            "已有辅助词库导入任务正在执行，请稍候...",
        ));
        return;
    }
    win.set_english_import_status(SharedString::from("正在等待选择文件..."));
    let win_weak = win.as_weak();
    let aux_dict = aux_dict.clone();
    let running_flag = running_flag.clone();

    pick_file_dialog_async(
        "请选择要导入的英文词库文件",
        "英文词汇与词频文件 (*.txt, *.dict.yaml, *.yaml)",
        "*.txt *.dict.yaml *.yaml *.yml",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = win_weak.upgrade() else {
                    running_flag.store(false, Ordering::Release);
                    return;
                };
                if let Some(path) = selected {
                    let path_str = path.to_string_lossy().to_string();
                    win.set_english_import_path(SharedString::from(&path_str));
                    win.set_english_import_status(SharedString::from(
                        "正在解析英文词库，请稍候...",
                    ));
                    spawn_english_import_task(win.as_weak(), path_str, aux_dict, running_flag);
                } else {
                    win.set_english_import_status(SharedString::from("已取消文件选择"));
                    running_flag.store(false, Ordering::Release);
                }
            });
        },
    );
}

/// 处理特殊符号库文件选取与异步导入派发
fn handle_pick_and_import_symbols(
    win: &SettingsWindow,
    aux_dict: &Arc<Mutex<AuxiliaryDict>>,
    running_flag: &Arc<AtomicBool>,
) {
    if running_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_symbols_import_status(SharedString::from(
            "已有辅助词库导入任务正在执行，请稍候...",
        ));
        return;
    }
    win.set_symbols_import_status(SharedString::from("正在等待选择文件..."));
    let win_weak = win.as_weak();
    let aux_dict = aux_dict.clone();
    let running_flag = running_flag.clone();

    pick_file_dialog_async(
        "请选择要导入的特殊符号库文件",
        "符号定义文件 (*.txt, *.dict.yaml, *.yaml)",
        "*.txt *.dict.yaml *.yaml *.yml",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = win_weak.upgrade() else {
                    running_flag.store(false, Ordering::Release);
                    return;
                };
                if let Some(path) = selected {
                    let path_str = path.to_string_lossy().to_string();
                    win.set_symbols_import_path(SharedString::from(&path_str));
                    win.set_symbols_import_status(SharedString::from(
                        "正在解析特殊符号库，请稍候...",
                    ));
                    spawn_symbols_import_task(win.as_weak(), path_str, aux_dict, running_flag);
                } else {
                    win.set_symbols_import_status(SharedString::from("已取消文件选择"));
                    running_flag.store(false, Ordering::Release);
                }
            });
        },
    );
}
