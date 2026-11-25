//! 自定义短语管理与自适应模型维护回调
//!
//! # 设计原理
//! - **实现初衷**：将短语文件落盘、界面列表增删同步以及自造词历史清理逻辑与主设置解耦。
//! - **核心优势**：职责明确，直接维护 `CustomPhraseManager` 与 WAL 辅助文件，隔离 I/O 与列表视图状态。
//! - **代价与局限**：列表变更依赖向 `phrases_model` 共享对象推入/移除数据项。

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, Model, SharedString, VecModel};
use waylan_core::config::AppConfig;
use waylan_engine::CustomPhraseManager;

use crate::{PhraseItem, SettingsWindow};

/// 自定义短语回调共享上下文
///
/// # 设计原理
/// - **实现初衷**：增删短语均需同时持有窗口弱引用、列表模型与短语文件路径三项状态，
///   若继续以多参数平铺传入处理函数会立即突破四个入参上限。
/// - **核心优势**：以单一上下文对象承载共享状态，Slint 回调闭包退化为一次具名函数转发。
/// - **代价与局限**：上下文需随每个回调克隆一份。
#[derive(Clone)]
struct PhraseCallbackContext {
    /// 窗口弱引用（升级失败表示窗口已销毁，回调应静默跳过）
    window_weak: slint::Weak<SettingsWindow>,
    /// 短语列表共享模型
    phrases_model: Rc<VecModel<PhraseItem>>,
    /// 短语持久化文件路径
    phrase_path: PathBuf,
}

/// 注册短语添加、短语删除与历史自造词记忆清理回调
pub(crate) fn register_phrase_callbacks(
    window: &SettingsWindow,
    phrases_model: Rc<VecModel<PhraseItem>>,
    phrase_path: PathBuf,
) {
    let context = PhraseCallbackContext {
        window_weak: window.as_weak(),
        phrases_model,
        phrase_path,
    };

    // 1. 新增短语回调
    let add_ctx = context.clone();
    window.on_add_custom_phrase(move |code, text| {
        apply_add_custom_phrase(&add_ctx, code, text);
    });

    // 2. 删除短语回调
    let delete_ctx = context;
    window.on_delete_custom_phrase(move |idx| {
        apply_delete_custom_phrase(&delete_ctx, idx);
    });

    // 3. 清空自适应历史词模型回调
    register_clear_user_model_callback(window);
}

/// 校验并落盘新增自定义短语，成功后回写列表项并清空输入框
///
/// # 设计原理
/// - **实现初衷**：将「入参校验 -> 落盘 -> 回写列表与输入框」这一条完整业务链收敛于单处具名函数，
///   避免大段业务逻辑内联在 Slint 事件闭包中难以维护与测试。
/// - **核心优势**：业务链可独立阅读与单测，回调闭包仅负责一次转发。
/// - **代价与局限**：窗口已销毁时整条链路静默跳过，不向调用方反馈。
fn apply_add_custom_phrase(ctx: &PhraseCallbackContext, code: SharedString, text: SharedString) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    let code_str = code.trim();
    let text_str = text.trim();
    if code_str.is_empty() || text_str.is_empty() {
        win.set_phrase_status(SharedString::from("快捷代码与展开文本不能为空！"));
        return;
    }

    let mut mgr = CustomPhraseManager::load_or_init(&ctx.phrase_path);
    mgr.add_entry(code_str.to_string(), text_str.to_string());
    if let Err(e) = mgr.save_to_file(&ctx.phrase_path) {
        win.set_phrase_status(SharedString::from(format!("保存短语失败: {}", e)));
        return;
    }

    ctx.phrases_model.push(PhraseItem {
        code: SharedString::from(code_str),
        text: SharedString::from(text_str),
    });
    win.set_new_phrase_code(SharedString::from(""));
    win.set_new_phrase_text(SharedString::from(""));
    win.set_phrase_status(SharedString::from("自定义短语添加成功！"));
}

/// 删除指定索引短语：必须先落盘成功再移除列表项，失败时保留原列表以避免界面与磁盘状态脱节
fn apply_delete_custom_phrase(ctx: &PhraseCallbackContext, idx: i32) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    let Ok(index) = usize::try_from(idx) else {
        return;
    };
    let Some(item) = ctx.phrases_model.row_data(index) else {
        return;
    };

    let mut mgr = CustomPhraseManager::load_or_init(&ctx.phrase_path);
    mgr.remove_entry(item.code.as_str(), item.text.as_str());
    match mgr.save_to_file(&ctx.phrase_path) {
        Ok(()) => {
            ctx.phrases_model.remove(index);
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

/// 删除当前 WAL 与旧版遗留历史词文件（文件不存在视为已清理）
///
/// 返回 `Err` 仅在真实磁盘故障时产生，由调用方回写界面错误状态。
fn remove_user_history_files(user_dict: &Path) -> Result<(), String> {
    // 当前 WAL 文件就是 user_dict_path 本身；旧版本可能遗留 user_dict.bin.wal，一并清理
    let legacy_wal = user_dict.with_extension("bin.wal");
    for path in [user_dict, &legacy_wal] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除 {} 失败: {}", path.display(), e)),
        }
    }
    Ok(())
}

/// 注册清空自适应历史词模型回调
fn register_clear_user_model_callback(window: &SettingsWindow) {
    let window_weak = window.as_weak();
    window.on_clear_user_model(move || {
        apply_clear_user_model(&window_weak);
    });
}

/// 清空历史自造词记忆并唤醒守护进程重载配置
///
/// # 设计原理
/// - **实现初衷**：清空动作需同时处理当前 WAL 与旧版本遗留的 `.bin.wal` 文件，
///   并在成功后唤醒守护进程重载，防止其退出时把旧内存数据回写磁盘。
/// - **核心优势**：磁盘清理与状态回写分离，错误路径单点收敛。
/// - **代价与局限**：窗口已销毁时不重试清理（下一次打开设置仍可再次触发）。
fn apply_clear_user_model(window_weak: &slint::Weak<SettingsWindow>) {
    let Some(win) = window_weak.upgrade() else {
        return;
    };

    if let Err(error) = remove_user_history_files(&AppConfig::user_dict_path()) {
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
