//! 向前兼容重导出：核心文件监控器实现已统一下沉至 `waylan_core::fs_watcher`。

pub use waylan_core::fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
    init_session_inotify_watcher_with_filter, DirEventFilter, InotifyWatcher,
};

pub use crate::session_common::{
    default_user_dict_filter, init_session_inotify_watcher, user_dict_filter_with_tracker,
};
