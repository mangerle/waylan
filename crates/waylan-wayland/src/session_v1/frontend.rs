//! Wayland V1 协议前端轻量管道适配器 (InputMethodFrontend 实现)
//!
//! # 设计原理
//! - **实现初衷**：将 Wayland V1 协议的具体文字提交、预编辑设定与图层渲染指令从输入法会话状态机中剥离。
//! - **核心优势**：纯净协议管道；直接借用协议上下文与表面上下文，消除借用扭曲，实现原生不相交安全借用。
//! - **代价与局限**：生命周期受限于借用范围。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::InputMethodFrontend;
use wayland_client::QueueHandle;

use super::WaylanV1Session;
use super::context::{WaylandV1ProtocolContext, WaylandV1SurfaceContext};

/// Wayland V1 协议前端轻量借用管道上下文
pub struct WaylanV1Frontend<'a> {
    pub protocol: &'a WaylandV1ProtocolContext,
    pub surface: &'a mut WaylandV1SurfaceContext,
    pub qhandle: Option<&'a QueueHandle<WaylanV1Session>>,
}

impl<'a> WaylanV1Frontend<'a> {
    /// 构造 V1 借用管道实例
    #[inline]
    pub fn new(
        protocol: &'a WaylandV1ProtocolContext,
        surface: &'a mut WaylandV1SurfaceContext,
        qhandle: Option<&'a QueueHandle<WaylanV1Session>>,
    ) -> Self {
        Self {
            protocol,
            surface,
            qhandle,
        }
    }
}

impl<'a> InputMethodFrontend for WaylanV1Frontend<'a> {
    fn commit_text(&mut self, text: &str) {
        if let Some(ref context) = self.protocol.active_context {
            context.commit_string(self.protocol.commit_serial, text.to_string());
            context.preedit_cursor(0);
            context.preedit_string(self.protocol.commit_serial, String::new(), String::new());
        }
        self.hide_candidates();
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        if let Some(ref context) = self.protocol.active_context {
            context.preedit_cursor(cursor_byte_idx as i32);
            context.preedit_string(self.protocol.commit_serial, text.to_string(), String::new());
        }
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        let params = crate::session_common::V1LayerCommitParams {
            shm: self.surface.shm.as_ref(),
            popup_wl_surface: self.surface.popup_wl_surface.as_ref(),
            layer_surface: self.protocol.layer_surface.as_ref(),
            qhandle: self.qhandle,
            active_buffer: &mut self.surface.active_buffer,
        };
        crate::session_common::commit_v1_layer_surface(
            &mut self.surface.surface_manager,
            state,
            params,
        );
    }

    fn hide_candidates(&mut self) {
        self.surface.surface_manager.hide_wayland_surface(
            self.surface.popup_wl_surface.as_ref(),
            &mut self.surface.active_buffer,
        );
    }

    fn move_cursor(&mut self, offset: i32) {
        if let Some(ref context) = self.protocol.active_context {
            let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
            for _ in 0..offset.unsigned_abs() {
                context.key(0, 0, dir_key, 1);
                context.key(0, 0, dir_key, 0);
            }
        }
    }

    fn sync_input_mode(&mut self, _mode: InputMode) {
        // 模式切换后的原子标志与托盘唤醒已由 ImeRuntime 统一托管
    }
}
