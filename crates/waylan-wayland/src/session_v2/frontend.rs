//! Wayland V2 协议前端轻量借用管道上下文 (InputMethodFrontend 实现)
//!
//! # 设计原理
//! - **实现初衷**：将 Wayland V2 协议的具体文字提交、预编辑设定与图层渲染指令从输入法会话状态机中剥离。
//! - **核心优势**：直接组合借用 `WaylandV2ProtocolContext` 与 `WaylandV2SurfaceContext`，
//!   彻底消除拆包借用元组与对 `runtime` 的伪依赖。
//! - **代价与局限**：生命周期受限于外部借用范围。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::InputMethodFrontend;
use wayland_client::QueueHandle;

use super::WaylanV2Session;
use super::context::{WaylandV2ProtocolContext, WaylandV2SurfaceContext};
use crate::session_common::{CandidateSurfaceCommitParams, commit_candidate_surface};

/// Wayland V2 协议前端轻量借用管道上下文
pub struct WaylanV2Frontend<'a> {
    pub protocol: &'a WaylandV2ProtocolContext,
    pub surface: &'a mut WaylandV2SurfaceContext,
    pub qhandle: Option<&'a QueueHandle<WaylanV2Session>>,
}

impl<'a> WaylanV2Frontend<'a> {
    /// 构造借用管道，解耦协议状态与表面渲染
    #[inline]
    pub fn new(
        protocol: &'a WaylandV2ProtocolContext,
        surface: &'a mut WaylandV2SurfaceContext,
        qhandle: Option<&'a QueueHandle<WaylanV2Session>>,
    ) -> Self {
        Self {
            protocol,
            surface,
            qhandle,
        }
    }
}

impl<'a> InputMethodFrontend for WaylanV2Frontend<'a> {
    fn commit_text(&mut self, text: &str) {
        if let Some(ref im) = self.protocol.input_method {
            im.commit_string(text.to_string());
            im.set_preedit_string(String::new(), 0, 0);
            im.commit(self.protocol.done_serial);
        }
        self.hide_candidates();
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        if let Some(ref im) = self.protocol.input_method {
            let cursor = cursor_byte_idx as i32;
            im.set_preedit_string(text.to_string(), cursor, cursor);
            im.commit(self.protocol.done_serial);
        }
    }

    fn delete_surrounding_text(&mut self, before_bytes: u32, after_bytes: u32) {
        if let Some(ref im) = self.protocol.input_method {
            im.delete_surrounding_text(before_bytes, after_bytes);
            im.commit(self.protocol.done_serial);
        }
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        let params = CandidateSurfaceCommitParams {
            shm: self.protocol.shm.as_ref(),
            popup_wl_surface: self.surface.popup_wl_surface.as_ref(),
            qhandle: self.qhandle,
            active_buffer: &mut self.surface.active_buffer,
        };
        commit_candidate_surface(&mut self.surface.surface_manager, state, params);
    }

    fn hide_candidates(&mut self) {
        self.surface.surface_manager.hide_wayland_surface(
            self.surface.popup_wl_surface.as_ref(),
            &mut self.surface.active_buffer,
        );
    }

    fn move_cursor(&mut self, offset: i32) {
        let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
        if !self.protocol.has_virtual_keymap {
            return;
        }
        if let Some(ref vk) = self.protocol.virtual_keyboard {
            for _ in 0..offset.unsigned_abs() {
                vk.key(0, dir_key, 1);
                vk.key(0, dir_key, 0);
            }
        }
    }

    fn sync_input_mode(&mut self, _mode: InputMode) {
        // 模式切换后的原子标志与托盘唤醒已由 ImeRuntime 统一托管
    }
}
