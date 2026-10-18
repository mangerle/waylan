use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use log::{debug, info};
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::{AlgoEngine, ImeRuntime, InputMethodFrontend};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_keyboard::WlKeyboard, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::{
    Event as ContextEvent, ZwpInputMethodContextV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_method_v1::{
    Event as MethodEvent, ZwpInputMethodV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1;
use crate::surface::CandidateSurfaceManager;

use crate::session_common::*;
use waylan_core::fs_watcher::InotifyWatcher;
pub use waylan_core::input::*;
use waylan_core::LoopWakeup;

/// V1 会话配置与词库热重载回调函数类型
pub type V1ReloadCallback = Box<dyn FnMut(&mut WaylanV1Session) + Send>;

/// 微澜输入法会话状态机
#[derive(Default)]
pub struct WaylanV1Session {
    /// 统一输入法交互运行时与算法决策状态机
    pub runtime: ImeRuntime,
    /// 候选框悬浮图层管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<V1ReloadCallback>,
    /// 监控外部进程修改的配置文件与词库路径列表（用于在焦点或按键事件时自动感知变更）
    pub watched_paths: Vec<PathBuf>,
    /// 上次记录的被监控文件最新状态签名
    pub last_watched_mtime: Option<waylan_core::FileSignature>,
    /// 当前激活的应用上下文引用
    pub active_context: Option<ZwpInputMethodContextV1>,
    /// 捕获的键盘对象
    pub grabbed_keyboard: Option<WlKeyboard>,
    /// 关联的共享内存管理器句柄
    pub shm: Option<WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<WlSurface>,
    /// V1 输入面板表面角色对象 (zwp_input_panel_surface_v1)
    pub panel_surface: Option<ZwpInputPanelSurfaceV1>,
    /// WLR Layer Shell 表面角色对象（用于 Sway / wlroots 等环境）
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前硬件按键事件序号
    pub current_serial: u32,
    /// 文本输入上下文状态同步序号（由 commit_state 事件更新，用于 commit_string 与 preedit_string）
    pub commit_serial: u32,
    /// 与外部系统托盘共享的优雅退出请求标志
    pub exit_requested: Option<Arc<AtomicBool>>,
    /// 基于 Linux inotify 的配置文件与词库变更监控器
    pub inotify_watcher: Option<InotifyWatcher>,
    /// 事件循环跨线程唤醒器
    pub loop_wakeup: Option<LoopWakeup>,
}

impl WaylanV1Session {
    /// 获取核心输入法会话引用
    #[inline]
    pub fn core(&self) -> &waylan_engine::session::ImeCoreSession {
        &self.runtime.core
    }

    /// 获取核心输入法会话可变引用
    #[inline]
    pub fn core_mut(&mut self) -> &mut waylan_engine::session::ImeCoreSession {
        &mut self.runtime.core
    }

    /// 拆分统一运行时引用与轻量前端管道借用
    pub fn split_runtime_and_frontend(&mut self) -> (&mut ImeRuntime, WaylanV1Frontend<'_>) {
        let frontend = WaylanV1Frontend {
            active_context: self.active_context.as_ref(),
            commit_serial: self.commit_serial,
            surface_manager: &mut self.surface_manager,
            active_buffer: &mut self.active_buffer,
            layer_surface: self.layer_surface.as_ref(),
            popup_wl_surface: self.popup_wl_surface.as_ref(),
            shm: self.shm.as_ref(),
            qhandle: self.qhandle.as_ref(),
        };
        (&mut self.runtime, frontend)
    }

    /// 创建默认配置的 Wayland V1 输入法会话实例
    ///
    /// # 设计原理
    /// - **实现初衷**：为支持早期或经典 Wayland 协议的桌面环境提供开箱即用的默认输入法会话。
    /// - **核心优势**：内建示例字库与标准标点映射，无需前置繁琐配置即可执行协议握手与连通性验证。
    /// - **代价与局限**：搭载精简示例词库，生产环境通常需使用 `with_engine` 注入完整词库实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用外部预加载并配置好的算法引擎构造输入法会话
    ///
    /// # 设计原理
    /// - **实现初衷**：在生产环境下注入已完成字库加载、用户自适应词典配置与全量词库映射的核心引擎。
    /// - **核心优势**：在 V2 降级或会话重启时直接移交核心引擎状态，避免数十兆词库在内存中二次加载解析。
    /// - **代价与局限**：调用方需负责算法引擎的预热与全生命周期参数调优。
    pub fn with_engine(engine: AlgoEngine) -> Self {
        Self {
            runtime: ImeRuntime::with_engine(engine),
            ..Self::default()
        }
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（优先基于 inotify 事件感知，回退时使用时间戳轮询）
    pub(crate) fn sync_external_state(&mut self) {
        let reload_flag = self.runtime.reload_requested.clone();
        let should_reload = check_reload_requested_or_files_changed(
            reload_flag.as_deref(),
            &mut self.inotify_watcher,
            &self.watched_paths,
            &mut self.last_watched_mtime,
        );

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                apply_default_runtime_reload(&mut self.runtime);
            }
            info!("[微澜 Wayland V1] 已完成用户词库与运行配置热重载。");
        }

        self.sync_mode_from_shared_flag();
    }

    /// 尝试根据当前被监控路径列表初始化 inotify 事件监控器
    pub(crate) fn init_inotify_watcher(&mut self) {
        init_session_inotify_watcher(&self.watched_paths, &mut self.inotify_watcher, "Wayland V1");
    }

    /// 计算事件循环下一次 poll 的等待超时毫秒数
    pub(crate) fn next_poll_timeout_ms(&self, default_ms: i32) -> i32 {
        default_ms
    }

    /// 按键热路径轻量同步：仅检查原子标志位，避免每次敲击按键触发磁盘文件 metadata 系统调用
    pub(crate) fn sync_atomic_state(&mut self) {
        let reload_flag = self.runtime.reload_requested.clone();
        if is_reload_requested(reload_flag.as_deref()) {
            self.sync_external_state();
            return;
        }

        self.sync_mode_from_shared_flag();
    }

    /// 从共享原子标志同步输入模式，若在拼音组合期间切至英文则立即清理残留预编辑与候选框
    fn sync_mode_from_shared_flag(&mut self) {
        let changed = self.runtime.sync_shared_mode();
        if let Some(InputMode::English) = changed {
            if !self.runtime.core.engine.raw_input().is_empty() {
                self.runtime.core.engine.reset();
                self.hide_candidate_surface();
                if let Some(ref ctx) = self.active_context {
                    ctx.preedit_cursor(0);
                    ctx.preedit_string(self.commit_serial, String::new(), String::new());
                }
            }
        }
    }

    /// 查询当前是否处于语音录音或语音浮条激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.runtime.is_voice_active()
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    pub fn poll_voice_cycle(&mut self) -> Option<String> {
        let (runtime, mut frontend) = self.split_runtime_and_frontend();
        runtime.poll_voice_cycle(&mut frontend, false)
    }

    /// 构造当前候选框的渲染状态
    pub(crate) fn build_candidate_box_state(&self) -> CandidateBoxState {
        self.runtime.build_candidate_box_state(true, false)
    }

    /// 触发候选框悬浮表面位图重绘并提交至 Wayland 表面
    ///
    /// 使用给定的候选框状态渲染并提交至 Wayland 表面
    pub fn render_candidate_surface(&mut self, state: &CandidateBoxState) {
        let params = CandidateSurfaceCommitParams {
            shm: self.shm.as_ref(),
            popup_wl_surface: self.popup_wl_surface.as_ref(),
            qhandle: self.qhandle.as_ref(),
            active_buffer: &mut self.active_buffer,
        };
        commit_candidate_surface(&mut self.surface_manager, state, params);

        if let Some(ref layer_surf) = self.layer_surface {
            let scale = self.surface_manager.scale.max(1) as u32;
            let (phys_w, phys_h) = self.surface_manager.current_size;
            let logical_w = (phys_w / scale).max(1);
            let logical_h = (phys_h / scale).max(1);
            let (pos_x, pos_y) = self
                .surface_manager
                .compute_popup_position(logical_w, logical_h);
            layer_surf.set_size(logical_w, logical_h);
            layer_surf.set_margin(pos_y.max(0), 0, 0, pos_x.max(0));
            if let Some(ref surf) = self.popup_wl_surface {
                surf.commit();
            }
        }
    }

    /// # 设计原理
    /// - **实现初衷**：在 V1 协议下将算法引擎候选状态渲染并提交至 InputPanel 或 LayerShell 悬浮表面。
    /// - **核心优势**：自动处理边缘防溢出坐标计算与 LayerShell 边距定位更新，确保候选框吸附于光标附近。
    /// - **代价与局限**：在无 LayerShell 支持的环境下依赖 InputPanel 原生相对偏移定位。
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        self.render_candidate_surface(&box_state);
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    ///
    /// # 设计原理
    /// - **实现初衷**：在 V1 会话失焦或清空输入时卸载候选表面，恢复屏幕原状。
    /// - **核心优势**：通知合成器解绑当前活动缓冲，安全回收共享内存槽位。
    /// - **代价与局限**：需等待合成器处理缓冲解绑与释放确认。
    pub fn hide_candidate_surface(&mut self) {
        self.surface_manager
            .hide_wayland_surface(self.popup_wl_surface.as_ref(), &mut self.active_buffer);
    }

    /// 重置会话的修饰键位图、按键消费记录及算法引擎状态
    fn reset_input_state(&mut self) {
        self.runtime.core.reset_input_state();
    }

    /// 处理 V1 协议下的会话激活与键盘抓取建立
    fn handle_v1_activate(
        &mut self,
        context: ZwpInputMethodContextV1,
        qhandle: &QueueHandle<Self>,
    ) {
        debug!("[微澜] 收到激活事件：用户已进入文本输入框。");
        self.sync_external_state();
        if let Some(old_kb) = self.grabbed_keyboard.take() {
            if old_kb.version() >= 3 {
                old_kb.release();
            }
        }
        if let Some(old_ctx) = self.active_context.take() {
            old_ctx.destroy();
        }
        let keyboard = context.grab_keyboard(qhandle, ());
        self.grabbed_keyboard = Some(keyboard);
        self.active_context = Some(context);
        self.reset_input_state();
    }

    /// 处理 V1 协议下的会话失焦与资源回收
    fn handle_v1_deactivate(&mut self, context: &ZwpInputMethodContextV1) {
        debug!("[微澜] 收到失焦事件：用户已离开文本输入框。");
        if self.active_context.as_ref() == Some(context) {
            if let Some(active_ctx) = self.active_context.take() {
                active_ctx.destroy();
            }
            if let Some(old_kb) = self.grabbed_keyboard.take() {
                if old_kb.version() >= 3 {
                    old_kb.release();
                }
            }
            self.runtime.core.reset_input_state();
            self.hide_candidate_surface();
            let _ = self.runtime.core.engine.maybe_compact_user_dict();
        }
    }
}

// 为 zwp_input_method_v1 实现事件分发
impl Dispatch<ZwpInputMethodV1, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodV1,
        event: MethodEvent,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            MethodEvent::Activate { id } => state.handle_v1_activate(id, qhandle),
            MethodEvent::Deactivate { context } => state.handle_v1_deactivate(&context),
        }
    }
}

// 为 zwp_input_method_context_v1 实现生命周期分发
impl Dispatch<ZwpInputMethodContextV1, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodContextV1,
        event: ContextEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ContextEvent::CommitState { serial } => {
                state.commit_serial = serial;
            }
            ContextEvent::Reset => {
                state.runtime.core.shift_sm.reset();
                state.runtime.core.punctuation_processor.reset();
                state.runtime.core.engine.reset();
                state.hide_candidate_surface();
            }
            _ => {}
        }
    }
}

impl InputMethodFrontend for WaylanV1Session {
    fn commit_text(&mut self, text: &str) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.commit_text(text);
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.update_preedit(text, cursor_byte_idx);
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.update_candidates(state);
    }

    fn hide_candidates(&mut self) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.hide_candidates();
    }

    fn move_cursor(&mut self, offset: i32) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.move_cursor(offset);
    }

    fn sync_input_mode(&mut self, mode: InputMode) {
        let (_, mut frontend) = self.split_runtime_and_frontend();
        frontend.sync_input_mode(mode);
    }
}

mod dispatch;
mod frontend;
mod keyboard;

pub use frontend::WaylanV1Frontend;

#[cfg(test)]
mod session_v1_tests;
