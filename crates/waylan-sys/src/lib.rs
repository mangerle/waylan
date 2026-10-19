//! 微澜 (WayLan) 平台操作系统底层原语抽象模块
//!
//! # 设计原理
//! - **实现初衷**：将对特定操作系统底层原语（如 Linux inotify 文件变动监听、eventfd 跨线程事件循环唤醒）
//!   从纯粹的领域核心库 `waylan-core` 中彻底剥离解耦。
//! - **核心优势**：
//!   1. 纯粹性：保持 `waylan-core` 纯纯粹粹的领域数据模型与配置定义，零底层平台库依赖；
//!   2. 隔离性：将系统调用错误、文件描述符生命周期与平台原生能力内聚在此模块中；
//!   3. 可移植性：为未来适配多平台操作系统原语提供明确的抽象隔离边界。
//! - **代价与局限**：当前实现聚焦于现代 Linux 内核提供的系统原语。

pub mod assets;
pub mod desktop;
pub mod fs_watcher;
pub mod loop_wakeup;
pub mod process;

pub use assets::{detect_default_assets_path, resolve_icon_path_or_name};
pub use desktop::{
    ensure_gnome_input_source_registered, install_gnome_hide_indicator_extension, is_gnome_desktop,
    prioritize_waylan_in_gsettings_sources, safe_sync_desktop_input_sources,
    set_gnome_extension_enabled, set_waylan_as_exclusive_input_source,
    sync_gnome_hide_indicator_extension,
};
pub use fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
    compute_initial_signature, create_timed_file_event_filter, init_session_inotify_watcher,
    init_session_inotify_watcher_with_filter, DirEventFilter, FileSignature, InotifyWatcher,
    ReadFlags,
};
pub use loop_wakeup::LoopWakeup;
pub use process::{
    run_command_with_timeout, safe_gsettings_get, safe_gsettings_set, safe_gsettings_set_async,
    spawn_command_detached, DEFAULT_COMMAND_TIMEOUT,
};
