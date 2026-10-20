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
use waylan_core::config::AppConfig;
use waylan_engine::CustomPhraseManager;

use crate::{PhraseItem, SettingsWindow};

/// 注册短语添加、短语删除与历史自造词记忆清理回调
pub(crate) fn register_phrase_callbacks(
    window: &SettingsWindow,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    // 1. 新增短语回调
    register_add_phrase_callback(window, phrases_model.clone(), phrase_path.clone());

    // 2. 删除短语回调
    register_delete_phrase_callback(window, phrases_model, phrase_path);

    // 3. 清空自适应历史词模型回调
    register_clear_user_model_callback(window);
}

/// 注册新增自定义短语回调
///
/// 设计意图：将「入参校验 -> 落盘 -> 回写列表与输入框」这一条完整业务链收敛于单处，
/// 避免主注册函数在多个回调之间交错共享状态，降低后续维护时的遗漏风险。
fn register_add_phrase_callback(
    window: &SettingsWindow,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    let window_weak = window.as_weak();
    window.on_add_custom_phrase(move |code, text| {
        if let Some(win) = window_weak.upgrade() {
            let code_str = code.trim();
            let text_str = text.trim();
            if code_str.is_empty() || text_str.is_empty() {
                win.set_phrase_status(SharedString::from("快捷代码与展开文本不能为空！"));
                return;
            }
            let mut mgr = CustomPhraseManager::load_or_init(&phrase_path);
            mgr.add_entry(code_str.to_string(), text_str.to_string());
            if let Err(e) = mgr.save_to_file(&phrase_path) {
                win.set_phrase_status(SharedString::from(format!("保存短语失败: {}", e)));
                return;
            }
            phrases_model.push(PhraseItem {
                code: SharedString::from(code_str),
                text: SharedString::from(text_str),
            });
            win.set_new_phrase_code(SharedString::from(""));
            win.set_new_phrase_text(SharedString::from(""));
            win.set_phrase_status(SharedString::from("自定义短语添加成功！"));
        }
    });
}

/// 注册删除自定义短语回调
///
/// 设计意图：删除动作必须先落盘成功再移除列表项，失败时保留原列表以避免界面与磁盘状态脱节；
/// 该一致性约束与实现细节一并封装在本回调内。
fn register_delete_phrase_callback(
    window: &SettingsWindow,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    let window_weak = window.as_weak();
    window.on_delete_custom_phrase(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            let index = idx as usize;
            if let Some(item) = phrases_model.row_data(index) {
                let mut mgr = CustomPhraseManager::load_or_init(&phrase_path);
                mgr.remove_entry(item.code.as_str(), item.text.as_str());
                match mgr.save_to_file(&phrase_path) {
                    Ok(()) => {
                        phrases_model.remove(index);
                        win.set_phrase_status(SharedString::from("已成功移除指定短语。"));
                    }
                    Err(e) => {
                        win.set_phrase_status(SharedString::from(format!(
                            "删除短语失败，词库未被修改: {}",
                            e
                        )));
                    }
                }
            }
        }
    });
}

/// 注册清空自适应历史词模型回调
///
/// 设计意图：清空动作需同时处理当前 WAL 文件与旧版本遗留的 `.bin.wal` 文件，
/// 并在成功后唤醒守护进程重载配置，防止守护进程退出时把旧内存数据回写磁盘。
fn register_clear_user_model_callback(window: &SettingsWindow) {
    let window_weak = window.as_weak();
    window.on_clear_user_model(move || {
        if let Some(win) = window_weak.upgrade() {
            let user_dict = AppConfig::user_dict_path();
            // 当前 WAL 文件就是 user_dict_path 本身；旧版本可能遗留 user_dict.bin.wal，一并清理
            let legacy_wal = user_dict.with_extension("bin.wal");
            let mut failed: Option<String> = None;
            for path in [&user_dict, &legacy_wal] {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        failed = Some(format!("删除 {} 失败: {}", path.display(), e));
                        break;
                    }
                }
            }

            if let Some(error) = failed {
                win.set_global_status_is_error(true);
                win.set_global_status(SharedString::from(error));
                return;
            }

            // 唤醒守护进程重载配置：守护进程检测到词库文件消失后会重置内存模型并重新绑定 WAL，
            // 避免其在退出时把旧内存数据再次压缩写回磁盘。
            AppConfig::touch_config_for_reload();
            win.set_user_model_info(SharedString::from(
                "自适应个性化历史词模型: 0 条记忆 (已清空)",
            ));
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from("用户历史自造词记忆已彻底清空。"));
        }
    });
}
