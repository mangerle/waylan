//! 自定义短语管理与自适应模型维护回调
//!
//! # 设计原理
//! - **实现初衷**：将短语文件落盘、界面列表增删同步以及自造词历史清理逻辑与主设置解耦。
//! - **核心优势**：职责明确，直接维护 `CustomPhraseManager` 与 WAL 辅助文件，隔离 I/O 与列表视图状态。
//! - **代价与局限**：列表变更依赖向 `phrases_model` 共享对象推入/移除数据项。

use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Model, SharedString, VecModel};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_core::config::AppConfig;

use crate::{PhraseItem, SettingsWindow};

/// 注册短语添加、短语删除与历史自造词记忆清理回调
pub(crate) fn register_phrase_callbacks(
    window: &SettingsWindow,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    // 1. 新增短语回调
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

    // 2. 删除短语回调
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

    // 3. 清空自适应历史词模型回调
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
