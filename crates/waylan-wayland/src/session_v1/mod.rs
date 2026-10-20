use log::debug;
use waylan_core::CandidateBoxState;
use waylan_engine::{AlgoEngine, ImeRuntime};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::{
    Event as ContextEvent, ZwpInputMethodContextV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_method_v1::{
    Event as MethodEvent, ZwpInputMethodV1,
};

use crate::session_common::*;

pub mod context;
mod dispatch;
mod frontend;
mod keyboard;

pub use context::{WaylandV1ProtocolContext, WaylandV1SurfaceContext, WaylandV1WatcherContext};
pub use frontend::WaylanV1Frontend;

/// V1 会话配置与词库热重载回调函数类型
pub type V1ReloadCallback = Box<dyn FnMut(&mut WaylanV1Session) + Send>;

/// 微澜输入法 V1 会话状态机
///
/// # 设计原理
/// - **实现初衷**：消除上帝结构体，将平铺的 17 个字段聚合解构为协议、表面、监控 3 个高内聚子结构体。
/// - **核心优势**：
///   1. 不相交字段借用：解耦运行时、协议通道与双缓冲渲染，按键热路径无需临时解构切分；
///   2. 架构统一：与 V2 会话的设计规范保持一致；
///   3. 零成本适配：保留 `frontend()` 适配器与便捷构造方法。
/// - **代价与局限**：深层字段访问需通过子结构体路径导航。
#[derive(Default)]
pub struct WaylanV1Session {
    /// 统一输入法交互运行时与算法决策状态机
    pub runtime: ImeRuntime,
    /// 协议原生对象句柄与事务同步上下文
    pub protocol: WaylandV1ProtocolContext,
    /// 候选框悬浮表面渲染与缓冲区上下文
    pub surface: WaylandV1SurfaceContext,
    /// 外部环境监控与会话生命周期协调器
    pub watcher: WaylandV1WatcherContext,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
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

    /// 构造轻量前端渲染管道
    #[inline]
    pub fn frontend(&mut self) -> WaylanV1Frontend<'_> {
        WaylanV1Frontend::new(&self.protocol, &mut self.surface, self.qhandle.as_ref())
    }

    /// 拆分统一运行时引用与轻量前端管道借用（兼容旧调用契约）
    #[inline]
    pub fn split_runtime_and_frontend(&mut self) -> (&mut ImeRuntime, WaylanV1Frontend<'_>) {
        let frontend =
            WaylanV1Frontend::new(&self.protocol, &mut self.surface, self.qhandle.as_ref());
        (&mut self.runtime, frontend)
    }

    /// 创建默认配置的 Wayland V1 输入法会话实例
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用外部预加载并配置好的算法引擎构造输入法会话
    pub fn with_engine(engine: AlgoEngine) -> Self {
        Self {
            runtime: ImeRuntime::with_engine(engine),
            ..Self::default()
        }
    }

    /// 计算事件循环下一次 poll 的等待超时毫秒数
    pub(crate) fn next_poll_timeout_ms(&self, default_ms: i32) -> i32 {
        default_ms
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
    pub fn render_candidate_surface(&mut self, state: &CandidateBoxState) {
        let params = CandidateSurfaceCommitParams {
            shm: self.surface.shm.as_ref(),
            popup_wl_surface: self.surface.popup_wl_surface.as_ref(),
            qhandle: self.qhandle.as_ref(),
            active_buffer: &mut self.surface.active_buffer,
        };
        commit_candidate_surface(&mut self.surface.surface_manager, state, params);

        if let Some(ref layer_surf) = self.protocol.layer_surface {
            let scale = self.surface.surface_manager.scale.max(1) as u32;
            let (phys_w, phys_h) = self.surface.surface_manager.current_size;
            let logical_w = (phys_w / scale).max(1);
            let logical_h = (phys_h / scale).max(1);
            let (pos_x, pos_y) = self
                .surface
                .surface_manager
                .compute_popup_position(logical_w, logical_h);
            layer_surf.set_size(logical_w, logical_h);
            layer_surf.set_margin(pos_y.max(0), 0, 0, pos_x.max(0));
            if let Some(ref surf) = self.surface.popup_wl_surface {
                surf.commit();
            }
        }
    }

    /// 刷新候选框悬浮表面
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        self.render_candidate_surface(&box_state);
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    pub fn hide_candidate_surface(&mut self) {
        self.surface.surface_manager.hide_wayland_surface(
            self.surface.popup_wl_surface.as_ref(),
            &mut self.surface.active_buffer,
        );
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
        if let Some(old_kb) = self.protocol.grabbed_keyboard.take() {
            if old_kb.version() >= 3 {
                old_kb.release();
            }
        }
        if let Some(old_ctx) = self.protocol.active_context.take() {
            old_ctx.destroy();
        }
        let keyboard = context.grab_keyboard(qhandle, ());
        self.protocol.grabbed_keyboard = Some(keyboard);
        self.protocol.active_context = Some(context);
        self.reset_input_state();
    }

    /// 处理 V1 协议下的会话失焦与资源回收
    fn handle_v1_deactivate(&mut self, context: &ZwpInputMethodContextV1) {
        debug!("[微澜] 收到失焦事件：用户已离开文本输入框。");
        if self.protocol.active_context.as_ref() == Some(context) {
            if let Some(active_ctx) = self.protocol.active_context.take() {
                active_ctx.destroy();
            }
            if let Some(old_kb) = self.protocol.grabbed_keyboard.take() {
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

impl WaylandSessionLifecycle for WaylanV1Session {
    #[inline]
    fn runtime(&mut self) -> &mut ImeRuntime {
        &mut self.runtime
    }

    #[inline]
    fn watcher(&mut self) -> &mut WaylandWatcherContext<Self> {
        &mut self.watcher
    }

    #[inline]
    fn session_label(&self) -> &'static str {
        "V1"
    }

    fn on_english_mode_switch(&mut self) {
        self.runtime.core.engine.reset();
        self.hide_candidate_surface();
        if let Some(ref ctx) = self.protocol.active_context {
            ctx.preedit_cursor(0);
            ctx.preedit_string(self.protocol.commit_serial, String::new(), String::new());
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
                state.protocol.commit_serial = serial;
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

#[cfg(test)]
mod session_v1_tests;
