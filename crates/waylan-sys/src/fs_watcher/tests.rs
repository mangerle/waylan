//! 文件监控与签名检测单元测试

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::check_reload_requested_or_files_changed;
use super::inotify::{create_timed_file_event_filter, InotifyWatcher};
use super::signature::check_watched_paths_changed;

#[test]
fn test_check_reload_requested() {
    let flag = AtomicBool::new(true);
    let mut watcher = None;
    let paths = Vec::new();
    let mut sig = None;

    let needed =
        check_reload_requested_or_files_changed(Some(&flag), &mut watcher, &paths, &mut sig);
    assert!(needed);
    assert!(!flag.load(Ordering::Relaxed));
}

#[test]
fn test_inotify_watcher_detection() {
    let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
    let test_file = temp_dir.path().join("test_config.toml");
    {
        let mut f = File::create(&test_file).expect("创建测试文件");
        f.write_all(b"key = 1\n").expect("写入初始数据");
    }

    let mut watcher = InotifyWatcher::try_new(std::slice::from_ref(&test_file))
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
fn test_inotify_timed_file_filter_suppression_and_replacement() {
    let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
    let watched_main = temp_dir.path().join("main_data.bin");
    let sibling_file = temp_dir.path().join("ignored_wal.bin");
    let sibling_tmp = temp_dir.path().join("config.toml.tmp.999");

    let tracker = Arc::new(AtomicU64::new(0));
    let filter = create_timed_file_event_filter("ignored_wal.bin", Some(tracker.clone()), 2000);
    let mut watcher =
        InotifyWatcher::try_new_with_filter(std::slice::from_ref(&watched_main), Some(filter))
            .expect("创建 InotifyWatcher 应当成功");
    assert!(!watcher.drain_events());

    let mut wal_file = File::create(&sibling_file).expect("创建 ignored_wal.bin");
    wal_file.write_all(b"MAGIC_HEADER").expect("写魔数");
    wal_file.flush().expect("刷盘");
    assert!(
        !watcher.drain_events(),
        "首次创建并追加写入被过滤的文件绝不能误触发热重载"
    );

    wal_file.write_all(b"record_data").expect("追加数据");
    wal_file.flush().expect("刷盘");
    drop(wal_file);
    assert!(
        !watcher.drain_events(),
        "常规写入或关闭句柄不应误触发热重载"
    );

    {
        let mut tmp_f = File::create(&sibling_tmp).expect("创建临时文件");
        tmp_f.write_all(b"temp").expect("写入临时数据");
    }
    std::fs::remove_file(&sibling_tmp).expect("清理临时文件");
    assert!(
        !watcher.drain_events(),
        "同目录下无关临时文件的创建、修改与删除不应误触发热重载"
    );

    let wal_tmp = temp_dir.path().join("wal.tmp");
    File::create(&wal_tmp)
        .expect("创建归并临时文件")
        .write_all(b"COMPACTED_DATA")
        .expect("写入归并内容");
    std::fs::rename(&wal_tmp, &sibling_file).expect("原子重命名替换 ignored_wal.bin");
    assert!(
        watcher.drain_events(),
        "外部通过 rename 原子替换 ignored_wal.bin 应正常触发热重载"
    );

    std::fs::remove_file(&sibling_file).expect("删除 ignored_wal.bin");
    assert!(
        watcher.drain_events(),
        "外部删除清空 ignored_wal.bin 应正常触发热重载"
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    tracker.store(now, Ordering::Release);
    let internal_tmp = temp_dir.path().join("internal.tmp");
    File::create(&internal_tmp)
        .expect("创建内部归并临时文件")
        .write_all(b"INTERNAL_COMPACT")
        .expect("写入内部归并数据");
    std::fs::rename(&internal_tmp, &sibling_file).expect("原子重命名替换 ignored_wal.bin");
    assert!(
        !watcher.drain_events(),
        "内部原子重命名在抑制时间窗口内绝不能误触发热重载"
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
