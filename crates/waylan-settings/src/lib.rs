//! 微澜 (WayLan) 输入法偏好设置中心图形界面（Slint 实现）
//!
//! # 设计原理
//! - **实现初衷**：基于轻量声明式 Slint 框架，彻底重构为“左侧菜单、右侧内容”的现代化桌面设置中心。
//! - **核心优势**：
//!   1. 物理完全解耦，不依赖重型 OpenGL/Mesa 图形运行时，内存与启动开销极小；
//!   2. 采用 Slint 响应式数据绑定与 ModelRc 列表模型，界面响应流畅、状态无缝同步；
//!   3. 独立进程架构，随关随释放全部系统资源，保障输入法主守护进程常驻纯净。

slint::include_modules!();

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::dict_importer::DictImporter;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::{AppConfig, PageKeys, SwitchKey, ThemeMode};

/// 外部导入文件体积最大允许上限（64 MB），防止异常大文件导致内存暴涨
const MAX_IMPORT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 将 AppConfig 数据应用到 Slint 界面属性
fn sync_config_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    window.set_page_size_val(cfg.page_size as f32);
    window.set_font_size_val(cfg.font_size);
    window.set_switch_key_index(match cfg.switch_key {
        SwitchKey::Shift => 0,
        SwitchKey::Ctrl => 1,
    });
    window.set_page_keys_index(match cfg.page_keys {
        PageKeys::MinusEqual => 0,
        PageKeys::Bracket => 1,
        PageKeys::CommaPeriod => 2,
    });
    window.set_theme_index(match cfg.theme {
        ThemeMode::Light => 0,
        ThemeMode::Dark => 1,
    });
    window.set_slash_to_dunhao(cfg.slash_to_dunhao);
    window.set_protect_digits_punctuation(cfg.protect_digits_punctuation);
    window.set_auto_complete_pairs(cfg.auto_complete_pairs);
    window.set_v_mode(cfg.v_mode);
    window.set_fuzzy_pinyin(cfg.fuzzy_pinyin);
    window.set_traditional_chinese(cfg.traditional_chinese);
    window.set_auto_pangu_spacing(cfg.auto_pangu_spacing);
}

/// 从 Slint 界面属性读取并构造 AppConfig 数据
fn extract_config_from_window(window: &SettingsWindow) -> AppConfig {
    let mut cfg = AppConfig {
        page_size: window.get_page_size_val().round() as usize,
        font_size: window.get_font_size_val().round(),
        switch_key: match window.get_switch_key_index() {
            1 => SwitchKey::Ctrl,
            _ => SwitchKey::Shift,
        },
        page_keys: match window.get_page_keys_index() {
            1 => PageKeys::Bracket,
            2 => PageKeys::CommaPeriod,
            _ => PageKeys::MinusEqual,
        },
        theme: match window.get_theme_index() {
            1 => ThemeMode::Dark,
            _ => ThemeMode::Light,
        },
        slash_to_dunhao: window.get_slash_to_dunhao(),
        protect_digits_punctuation: window.get_protect_digits_punctuation(),
        auto_complete_pairs: window.get_auto_complete_pairs(),
        v_mode: window.get_v_mode(),
        fuzzy_pinyin: window.get_fuzzy_pinyin(),
        traditional_chinese: window.get_traditional_chinese(),
        auto_pangu_spacing: window.get_auto_pangu_spacing(),
        tray_icon_theme_path: None,
        tray_icon_zh: None,
        tray_icon_en: None,
    };
    cfg.sanitize();
    cfg
}

/// 解析并合入外部拼音词库
fn import_pinyin_dict_file(path_str: &str) -> Result<String, String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的词库文件不存在或路径无效，请核对后重试。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("词库文件体积超过 64MB 安全上限，已拒绝加载。".to_string());
    }

    let entries = DictImporter::import_file(path).map_err(|e| format!("解析词库失败: {}", e))?;
    let count = entries.len();
    let user_dict_path = AppConfig::user_dict_path();
    let mut user_model = UserAdaptiveModel::load_or_replay_wal(&user_dict_path).unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_000);
    for entry in &entries {
        user_model.record_commit(&entry.text, &entry.pinyin, now);
    }
    user_model
        .compact_wal(&user_dict_path)
        .map_err(|e| format!("保存用户词库失败: {}", e))?;

    Ok(format!(
        "成功合入 {} 条词目，当前自适应词库总词量: {} 条",
        count,
        user_model.total_entries()
    ))
}

/// 解析并合入外部英文单词库
fn import_english_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的英文词典文件不存在，请核对路径。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("英文词库体积超过 64MB 安全上限。".to_string());
    }

    let content = fs::read_to_string(path).map_err(|e| format!("读取文件失败: {}", e))?;
    let mut existing: HashSet<String> = aux
        .english_words
        .iter()
        .map(|(w, _)| w.to_lowercase())
        .collect();
    let mut added = 0;
    let mut in_yaml_header = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            in_yaml_header = true;
            continue;
        }
        if trimmed == "..." {
            in_yaml_header = false;
            continue;
        }
        if in_yaml_header || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let word = trimmed.split('\t').next().unwrap_or("").trim();
        if !word.is_empty()
            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
            && existing.insert(word.to_lowercase())
        {
            aux.english_words.push((word.to_string(), 50_000));
            added += 1;
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| format!("保存用户辅助词库失败: {}", e))?;

    Ok((
        format!(
            "成功导入 {} 个英文单词，当前单词库总数: {}",
            added,
            aux.english_words.len()
        ),
        aux.english_words.len(),
    ))
}

/// 解析并合入外部符号映射表
fn import_symbols_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的符号映射文件不存在，请核对路径。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("符号词库体积超过 64MB 安全上限。".to_string());
    }

    let content = fs::read_to_string(path).map_err(|e| format!("读取文件失败: {}", e))?;
    let mut existing: HashSet<(String, String)> = aux
        .symbols
        .iter()
        .map(|(k, s, _)| (k.clone(), s.clone()))
        .collect();
    let mut added = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let key = parts[0].trim().to_string();
            let sym = parts[1].trim().to_string();
            let desc = if parts.len() >= 3 {
                parts[2].trim().to_string()
            } else {
                "外部符号".to_string()
            };
            if existing.insert((key.clone(), sym.clone())) {
                aux.symbols.push((key, sym, desc));
                added += 1;
            }
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| format!("保存用户符号词库失败: {}", e))?;

    Ok((
        format!(
            "成功导入 {} 组符号联想项，当前符号总数: {}",
            added,
            aux.symbols.len()
        ),
        aux.symbols.len(),
    ))
}

/// 运行 Slint 偏好设置中心图形界面
///
/// # Errors
/// 当窗口初始化或事件循环运行失败时返回错误。
pub fn run_settings_gui() -> Result<(), Box<dyn Error>> {
    let window = SettingsWindow::new()?;

    // 1. 加载持久化配置并同步到界面
    let config = AppConfig::load_or_create();
    sync_config_to_window(&window, &config);

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

    // 3. 注册保存设置回调
    let window_weak = window.as_weak();
    window.on_save_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            let mut cfg = extract_config_from_window(&win);
            cfg.sanitize();
            match cfg.save_default() {
                Ok(()) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_roundtrip_logic() {
        let mut cfg = AppConfig {
            page_size: 8,
            font_size: 18.0,
            switch_key: SwitchKey::Ctrl,
            page_keys: PageKeys::Bracket,
            theme: ThemeMode::Dark,
            auto_pangu_spacing: true,
            ..Default::default()
        };
        cfg.sanitize();

        assert_eq!(cfg.page_size, 8);
        assert_eq!(cfg.font_size, 18.0);
        assert_eq!(cfg.switch_key, SwitchKey::Ctrl);
        assert_eq!(cfg.page_keys, PageKeys::Bracket);
        assert_eq!(cfg.theme, ThemeMode::Dark);
        assert!(cfg.auto_pangu_spacing);
    }

    #[test]
    fn test_import_non_existent_dict_returns_err() {
        let res = import_pinyin_dict_file("/path/to/never/exist/dict.txt");
        assert!(res.is_err());
    }
}
