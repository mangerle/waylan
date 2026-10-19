pub mod context;
mod frontend;
mod keys;
mod lifecycle;
mod surrounding;

pub use context::{
    WaylandV2FocusContext, WaylandV2KeyboardTracker, WaylandV2ProtocolContext,
    WaylandV2SurfaceContext, WaylandV2WatcherContext,
};
pub use frontend::WaylanV2Frontend;

use log::{debug, warn};
use waylan_core::CandidateBoxState;
use waylan_engine::{
    build_common_candidate_box_state, AlgoEngine, CandidateBoxBuildOptions, ImeRuntime,
    InputMethodFrontend,
};
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;

use crate::session_common::*;
pub use surrounding::{calculate_delete_backward_range, calculate_delete_forward_range};
pub use waylan_core::input::*;
pub use waylan_core::InputMode;

/// 配置与词库热重载回调函数类型
pub type ReloadCallback = Box<dyn FnMut(&mut WaylanV2Session) + Send>;

/// 微澜面向现代 Wayland 合成器 (KDE Plasma 6 / GNOME 46+ / Hyprland) 的 V2 会话状态
pub struct WaylanV2Session {
    /// 统一输入法交互运行时 (IME Runtime)
    pub runtime: ImeRuntime,
    /// 协议对象与扩展句柄上下文
    pub protocol: WaylandV2ProtocolContext,
    /// 表面渲染、双缓冲与鼠标交互上下文
    pub surface: WaylandV2SurfaceContext,
    /// 会话激活、焦点与周围文本上下文
    pub focus: WaylandV2FocusContext,
    /// 键盘修饰键与长按连击跟踪器
    pub keyboard: WaylandV2KeyboardTracker,
    /// 词库监控与生命周期协调器
    pub watcher: WaylandV2WatcherContext,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
}

impl WaylanV2Session {
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

    /// 查询 5 行大网格候选卡片当前是否处于展开状态
    #[inline]
    pub fn is_grid_expanded(&self) -> bool {
        self.runtime.core.is_grid_expanded()
    }

    /// 获取 5 行大网格候选卡片当前选中的候选词全局索引
    #[inline]
    pub fn grid_selected_index(&self) -> usize {
        self.runtime.core.grid_selected_index()
    }

    /// 基于已加载并配置好的核心算法引擎构造 Wayland V2 输入法会话
    ///
    /// # 设计原理
    /// - **实现初衷**：对接 Wayland `zwp_input_method_v2` 协议标准，通过组合子上下文解构与双缓冲事务机制实现与合成器解耦。
    /// - **核心优势**：通过子结构体划分彻底消除借用冲突，支持虚拟键盘直通回退、周围文本感知与客户端按键连击加速。
    /// - **代价与局限**：强依赖合成器完整实现 V2 输入法扩展协议。
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            runtime: ImeRuntime::with_engine(engine),
            protocol: WaylandV2ProtocolContext::default(),
            surface: WaylandV2SurfaceContext::default(),
            focus: WaylandV2FocusContext::default(),
            keyboard: WaylandV2KeyboardTracker {
                repeat_info: (25, 400),
                ..Default::default()
            },
            watcher: WaylandV2WatcherContext::default(),
            qhandle: None,
        }
    }

    /// 获取当前会话协议管道的前端轻量借用上下文
    #[inline]
    pub fn frontend(&mut self) -> WaylanV2Frontend<'_> {
        WaylanV2Frontend {
            protocol: &self.protocol,
            surface: &mut self.surface,
            qhandle: self.qhandle.as_ref(),
        }
    }

    /// 拆分持有统一运行时与协议前端管道借用（向后兼容接口）
    #[inline]
    pub fn split_runtime_and_frontend(&mut self) -> (&mut ImeRuntime, WaylanV2Frontend<'_>) {
        (
            &mut self.runtime,
            WaylanV2Frontend {
                protocol: &self.protocol,
                surface: &mut self.surface,
                qhandle: self.qhandle.as_ref(),
            },
        )
    }

    /// 查询当前是否处于语音录音或语音浮条激活状态
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.runtime.is_voice_active()
    }

    /// 轮询并消费后台语音引擎上报的事件，实时刷新波形或提取完成文字
    pub fn poll_voice_cycle(&mut self) -> Option<String> {
        let mut frontend =
            WaylanV2Frontend::new(&self.protocol, &mut self.surface, self.qhandle.as_ref());
        self.runtime.poll_voice_cycle(&mut frontend, true)
    }

    /// 设置关联的 V2 输入法实例
    pub(crate) fn set_input_method(&mut self, im: ZwpInputMethodV2) {
        self.protocol.input_method = Some(im);
    }

    /// 根据当前算法引擎与输入模式构建候选框渲染状态
    pub(crate) fn build_candidate_box_state(&self) -> CandidateBoxState {
        let voice_state = self
            .runtime
            .voice_manager
            .as_ref()
            .and_then(|vm| vm.render_state());

        build_common_candidate_box_state(
            &self.runtime.core.engine,
            CandidateBoxBuildOptions::from_session(
                &self.runtime.core,
                self.focus.is_active,
                true,
                voice_state,
            ),
        )
    }

    /// 触发候选框悬浮表面重绘并提交至 Wayland 表面
    pub fn render_candidate_surface(&mut self, state: &CandidateBoxState) {
        let params = CandidateSurfaceCommitParams {
            shm: self.protocol.shm.as_ref(),
            popup_wl_surface: self.surface.popup_wl_surface.as_ref(),
            qhandle: self.qhandle.as_ref(),
            active_buffer: &mut self.surface.active_buffer,
        };
        commit_candidate_surface(&mut self.surface.surface_manager, state, params);
    }

    /// 主动刷新并提交合成器渲染候选框
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        self.render_candidate_surface(&box_state);
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    pub fn hide_candidate_surface(&mut self) {
        hide_candidate_surface_buffer(
            &mut self.surface.surface_manager,
            self.surface.popup_wl_surface.as_ref(),
            &mut self.surface.active_buffer,
        );
    }

    /// 提交指定文本上屏，并重置预编辑状态
    pub(crate) fn commit_text(&mut self, text: &str) {
        self.runtime.core.collapse_grid();
        if let Some(ref im) = self.protocol.input_method {
            im.commit_string(text.to_string());
            im.set_preedit_string(String::new(), 0, 0);
            im.commit(self.protocol.done_serial);
        }
        self.hide_candidate_surface();
    }

    /// 更新当前 preedit 预编辑字符串到目标客户端
    pub(crate) fn update_preedit(&mut self, preedit: &str) {
        if let Some(ref im) = self.protocol.input_method {
            let cursor = if preedit.is_empty() {
                0
            } else {
                self.runtime.core.engine.formatted_cursor_byte_offset() as i32
            };
            im.set_preedit_string(preedit.to_string(), cursor, cursor);
            im.commit(self.protocol.done_serial);
        }
        if preedit.is_empty() {
            self.hide_candidate_surface();
        } else {
            self.refresh_candidate_surface();
        }
    }

    /// 分段确认部分文本上屏，并以剩余拼音继续推进预编辑与候选渲染
    pub(crate) fn commit_partial(&mut self, text: &str, remaining_preedit: &str) {
        if let Some(ref im) = self.protocol.input_method {
            im.commit_string(text.to_string());
            let cursor = self.runtime.core.engine.formatted_cursor_byte_offset() as i32;
            im.set_preedit_string(remaining_preedit.to_string(), cursor, cursor);
            im.commit(self.protocol.done_serial);
        }
        self.refresh_candidate_surface();
    }

    /// 取消当前输入，重置引擎与预编辑区
    pub(crate) fn cancel_input(&mut self) {
        self.runtime.core.collapse_grid();
        self.runtime.core.engine.reset();
        self.update_preedit("");
    }

    /// 触发退格向后删除（空缓冲区或英文模式下删除光标前 1 个字符或删除当前选区）
    pub fn delete_backward(&mut self) {
        if let Some(ref im) = self.protocol.input_method {
            let (before, after) =
                calculate_delete_backward_range(self.focus.surrounding_text.as_ref());
            if before > 0 || after > 0 {
                im.delete_surrounding_text(before, after);
                im.commit(self.protocol.done_serial);
            }
        }
    }

    /// 触发向前删除（Delete 键删除光标后 1 个字符或删除当前选区）
    pub fn delete_forward(&mut self) {
        if let Some(ref im) = self.protocol.input_method {
            let (before, after) =
                calculate_delete_forward_range(self.focus.surrounding_text.as_ref());
            if before > 0 || after > 0 {
                im.delete_surrounding_text(before, after);
                im.commit(self.protocol.done_serial);
            }
        }
    }

    /// 将未被输入法消费的原始物理按键通过虚拟键盘协议回投到席位
    pub(crate) fn forward_key_to_seat(&mut self, time: u32, key: u32, is_press: bool) {
        if !self.protocol.has_virtual_keymap {
            debug!(
                "[Wayland v2 键盘回投] 虚拟键盘 keymap 尚未就绪，跳过转发键码 {} (press={})",
                key, is_press
            );
            return;
        }
        if let Some(ref vk) = self.protocol.virtual_keyboard {
            let state_val = if is_press { 1 } else { 0 };
            vk.key(time, key, state_val);
        } else {
            warn!(
                "[Wayland v2 键盘回投 警告] 当前会话未绑定 zwp_virtual_keyboard_v1 实例，键码 {} 被丢弃",
                key
            );
        }
    }

    /// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
    pub(crate) fn process_key_event(&mut self, key: u32, is_press: bool) -> (bool, Option<String>) {
        keys::process_key_event(self, key, is_press)
    }

    /// 计算下一次 poll 等待超时时间（毫秒），兼顾按键长按连击定时器
    pub(crate) fn next_poll_timeout_ms(&self, default_ms: i32) -> i32 {
        keys::next_poll_timeout_ms(self, default_ms)
    }

    /// 检查并触发到期的长按按键连击事件
    pub(crate) fn drive_key_repeat(&mut self) {
        keys::drive_key_repeat(self);
    }

    /// 处理单个硬件按键按下/释放事件（返回本次直接上屏的文本，便于无合成器环境下单元测试验证）
    pub fn handle_key_event(&mut self, key: u32, is_press: bool) -> Option<String> {
        self.process_key_event(key, is_press).1
    }
}

impl InputMethodFrontend for WaylanV2Session {
    fn commit_text(&mut self, text: &str) {
        self.frontend().commit_text(text);
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        self.frontend().update_preedit(text, cursor_byte_idx);
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        self.frontend().update_candidates(state);
    }

    fn hide_candidates(&mut self) {
        self.frontend().hide_candidates();
    }

    fn move_cursor(&mut self, offset: i32) {
        self.frontend().move_cursor(offset);
    }

    fn sync_input_mode(&mut self, mode: InputMode) {
        self.frontend().sync_input_mode(mode);
    }
}

mod dispatch;
mod protocol;

#[cfg(test)]
mod tests;
