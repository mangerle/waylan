//! 基于 Linux inotify 的通用文件与目录事件监控工具
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心词库、自定义短语及运行时配置文件提供高效的内核级事件驱动文件变动监听，
//!   消除主动轮询造成的 CPU 开销。
//! - **核心优势**：基于内核级 inotify 事件唤醒，同时监控目标文件及其父目录（防御文本编辑器原子重命名替换），
//!   支持通过通用过滤器精确短路无关临时文件。
//! - **代价与局限**：受 Linux inotify 实例配额与描述符限制。

pub mod inotify;
pub mod signature;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{info, warn};

pub use inotify::{create_timed_file_event_filter, DirEventFilter, InotifyWatcher, ReadFlags};
pub use signature::{check_watched_paths_changed, compute_initial_signature, FileSignature};

/// 尝试初始化通用的 inotify 文件监控器（默认无特殊排除过滤）
pub fn init_session_inotify_watcher(
    watched_paths: &[PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
) {
    init_session_inotify_watcher_with_filter(watched_paths, inotify_watcher, session_label, None);
}

/// 尝试初始化带有自定义目录事件过滤器的 inotify 文件监控器
pub fn init_session_inotify_watcher_with_filter(
    watched_paths: &[PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
    filter: Option<DirEventFilter>,
) {
    if !watched_paths.is_empty() && inotify_watcher.is_none() {
        match InotifyWatcher::try_new_with_filter(watched_paths, filter) {
            Ok(watcher) => {
                info!(
                    "[微澜 {}] 成功初始化 inotify 文件监控，关闭高频后台 stat 轮询。",
                    session_label
                );
                *inotify_watcher = Some(watcher);
            }
            Err(e) => {
                warn!(
                    "[微澜 {}] 初始化 inotify 失败 ({})，回退至定时轮询模式。",
                    session_label, e
                );
            }
        }
    }
}

/// 检查热重载请求或被监控文件是否发生变化
///
/// 优先排空 inotify 队列事件，回退时比对被监控文件状态签名记录。
pub fn check_reload_requested_or_files_changed(
    reload_requested: Option<&AtomicBool>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    watched_paths: &[PathBuf],
    last_signature: &mut Option<FileSignature>,
) -> bool {
    let mut should_reload = reload_requested
        .map(|flag| flag.swap(false, Ordering::AcqRel))
        .unwrap_or(false);

    let changed = if let Some(ref mut watcher) = inotify_watcher {
        watcher.drain_events()
    } else {
        check_watched_paths_changed(watched_paths, last_signature)
    };
    if changed {
        should_reload = true;
    }
    should_reload
}
