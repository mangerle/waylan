//! 文件系统监控与热重载变更高频检测辅助模块
//!
//! 提供基于 Linux `inotify` 的事件驱动文件变更检测，并在不支持时回退为基于元数据时间戳的轻量轮询检测。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::inotify::ReadFlags;
pub use waylan_core::fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
    init_session_inotify_watcher_with_filter, DirEventFilter, InotifyWatcher,
};

/// 构造附带专用落盘时间戳跟踪器的用户词库过滤回调
pub fn user_dict_filter_with_tracker(tracker: Option<Arc<AtomicU64>>) -> DirEventFilter {
    Arc::new(move |_dir, file_name, flags| {
        if file_name == "user_dict.bin" {
            let is_recent = if let Some(ref t) = tracker {
                let last = t.load(Ordering::Acquire);
                if last != 0 {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    now.saturating_sub(last) <= 2000
                } else {
                    false
                }
            } else {
                waylan_algo::user_adaptive::is_recent_internal_compact(2000)
            };

            if is_recent {
                return Some(false);
            }
            if flags.intersects(ReadFlags::DELETE | ReadFlags::MOVED_TO) {
                return Some(true);
            }
        }
        None
    })
}

/// 构造默认的用户词库过滤回调，解耦硬编码文件名与算法状态依赖
pub fn default_user_dict_filter() -> DirEventFilter {
    user_dict_filter_with_tracker(None)
}

/// 为 Wayland 会话创建带有默认用户词库防抖过滤器的 Inotify 监控实例
pub fn create_wayland_inotify_watcher(paths: &[PathBuf]) -> std::io::Result<InotifyWatcher> {
    InotifyWatcher::try_new_with_filter(paths, Some(default_user_dict_filter()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_inotify_watcher_detection() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let test_file = temp_dir.path().join("test_config.toml");
        {
            let mut f = File::create(&test_file).expect("创建测试文件");
            f.write_all(b"key = 1\n").expect("写入初始数据");
        }

        let mut watcher = create_wayland_inotify_watcher(std::slice::from_ref(&test_file))
            .expect("创建 InotifyWatcher 应当成功");

        // 初始状态下无挂起事件
        assert!(!watcher.drain_events());

        // 修改文件内容并写入
        {
            let mut f = File::create(&test_file).expect("打开测试文件覆盖");
            f.write_all(b"key = 2\n").expect("写入更新数据");
            f.flush().expect("刷盘");
        }

        // inotify 应捕获到修改事件
        assert!(watcher.drain_events());
        // 再次清空应返回 false
        assert!(!watcher.drain_events());
    }

    #[test]
    fn test_inotify_ignores_sibling_wal_append_but_detects_reset_and_replace() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let watched_aux = temp_dir.path().join("aux.dict.bin");
        let sibling_user_dict = temp_dir.path().join("user_dict.bin");
        let sibling_tmp = temp_dir.path().join("config.toml.tmp.999");

        let mut watcher = create_wayland_inotify_watcher(std::slice::from_ref(&watched_aux))
            .expect("创建 InotifyWatcher 应当成功");
        assert!(!watcher.drain_events());

        // 1. 模拟同目录下选词上屏首次创建并持续追加写入 user_dict.bin，以及写入无关临时文件
        let mut wal_file = File::create(&sibling_user_dict).expect("创建 user_dict.bin");
        wal_file.write_all(b"WAYLAN_USERWAL02").expect("写魔数");
        wal_file.flush().expect("刷盘");
        assert!(
            !watcher.drain_events(),
            "选词首次创建并追加写入 user_dict.bin 绝不能误触发配置热重载"
        );

        wal_file.write_all(b"record_frame_1").expect("追加词频记录");
        wal_file.flush().expect("刷盘");
        drop(wal_file);
        assert!(
            !watcher.drain_events(),
            "user_dict.bin 常规写入或关闭句柄不应误触发配置热重载"
        );

        {
            let mut tmp_f = File::create(&sibling_tmp).expect("创建临时文件");
            tmp_f.write_all(b"temp").expect("写入临时数据");
        }
        std::fs::remove_file(&sibling_tmp).expect("清理临时文件");
        assert!(
            !watcher.drain_events(),
            "同目录下无关临时文件的创建、修改与删除不应误触发配置热重载"
        );

        // 2. 模拟外部设置中心通过临时文件原子 rename 替换 user_dict.bin (MOVED_TO)
        let wal_tmp = temp_dir.path().join("user_dict.wal.tmp");
        File::create(&wal_tmp)
            .expect("创建归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_COMPACTED")
            .expect("写入归并内容");
        std::fs::rename(&wal_tmp, &sibling_user_dict).expect("原子重命名替换 user_dict.bin");
        assert!(
            watcher.drain_events(),
            "外部通过 rename 原子替换 user_dict.bin 应正常触发热重载"
        );

        // 3. 模拟外部设置中心清空删除 user_dict.bin (DELETE)
        std::fs::remove_file(&sibling_user_dict).expect("删除 user_dict.bin");
        assert!(
            watcher.drain_events(),
            "外部删除清空 user_dict.bin 应正常触发热重载"
        );

        // 4. 模拟进程内部调用 mark_internal_compact 归并落盘时，原子重命名不应误触发热重载
        waylan_algo::user_adaptive::mark_internal_compact();
        let internal_tmp = temp_dir.path().join("user_dict.internal.tmp");
        File::create(&internal_tmp)
            .expect("创建内部归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_INTERNAL")
            .expect("写入内部归并数据");
        std::fs::rename(&internal_tmp, &sibling_user_dict).expect("原子重命名替换 user_dict.bin");
        assert!(
            !watcher.drain_events(),
            "内部 compact_wal 原子重命名 user_dict.bin 绝不能误触发热重载"
        );

        // 5. 模拟生产环境中目标 user_dict.bin 已存在时，内部 compact_wal 再次原子覆盖替换（可能产生 DELETE 事件）
        waylan_algo::user_adaptive::mark_internal_compact();
        let internal_tmp2 = temp_dir.path().join("user_dict.internal2.tmp");
        File::create(&internal_tmp2)
            .expect("创建第二次内部归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_INTERNAL_2")
            .expect("写入第二次内部归并数据");
        std::fs::rename(&internal_tmp2, &sibling_user_dict)
            .expect("原子覆盖替换已存在的 user_dict.bin");
        assert!(
            !watcher.drain_events(),
            "内部 compact_wal 覆盖替换已存在的 user_dict.bin 时绝不能误触发热重载"
        );
    }

    #[test]
    fn test_check_watched_paths_changed_multiple_files_and_deletion() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let file_a = temp_dir.path().join("a.toml");
        let file_b = temp_dir.path().join("b.toml");
        std::fs::write(&file_a, "a = 1").expect("写文件 a");
        std::fs::write(&file_b, "b = 1").expect("写文件 b");

        let paths = vec![file_a.clone(), file_b.clone()];
        let mut last_mtime = None;

        assert!(!check_watched_paths_changed(&paths, &mut last_mtime));
        assert!(last_mtime.is_some());

        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&file_b, "b = 2").expect("修改文件 b");
        assert!(check_watched_paths_changed(&paths, &mut last_mtime));

        assert!(!check_watched_paths_changed(&paths, &mut last_mtime));

        std::fs::remove_file(&file_a).expect("删除文件 a");
        std::fs::remove_file(&file_b).expect("删除文件 b");
        assert!(check_watched_paths_changed(&paths, &mut last_mtime));
        assert!(last_mtime.is_none());
    }
}
