//! V1 与 V2 输入法会话通用状态管理与辅助同步逻辑
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 与 V2 会话中重复的状态感知、修饰键跟踪与输入模式同步样板代码。
//! - **核心优势**：将热重载检测、原子状态同步与修饰键掩码判定收敛为单一真相源（Single Source of Truth）。
//! - **代价与局限**：具体文件监控与候选框装配已解耦至 `watcher` 与 `surface::state_builder` 独立模块。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use log::info;
use waylan_core::input::{update_modifier_masks, ModifierState, MOD_CAPS};
use waylan_core::{AppConfig, InputMode, SharedInputFlags, VoiceProvider};
use waylan_engine::ImeRuntime;
use waylan_sys::LoopWakeup;
use waylan_ui::CandidateBoxRenderer;

pub use crate::surface::state_builder::*;
pub use waylan_core::input::sync_mode_from_shared_flags;
pub use waylan_engine::{apply_default_engine_reload, apply_default_runtime_reload};
pub use waylan_sys::fs_watcher::*;

/// 会话热重载回调闭包契约
pub type SessionReloadCallback<S> = Box<dyn FnMut(&mut S) + Send>;

/// Wayland 外部环境监控与会话生命周期协调器（统合 V1 与 V2 镜像结构）
pub struct WaylandWatcherContext<S> {
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<SessionReloadCallback<S>>,
    /// 监控外部进程修改的配置文件与词库路径列表
    pub watched_paths: Vec<PathBuf>,
    /// 上次记录的被监控文件最新状态签名
    pub last_watched_mtime: Option<FileSignature>,
    /// 基于 Linux inotify 的配置文件与词库变更监控器
    pub inotify_watcher: Option<InotifyWatcher>,
    /// 事件循环跨线程唤醒器
    pub loop_wakeup: Option<LoopWakeup>,
    /// 与外部系统托盘共享的优雅退出请求标志
    pub exit_requested: Option<Arc<AtomicBool>>,
}

impl<S> Default for WaylandWatcherContext<S> {
    fn default() -> Self {
        Self {
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            inotify_watcher: None,
            loop_wakeup: None,
            exit_requested: None,
        }
    }
}

/// Wayland 会话装配环境参数（用于消除 V1 与 V2 镜像赋值代码）
pub struct SessionSetupOptions<'a> {
    pub renderer: CandidateBoxRenderer,
    pub cfg: &'a AppConfig,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<FileSignature>,
    pub shared_flags: Option<SharedInputFlags>,
    pub mode_notify_tx: Option<SyncSender<()>>,
    pub reload_requested: Option<Arc<AtomicBool>>,
    pub exit_requested: Option<Arc<AtomicBool>>,
    pub loop_wakeup: Option<LoopWakeup>,
    pub voice_provider: Option<Box<dyn VoiceProvider>>,
}

/// 会话装配目标上下文（运行时、渲染器、监控上下文与热重载回调的可变借用集合）
///
/// # 设计原理
/// - **实现初衷**：会话装配需要同时操作运行时、候选框渲染器与文件监控上下文三个可变目标，
///   再叠加热重载回调后平铺传参易在 V1 与 V2 两处调用点发生顺序错配。
/// - **核心优势**：以参数对象聚合装配目标，调用点按名绑定每个目标，杜绝位置传参歧义。
/// - **代价与局限**：调用点需显式书写字段名。
pub struct SessionSetupTargets<'a, S: 'static> {
    /// 输入法核心运行时
    pub runtime: &'a mut ImeRuntime,
    /// 候选框渲染器
    pub renderer: &'a mut CandidateBoxRenderer,
    /// 外部环境监控上下文
    pub watcher: &'a mut WaylandWatcherContext<S>,
    /// 配置热重载回调
    pub reload_cb: fn(&mut S),
}

/// 统一为会话上下文应用运行参数、配置与监控回调
pub fn apply_session_setup<S: 'static>(
    targets: SessionSetupTargets<'_, S>,
    opts: SessionSetupOptions<'_>,
) {
    let SessionSetupTargets {
        runtime,
        renderer,
        watcher,
        reload_cb,
    } = targets;
    *renderer = opts.renderer;
    watcher.watched_paths = opts.watched_paths;
    watcher.last_watched_mtime = opts.initial_mtime;
    runtime.set_shared_flags(opts.shared_flags);
    runtime.set_mode_notify_tx(opts.mode_notify_tx);
    runtime.set_reload_requested(opts.reload_requested);
    runtime.set_voice_provider(opts.voice_provider);
    watcher.exit_requested = opts.exit_requested;
    watcher.loop_wakeup = opts.loop_wakeup;
    watcher.reload_callback = Some(Box::new(reload_cb));
    runtime.apply_initial_config(opts.cfg);
}

/// Wayland V1/V2 共用会话生命周期契约
///
/// 统一处理热重载检测、原子状态同步、输入模式切换与 inotify 初始化，协议会话只负责
/// 提供运行时/监控上下文访问器以及模式切换后的协议侧清理动作。
pub trait WaylandSessionLifecycle: Sized {
    /// 获取统一输入法运行时
    fn runtime(&mut self) -> &mut ImeRuntime;

    /// 获取外部环境监控上下文
    fn watcher(&mut self) -> &mut WaylandWatcherContext<Self>;

    /// 会话日志标签（V1 / V2）
    fn session_label(&self) -> &'static str;

    /// 拼音组合期间切换到英文模式时的协议侧清理动作
    fn on_english_mode_switch(&mut self);

    /// 同步外部系统托盘状态、配置热重载与文件变更
    fn sync_external_state(&mut self) {
        let label = self.session_label();
        let should_reload = {
            let reload_flag = self.runtime().reload_requested();
            let watcher = self.watcher();
            check_reload_requested_or_files_changed(
                reload_flag.as_deref(),
                &mut watcher.inotify_watcher,
                &watcher.watched_paths,
                &mut watcher.last_watched_mtime,
            )
        };

        if should_reload {
            let callback = self.watcher().reload_callback.take();
            if let Some(mut callback) = callback {
                callback(self);
                self.watcher().reload_callback = Some(callback);
            } else {
                apply_default_runtime_reload(self.runtime());
            }
            info!("[微澜 Wayland {}] 已完成用户词库与运行配置热重载。", label);
        }

        self.sync_mode_from_shared_flag();
    }

    /// 尝试根据当前被监控路径列表初始化 inotify 事件监控器
    fn init_inotify_watcher(&mut self) {
        let label = self.session_label();
        let watcher = self.watcher();
        init_session_inotify_watcher_with_filter(
            &watcher.watched_paths,
            &mut watcher.inotify_watcher,
            label,
            Some(default_user_dict_filter()),
        );
    }

    /// 按键热路径轻量同步：仅检查原子标志位
    fn sync_atomic_state(&mut self) {
        let reload_flag = self.runtime().reload_requested();
        if is_reload_requested(reload_flag.as_deref()) {
            self.sync_external_state();
            return;
        }
        self.sync_mode_from_shared_flag();
    }

    /// 从共享原子标志同步输入模式
    fn sync_mode_from_shared_flag(&mut self) {
        let mode_changed = self.runtime().sync_shared_mode();
        if let Some(InputMode::English) = mode_changed {
            if !self.runtime().core.engine.raw_input().is_empty() {
                self.on_english_mode_switch();
            }
        }
    }
}

/// 构造默认的用户词库过滤回调（忽略短时间内的 WAL 内部写落盘事件）
pub fn default_user_dict_filter() -> DirEventFilter {
    create_timed_file_event_filter("user_dict.bin", None, 2000)
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
    (mods_locked & MOD_CAPS) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use waylan_core::state::SharedInputFlags;
    use waylan_core::InputMode;

    #[test]
    fn test_sync_mode_from_shared_flags() {
        let flags = SharedInputFlags::new(true, false, false, false);
        let mut mode = InputMode::Chinese;

        flags.set_chinese(false);
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
        assert_eq!(changed, Some(InputMode::English));
        assert_eq!(mode, InputMode::English);

        // 再次同步，无变化
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
        assert_eq!(changed, None);
        assert_eq!(mode, InputMode::English);

        // 切换回中文
        flags.set_chinese(true);
        let changed = sync_mode_from_shared_flags(Some(&flags), &mut mode);
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
}
