//! Wayland 会话文件监控与词库热重载
//!
//! # 设计原理
//! - **实现初衷**：负责配置与词库文件的 inotify 监听、精准事件过滤（防止 WAL 临时追加误触发）以及算法引擎与自定义短语热重载。
//! - **核心优势**：解耦硬编码文件名与算法内部状态，保证外部修改时自动更新、内部归并时不误触发。
//! - **代价与局限**：对部分不支持 inotify 的特殊文件系统降级采用 mtime 轮询机制。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::inotify::ReadFlags;
use waylan_core::fs_watcher::{DirEventFilter, InotifyWatcher};
use waylan_engine::algo::custom_phrase::CustomPhraseManager;
use waylan_engine::AlgoEngine;

pub use waylan_core::fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
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
                false
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

/// 尝试为 Wayland 会话初始化 inotify 文件监控器（注入默认用户词库过滤规则）
pub fn init_session_inotify_watcher(
    watched_paths: &[std::path::PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
) {
    waylan_core::fs_watcher::init_session_inotify_watcher_with_filter(
        watched_paths,
        inotify_watcher,
        session_label,
        Some(default_user_dict_filter()),
    );
}

/// 执行默认的算法引擎词库与自定义短语热重载
pub fn apply_default_engine_reload(engine: &mut AlgoEngine) {
    let phrase_path = CustomPhraseManager::default_path();
    engine.resources.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
    let wal_opt = engine.user_dict_path().map(|p| p.to_path_buf());
    if let Some(ref wal_path) = wal_opt {
        engine.set_user_dict_path(wal_path);
    }
}

/// 执行默认的输入法运行时完整配置与词库热重载（统一收敛至 ImeRuntime::reload_config）
pub fn apply_default_runtime_reload(runtime: &mut waylan_engine::ImeRuntime) {
    let cfg = waylan_core::AppConfig::load_or_create();
    runtime.reload_config(&cfg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_engine::AlgoConfig;

    #[test]
    fn test_apply_default_engine_reload() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        apply_default_engine_reload(&mut engine);
        assert_eq!(engine.raw_input(), "");
    }

    #[test]
    fn test_apply_default_runtime_reload() {
        let mut runtime = waylan_engine::ImeRuntime::with_engine(AlgoEngine::with_sample_data(
            AlgoConfig::default(),
        ));
        apply_default_runtime_reload(&mut runtime);
        assert_eq!(runtime.core.engine.raw_input(), "");
    }

    #[test]
    fn test_inotify_watcher_detection() {
        use std::fs::File;
        use std::io::Write;

        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let test_file = temp_dir.path().join("test_config.toml");
        {
            let mut f = File::create(&test_file).expect("创建测试文件");
            f.write_all(b"key = 1\n").expect("写入初始数据");
        }

        let mut watcher = InotifyWatcher::try_new_with_filter(
            std::slice::from_ref(&test_file),
            Some(default_user_dict_filter()),
        )
        .expect("创建 InotifyWatcher 应当成功");

        assert!(!watcher.drain_events());

        {
            let mut f = File::create(&test_file).expect("打开测试文件覆盖");
            f.write_all(b"key = 2\n").expect("写入更新数据");
            f.flush().expect("刷盘");
        }

        assert!(watcher.drain_events());
        assert!(!watcher.drain_events());
    }

    #[test]
    fn test_inotify_ignores_sibling_wal_append_but_detects_reset_and_replace() {
        use std::fs::File;
        use std::io::Write;

        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let watched_aux = temp_dir.path().join("aux.dict.bin");
        let sibling_user_dict = temp_dir.path().join("user_dict.bin");
        let sibling_tmp = temp_dir.path().join("config.toml.tmp.999");

        let tracker = Arc::new(AtomicU64::new(0));
        let mut watcher = InotifyWatcher::try_new_with_filter(
            std::slice::from_ref(&watched_aux),
            Some(user_dict_filter_with_tracker(Some(tracker.clone()))),
        )
        .expect("创建 InotifyWatcher 应当成功");
        assert!(!watcher.drain_events());

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

        std::fs::remove_file(&sibling_user_dict).expect("删除 user_dict.bin");
        assert!(
            watcher.drain_events(),
            "外部删除清空 user_dict.bin 应正常触发热重载"
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        tracker.store(now, Ordering::Release);
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

        let now2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        tracker.store(now2, Ordering::Release);
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
