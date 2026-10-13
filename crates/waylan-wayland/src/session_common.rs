//! V1 与 V2 输入法会话通用状态管理与辅助同步逻辑
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 (`zwp_input_method_v1`) 与 V2 (`zwp_input_method_v2`)
//!   会话实现中完全重复的外部状态感知、inotify 文件变更监听、修饰键跟踪与输入模式同步样板代码。
//! - **核心优势**：将热重载检测、原子状态同步与修饰键掩码判定收敛为单一真相源（Single Source of Truth），
//!   防止后续协议演进时维护失步。
//! - **代价与局限**：调用方需传递对应的字段引用，相比完全内嵌略微增加了函数调用跳转，但消除了数十处重复代码。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use log::{info, warn};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::AlgoEngine;
use waylan_core::{CandidateBoxState, InputMode};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

use crate::fs_watcher::{check_watched_paths_changed, InotifyWatcher};
use crate::input_core::{update_modifier_masks, ModifierState};
use crate::surface::CandidateSurfaceManager;

/// 尝试初始化 inotify 文件监控器
///
/// 若当前已有监控器或监控路径列表为空，则静默跳过；若初始化失败则记录告警并安全降级至时间戳轮询。
pub fn init_session_inotify_watcher(
    watched_paths: &[PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
) {
    if !watched_paths.is_empty() && inotify_watcher.is_none() {
        match InotifyWatcher::try_new(watched_paths) {
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
/// 优先排空 inotify 队列事件，回退时比对被监控文件的时间戳修改记录。
pub fn check_reload_requested_or_files_changed(
    reload_requested: Option<&AtomicBool>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    watched_paths: &[PathBuf],
    last_watched_mtime: &mut Option<SystemTime>,
) -> bool {
    let mut should_reload = reload_requested
        .map(|flag| flag.swap(false, Ordering::Relaxed))
        .unwrap_or(false);

    let changed = if let Some(ref mut watcher) = inotify_watcher {
        watcher.drain_events()
    } else {
        check_watched_paths_changed(watched_paths, last_watched_mtime)
    };
    if changed {
        should_reload = true;
    }
    should_reload
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

/// 从共享原子标志同步输入语言模式
///
/// 若模式发生变更，返回 `Some(new_mode)`；若未变更，返回 `None`。
pub fn sync_mode_from_shared_flag(
    shared_is_chinese: Option<&AtomicBool>,
    current_mode: &mut InputMode,
) -> Option<InputMode> {
    if let Some(zh_flag) = shared_is_chinese {
        let external_zh = zh_flag.load(Ordering::Acquire);
        let current_zh = matches!(*current_mode, InputMode::Chinese | InputMode::VMode);
        if external_zh != current_zh {
            let new_mode = if external_zh {
                InputMode::Chinese
            } else {
                InputMode::English
            };
            *current_mode = new_mode;
            return Some(new_mode);
        }
    }
    None
}

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
    (mods_locked & crate::input_constants::MOD_CAPS) != 0
}

/// 提交候选框状态至 Wayland 表面并更新活跃缓冲区
pub fn commit_candidate_surface<S>(
    surface_manager: &mut CandidateSurfaceManager,
    box_state: &CandidateBoxState,
    shm: Option<&WlShm>,
    popup_wl_surface: Option<&WlSurface>,
    qhandle: Option<&QueueHandle<S>>,
    active_buffer: &mut Option<WlBuffer>,
) where
    S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    surface_manager.commit_to_wayland(box_state, shm, popup_wl_surface, qhandle, active_buffer);
}

/// 隐藏候选框并解绑关联的 Wayland 表面缓冲区
pub fn hide_candidate_surface_buffer(
    surface_manager: &mut CandidateSurfaceManager,
    popup_wl_surface: Option<&WlSurface>,
    active_buffer: &mut Option<WlBuffer>,
) {
    surface_manager.hide_wayland_surface(popup_wl_surface, active_buffer);
}

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
        let paths = Vec::new();
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
