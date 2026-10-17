//! V1 与 V2 输入法会话通用状态管理与辅助同步逻辑
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 (`zwp_input_method_v1`) 与 V2 (`zwp_input_method_v2`)
//!   会话实现中完全重复的外部状态感知、inotify 文件变更监听、修饰键跟踪与输入模式同步样板代码。
//! - **核心优势**：将热重载检测、原子状态同步与修饰键掩码判定收敛为单一真相源（Single Source of Truth），
//!   防止后续协议演进时维护失步。
//! - **代价与局限**：调用方需传递对应的字段引用，相比完全内嵌略微增加了函数调用跳转，但消除了数十处重复代码。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::inotify::ReadFlags;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::AlgoEngine;
use waylan_core::fs_watcher::{DirEventFilter, InotifyWatcher};
use waylan_core::input::{update_modifier_masks, ModifierState, MOD_CAPS};
use waylan_core::{CandidateBoxState, InputMode};
use waylan_voice::VoiceInteractionManager;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

use crate::surface::CandidateSurfaceManager;

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
    engine.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
    let wal_opt = engine.user_dict_path().map(|p| p.to_path_buf());
    if let Some(ref wal_path) = wal_opt {
        engine.set_user_dict_path(wal_path);
    }
}

pub use waylan_core::input::sync_mode_from_shared_flag;

/// 判断热重载原子标志是否被触发（用于按键热路径轻量检测，避免高频系统调用）
#[inline]
pub fn is_reload_requested(reload_requested: Option<&AtomicBool>) -> bool {
    reload_requested
        .map(|flag| flag.load(Ordering::Acquire))
        .unwrap_or(false)
}

/// 根据按键事件更新修饰键位掩码
#[inline]
pub fn update_session_modifiers(
    mods_depressed: &mut u32,
    phys_mods: &mut u8,
    keycode: u32,
    is_press: bool,
) {
    update_modifier_masks(mods_depressed, phys_mods, keycode, is_press);
}

/// 判断给定的修饰键状态是否为 Shift 活跃
#[inline]
pub fn is_shift_active_state(mods_depressed: u32, phys_mods: u8) -> bool {
    ModifierState {
        depressed: mods_depressed,
        phys_mods,
        locked: 0,
    }
    .is_shift_active()
}

/// 判断给定的修饰键状态是否包含 Ctrl / Alt / Super
#[inline]
pub fn has_ctrl_or_alt_state(mods_depressed: u32, phys_mods: u8) -> bool {
    ModifierState {
        depressed: mods_depressed,
        phys_mods,
        locked: 0,
    }
    .has_ctrl_or_alt()
}

/// 判断给定的锁定掩码是否处于 CapsLock 大写锁定状态
#[inline]
pub fn is_caps_locked_state(mods_locked: u32) -> bool {
    (mods_locked & MOD_CAPS) != 0
}

/// 提交候选框图层所需的 Wayland 目标上下文参数对象
pub struct CandidateSurfaceCommitParams<'a, S> {
    /// 共享内存协议句柄
    pub shm: Option<&'a WlShm>,
    /// 候选框目标表面句柄
    pub popup_wl_surface: Option<&'a WlSurface>,
    /// 事件队列句柄
    pub qhandle: Option<&'a QueueHandle<S>>,
    /// 活跃像素缓冲区借用
    pub active_buffer: &'a mut Option<WlBuffer>,
}

/// 提交候选框状态至 Wayland 表面并更新活跃缓冲区（采用参数对象收敛入参）
pub fn commit_candidate_surface<S>(
    surface_manager: &mut CandidateSurfaceManager,
    box_state: &CandidateBoxState,
    params: CandidateSurfaceCommitParams<'_, S>,
) where
    S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    surface_manager.commit_to_wayland(
        box_state,
        params.shm,
        params.popup_wl_surface,
        params.qhandle,
        params.active_buffer,
    );
}

/// 统一构建候选框渲染状态的核心数据（收敛 V1 和 V2 对 Candidate 与 ExpandedCardState 的转换）
pub fn build_common_candidate_box_state(
    engine: &AlgoEngine,
    mode: InputMode,
    is_active: bool,
    is_grid_expanded: bool,
    grid_selected_index: usize,
    inline_preedit: bool,
    voice_manager: Option<&VoiceInteractionManager>,
) -> CandidateBoxState {
    use waylan_core::{Candidate, ExpandedCardState};

    // 若当前处于语音输入状态，优先呈现语音识别流式预览与动态波形（与 IBus 视觉契约 100% 对齐）
    if let Some(vm) = voice_manager {
        if vm.is_active() {
            let candidates = if let Some(ref voice_text) = vm.display_text {
                vec![Candidate::new(
                    voice_text.clone(),
                    vm.waveform.clone().unwrap_or_default(),
                    1000,
                )]
            } else {
                Vec::new()
            };
            let preedit = vm.partial_text.clone().unwrap_or_default();
            return CandidateBoxState {
                visible: is_active,
                preedit,
                candidates,
                selected_index: 0,
                expanded_card: ExpandedCardState::Collapsed,
                mode_badge: "语".to_string(),
                is_grid_expanded: false,
                grid_candidates: Vec::new(),
            };
        }
    }

    let core_candidates: Vec<Candidate> = engine.candidates().iter().map(Candidate::from).collect();

    let grid_candidates: Vec<Candidate> = if is_grid_expanded {
        engine
            .all_candidates()
            .iter()
            .take(30)
            .map(Candidate::from)
            .collect()
    } else {
        Vec::new()
    };

    let expanded_card = if engine.is_expanded() {
        ExpandedCardState::Expanded {
            selected_index: engine.expanded_selected_index(),
        }
    } else {
        ExpandedCardState::Collapsed
    };

    let is_dynamic_v_mode = engine.raw_input().starts_with('v');
    let (preedit, mode_badge) = if inline_preedit {
        if is_dynamic_v_mode {
            (engine.raw_input().to_string(), "V模式".to_string())
        } else {
            match mode {
                InputMode::Chinese => (String::new(), String::new()),
                InputMode::English => (String::new(), "英".to_string()),
                InputMode::VMode => (engine.formatted_preedit(), "V".to_string()),
            }
        }
    } else {
        let badge = match mode {
            InputMode::Chinese => "中".to_string(),
            InputMode::English => "英".to_string(),
            InputMode::VMode => "V".to_string(),
        };
        (engine.formatted_preedit(), badge)
    };

    let selected_index = if is_grid_expanded {
        grid_selected_index
    } else {
        engine.selected_index()
    };

    CandidateBoxState {
        visible: is_active && !engine.raw_input().is_empty(),
        preedit,
        candidates: core_candidates,
        selected_index,
        expanded_card,
        mode_badge,
        is_grid_expanded,
        grid_candidates,
    }
}

/// 为会话实现 7 组通用的基础 Wayland Dispatch trait（消除 V1/V2 重复样板代码）
#[macro_export]
macro_rules! impl_common_wayland_dispatch {
    ($session_type:ty) => {
        impl wayland_client::Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()>
            for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_seat::WlSeat,
                _event: <wayland_client::protocol::wl_seat::WlSeat as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_compositor::WlCompositor,
                (),
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_compositor::WlCompositor,
                _event: <wayland_client::protocol::wl_compositor::WlCompositor as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl wayland_client::Dispatch<wayland_client::protocol::wl_shm::WlShm, ()>
            for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_shm::WlShm,
                _event: <wayland_client::protocol::wl_shm::WlShm as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_surface::WlSurface,
                (),
            > for $session_type
        {
            fn event(
                state: &mut Self,
                _proxy: &wayland_client::protocol::wl_surface::WlSurface,
                event: <wayland_client::protocol::wl_surface::WlSurface as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
                if let wayland_client::protocol::wl_surface::Event::PreferredBufferScale { factor } =
                    event
                {
                    state.surface_manager.set_scale(factor);
                }
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_shm_pool::WlShmPool,
                (),
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_shm_pool::WlShmPool,
                _event: <wayland_client::protocol::wl_shm_pool::WlShmPool as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_buffer::WlBuffer,
                (),
            > for $session_type
        {
            fn event(
                state: &mut Self,
                proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
                event: <wayland_client::protocol::wl_buffer::WlBuffer as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
                if matches!(
                    event,
                    wayland_client::protocol::wl_buffer::Event::Release
                ) {
                    state.surface_manager.on_buffer_released(proxy);
                }
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_registry::WlRegistry,
                wayland_client::globals::GlobalListContents,
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_registry::WlRegistry,
                _event: <wayland_client::protocol::wl_registry::WlRegistry as wayland_client::Proxy>::Event,
                _data: &wayland_client::globals::GlobalListContents,
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }
    };
}

/// 隐藏候选框并解绑关联的 Wayland 表面缓冲区
pub fn hide_candidate_surface_buffer(
    surface_manager: &mut CandidateSurfaceManager,
    popup_wl_surface: Option<&WlSurface>,
    active_buffer: &mut Option<WlBuffer>,
) {
    surface_manager.hide_wayland_surface(popup_wl_surface, active_buffer);
}

/// 外部按键事件拦截处理结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFilterResult {
    /// 未拦截该按键，继续按默认输入法流程处理
    PassThrough,
    /// 该按键已被外部处理程序消费，不进入拼音/标点管线，亦不透传给客户端应用
    Consumed,
    /// 拦截消费并要求直接向当前客户端提交文本
    Commit(String),
}

/// 外部按键拦截回调函数类型，入参为 (evdev_keycode, is_press, mods_depressed)
pub type KeyFilterCallback = Box<dyn FnMut(u32, bool, u32) -> KeyFilterResult + Send>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use waylan_algo::types::AlgoConfig;

    #[test]
    fn test_sync_mode_from_shared_flag() {
        let flag = AtomicBool::new(false);
        let mut mode = InputMode::Chinese;

        let changed = sync_mode_from_shared_flag(Some(&flag), &mut mode);
        assert_eq!(changed, Some(InputMode::English));
        assert_eq!(mode, InputMode::English);

        // 再次同步，无变化
        let changed = sync_mode_from_shared_flag(Some(&flag), &mut mode);
        assert_eq!(changed, None);
        assert_eq!(mode, InputMode::English);

        // 切换回中文
        flag.store(true, Ordering::Release);
        let changed = sync_mode_from_shared_flag(Some(&flag), &mut mode);
        assert_eq!(changed, Some(InputMode::Chinese));
        assert_eq!(mode, InputMode::Chinese);
    }

    #[test]
    fn test_check_reload_requested() {
        let flag = AtomicBool::new(true);
        let mut watcher = None;
        let paths: Vec<std::path::PathBuf> = Vec::new();
        let mut mtime = None;

        let needed =
            check_reload_requested_or_files_changed(Some(&flag), &mut watcher, &paths, &mut mtime);
        assert!(needed);
        assert!(!flag.load(Ordering::Relaxed)); // 验证已重置为 false
    }

    #[test]
    fn test_apply_default_engine_reload() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        apply_default_engine_reload(&mut engine);
        assert_eq!(engine.raw_input(), "");
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

        let mut watcher = InotifyWatcher::try_new_with_filter(
            std::slice::from_ref(&watched_aux),
            Some(default_user_dict_filter()),
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
