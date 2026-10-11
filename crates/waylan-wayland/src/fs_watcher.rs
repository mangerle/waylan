//! 文件系统监控与热重载变更高频检测辅助模块
//!
//! 提供基于文件元数据修改时间戳的轻量变更检测逻辑，用于词库与配置的热重载。

use std::path::PathBuf;
use std::time::SystemTime;

/// 检查监控文件列表中是否有任何文件发生修改，并在检测到变更时更新基准时间戳
///
/// # 设计原理
/// - **实现初衷**：在事件循环空闲时轮询外部词库与配置文件 mtime，实现无需重启进程的动态热重载。
/// - **核心优势**：零外部依赖，纯标准库元数据只读比较。
pub fn check_watched_paths_changed(
    watched_paths: &[PathBuf],
    last_watched_mtime: &mut Option<SystemTime>,
) -> bool {
    if watched_paths.is_empty() {
        return false;
    }

    let mut latest_mtime: Option<SystemTime> = None;
    for p in watched_paths {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                latest_mtime = Some(match latest_mtime {
                    Some(prev) => prev.max(mtime),
                    None => mtime,
                });
            }
        }
    }

    match (*last_watched_mtime, latest_mtime) {
        (Some(old), Some(new)) if new > old => {
            *last_watched_mtime = Some(new);
            true
        }
        (None, Some(new)) => {
            *last_watched_mtime = Some(new);
            false
        }
        _ => false,
    }
}
