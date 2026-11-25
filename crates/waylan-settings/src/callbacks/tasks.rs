//! 词库与配置异步后台导入任务派发
//!
//! # 设计原理
//! - **实现初衷**：将大体积拼音词库、常用英文词汇与特殊符号库的磁盘解析、反序列化与合并迁移至后台线程池。
//! - **核心优势**：避免解析数百 MB 外部文本阻塞 Slint 主事件循环；配合 RAII Guard 防并发重入。
//! - **代价与局限**：后台任务需跨线程向 Slint UI 线程派发 `invoke_from_event_loop` 进行界面状态刷新。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use slint::SharedString;
use waylan_core::config::AppConfig;
use waylan_engine::{AuxiliaryDict, LexiconSummaryService};

use crate::SettingsWindow;
use crate::importer::{
    ImportError, import_english_dict_file, import_pinyin_dict_file, import_symbols_dict_file,
};

/// 异步任务防重入与生命周期安全守护对象 (RAII)
///
/// 确保异步任务在退出（无论正常完成还是异常恐慌）时，均能原子化清除正在执行标志，杜绝并发重入死锁。
pub(crate) struct TaskRunningGuard(pub(crate) Arc<AtomicBool>);

impl Drop for TaskRunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 派生并注册一条后台导入线程，线程创建失败时立即回滚防重入标志并记录告警
///
/// # 设计原理
/// - **实现初衷**：三类词库导入的线程派生样板（命名、spawn、失败降级）完全一致，
///   在每处重复书写会让失败路径出现行为漂移。
/// - **核心优势**：单一派生入口统一失败降级语义，调用点仅需提供线程名与线程体。
/// - **代价与局限**：`worker` 必须是具名函数指针，无法携带捕获状态（捕获项由调用点预先搬入闭包外的 move 绑定）。
fn spawn_import_worker<F>(
    thread_name: &str,
    running_flag: &Arc<AtomicBool>,
    failure_hint: &str,
    worker: F,
) where
    F: FnOnce() + Send + 'static,
{
    match std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(worker)
    {
        Ok(_) => {}
        Err(e) => {
            running_flag.store(false, Ordering::Release);
            log::warn!("[偏好设置 警告] {}：{}", failure_hint, e);
        }
    }
}

/// 在主事件循环回填拼音词库导入结果与自适应模型统计
fn report_pinyin_import(
    win_weak: &slint::Weak<SettingsWindow>,
    result: Result<String, ImportError>,
    adaptive_count: usize,
) {
    let Some(w) = win_weak.upgrade() else {
        return;
    };
    match result {
        Ok(msg) => {
            w.set_pinyin_import_status(SharedString::from(msg));
            w.set_user_model_info(SharedString::from(format!(
                "自适应个性化历史词模型: {} 条记忆",
                adaptive_count
            )));
        }
        Err(err) => {
            w.set_pinyin_import_status(SharedString::from(format!("导入失败: {}", err)));
        }
    }
}

/// 带词数统计的辅助词库导入结果回填参数
///
/// # 设计原理
/// - **实现初衷**：英文与符号词库的导入结果结构完全一致（`Result<(String, usize), ImportError>`），
///   仅界面属性写入器与统计文案模板不同，用函数指针参数化可消除两处逐字重复的回填分支。
/// - **核心优势**：失败降级与窗口升级判定收敛为单一实现，杜绝两处状态回填语义漂移。
/// - **代价与局限**：以函数指针传参，可读性略低于直白重复。
struct CountedImportReport<'a> {
    /// 界面窗口弱引用（升级失败表示窗口已销毁，回填应静默跳过）
    win_weak: &'a slint::Weak<SettingsWindow>,
    /// 后台导入结果（消息文案与词条总数）
    result: Result<(String, usize), ImportError>,
    /// 导入状态属性写入器
    status_setter: fn(&SettingsWindow, SharedString),
    /// 词库规模属性写入器
    info_setter: fn(&SettingsWindow, SharedString),
    /// 词库规模统计文案构造函数（入参为词条总数）
    info_builder: fn(usize) -> String,
}

/// 在主事件循环回填带词数统计的辅助词库导入结果
fn report_counted_import(report: CountedImportReport<'_>) {
    let CountedImportReport {
        win_weak,
        result,
        status_setter,
        info_setter,
        info_builder,
    } = report;
    let Some(w) = win_weak.upgrade() else {
        return;
    };
    match result {
        Ok((msg, total)) => {
            status_setter(&w, SharedString::from(msg));
            info_setter(&w, SharedString::from(info_builder(total)));
        }
        Err(err) => {
            status_setter(&w, SharedString::from(format!("导入失败: {}", err)));
        }
    }
}

/// 拼音词库导入后台线程体：解析磁盘词库并在完成后回投 UI 线程刷新状态
fn run_pinyin_import(
    path_str: String,
    guard_flag: Arc<AtomicBool>,
    win_weak: slint::Weak<SettingsWindow>,
) {
    let _guard = TaskRunningGuard(guard_flag);
    let result = import_pinyin_dict_file(&path_str);
    if result.is_ok() {
        AppConfig::touch_config_for_reload();
    }
    let count = LexiconSummaryService::query_user_adaptive_entry_count();
    let _ = slint::invoke_from_event_loop(move || {
        report_pinyin_import(&win_weak, result, count);
    });
}

/// 英文词库导入后台线程体：锁外执行大文件解析与落盘，成功后再短暂持锁提交内存镜像
fn run_english_import(
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    guard_flag: Arc<AtomicBool>,
    win_weak: slint::Weak<SettingsWindow>,
) {
    let _guard = TaskRunningGuard(guard_flag);
    let mut working_aux = aux_lock.lock().clone();
    let result = import_english_dict_file(&path_str, &mut working_aux);
    if result.is_ok() {
        *aux_lock.lock() = working_aux;
        AppConfig::touch_config_for_reload();
    }
    let _ = slint::invoke_from_event_loop(move || {
        report_counted_import(CountedImportReport {
            win_weak: &win_weak,
            result,
            status_setter: SettingsWindow::set_english_import_status,
            info_setter: SettingsWindow::set_english_dict_info,
            info_builder: |total| format!("已就绪常用英文单词: {total} 词"),
        });
    });
}

/// 特殊符号库导入后台线程体：锁外执行文件解析与落盘，成功后再短暂持锁提交内存镜像
fn run_symbols_import(
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    guard_flag: Arc<AtomicBool>,
    win_weak: slint::Weak<SettingsWindow>,
) {
    let _guard = TaskRunningGuard(guard_flag);
    let mut working_aux = aux_lock.lock().clone();
    let result = import_symbols_dict_file(&path_str, &mut working_aux);
    if result.is_ok() {
        *aux_lock.lock() = working_aux;
        AppConfig::touch_config_for_reload();
    }
    let _ = slint::invoke_from_event_loop(move || {
        report_counted_import(CountedImportReport {
            win_weak: &win_weak,
            result,
            status_setter: SettingsWindow::set_symbols_import_status,
            info_setter: SettingsWindow::set_symbols_dict_info,
            info_builder: |total| format!("已收录语义符号与表情: {total} 项"),
        });
    });
}

/// 异步执行拼音词库导入并在主界面刷新状态
pub(crate) fn spawn_pinyin_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    spawn_import_worker(
        "pinyin-importer",
        &running_flag,
        "派发拼音导入线程失败",
        move || run_pinyin_import(path_str, guard_flag, win_weak),
    );
}

/// 异步执行英文词库导入并在主界面刷新状态
pub(crate) fn spawn_english_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    spawn_import_worker(
        "english-dict-importer",
        &running_flag,
        "派发英文词库导入线程失败",
        move || run_english_import(path_str, aux_lock, guard_flag, win_weak),
    );
}

/// 异步执行特殊符号库导入并在主界面刷新状态
pub(crate) fn spawn_symbols_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    spawn_import_worker(
        "symbols-dict-importer",
        &running_flag,
        "派发符号词库导入线程失败",
        move || run_symbols_import(path_str, aux_lock, guard_flag, win_weak),
    );
}
