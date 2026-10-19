//! 词库与配置异步后台导入任务派发
//!
//! # 设计原理
//! - **实现初衷**：将大体积拼音词库、常用英文词汇与特殊符号库的磁盘解析、反序列化与合并迁移至后台线程池。
//! - **核心优势**：避免解析数百 MB 外部文本阻塞 Slint 主事件循环；配合 RAII Guard 防并发重入。
//! - **代价与局限**：后台任务需跨线程向 Slint UI 线程派发 `invoke_from_event_loop` 进行界面状态刷新。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::SharedString;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::AppConfig;

use crate::importer::{
    import_english_dict_file, import_pinyin_dict_file, import_symbols_dict_file,
};
use crate::SettingsWindow;

/// 异步任务防重入与生命周期安全守护对象 (RAII)
///
/// 确保异步任务在退出（无论正常完成还是异常恐慌）时，均能原子化清除正在执行标志，杜绝并发重入死锁。
pub(crate) struct TaskRunningGuard(pub(crate) Arc<AtomicBool>);

impl Drop for TaskRunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 异步执行拼音词库导入并在主界面刷新状态
pub(crate) fn spawn_pinyin_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("pinyin-importer".to_string())
        .spawn(move || {
            let _guard = TaskRunningGuard(guard_flag);
            let result = import_pinyin_dict_file(&path_str);
            if result.is_ok() {
                AppConfig::touch_config_for_reload();
            }
            let count = UserAdaptiveModel::load_or_replay_wal(AppConfig::user_dict_path())
                .map(|m| m.total_entries())
                .unwrap_or(0);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
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
        })
    {
        running_flag.store(false, Ordering::Release);
        log::warn!("[偏好设置 警告] 派发拼音导入线程失败: {}", e);
    }
}

/// 异步执行英文词库导入并在主界面刷新状态
pub(crate) fn spawn_english_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("english-dict-importer".to_string())
        .spawn(move || {
            let _guard = TaskRunningGuard(guard_flag);
            let result = {
                let mut aux = aux_lock.lock();
                import_english_dict_file(&path_str, &mut aux)
            };
            if result.is_ok() {
                AppConfig::touch_config_for_reload();
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
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
        })
    {
        running_flag.store(false, Ordering::Release);
        log::warn!("[偏好设置 警告] 派发英文词库导入线程失败: {}", e);
    }
}

/// 异步执行特殊符号库导入并在主界面刷新状态
pub(crate) fn spawn_symbols_import_task(
    win_weak: slint::Weak<SettingsWindow>,
    path_str: String,
    aux_lock: Arc<Mutex<AuxiliaryDict>>,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("symbols-dict-importer".to_string())
        .spawn(move || {
            let _guard = TaskRunningGuard(guard_flag);
            let result = {
                let mut aux = aux_lock.lock();
                import_symbols_dict_file(&path_str, &mut aux)
            };
            if result.is_ok() {
                AppConfig::touch_config_for_reload();
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
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
        })
    {
        running_flag.store(false, Ordering::Release);
        log::warn!("[偏好设置 警告] 派发符号词库导入线程失败: {}", e);
    }
}
