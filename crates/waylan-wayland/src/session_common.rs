//! V1 与 V2 输入法会话通用状态管理与辅助同步逻辑
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 (`zwp_input_method_v1`) 与 V2 (`zwp_input_method_v2`)
//!   会话实现中完全重复的外部状态感知、inotify 文件变更监听、修饰键跟踪与输入模式同步样板代码。
//! - **核心优势**：将热重载检测、原子状态同步与修饰键掩码判定收敛为单一真相源（Single Source of Truth），
//!   防止后续协议演进时维护失步。
//! - **代价与局限**：调用方需传递对应的字段引用，相比完全内嵌略微增加了函数调用跳转，但消除了数十处重复代码。

use std::sync::atomic::{AtomicBool, Ordering};

use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::AlgoEngine;
use waylan_core::{CandidateBoxState, InputMode};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

use crate::fs_watcher::{default_user_dict_filter, InotifyWatcher};
use crate::surface::CandidateSurfaceManager;
use waylan_core::input::{update_modifier_masks, ModifierState, MOD_CAPS};

pub use waylan_core::fs_watcher::check_reload_requested_or_files_changed;

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
    let wal_opt = engine.user_dict_path.clone();
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
) -> CandidateBoxState {
    use waylan_core::{Candidate, ExpandedCardState};

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

    let expanded_card = if engine.is_expanded {
        ExpandedCardState::Expanded {
            selected_index: engine.expanded_selected_index,
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
        engine.selected_index
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
}
