//! Wayland V1 协议前端轻量管道适配器 (InputMethodFrontend 实现)
//!
//! # 设计原理
//! - **实现初衷**：将 Wayland V1 协议的具体文字提交、预编辑设定与图层渲染指令从输入法会话状态机中剥离。
//! - **核心优势**：纯净协议管道；与 [`waylan_engine::ImeRuntime`] 解耦，满足借用拆分需求，实现按键决策全权委托。
//! - **代价与局限**：作为生命周期绑定的借用结构体传递给运行时。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::InputMethodFrontend;
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm::WlShm, wl_surface::WlSurface};
use wayland_client::QueueHandle;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use super::WaylanV1Session;
use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::session_common::{commit_candidate_surface, CandidateSurfaceCommitParams};
use crate::surface::CandidateSurfaceManager;

/// Wayland V1 协议前端轻量借用管道上下文
pub struct WaylanV1Frontend<'a> {
    pub active_context: Option<&'a ZwpInputMethodContextV1>,
    pub commit_serial: u32,
    pub surface_manager: &'a mut CandidateSurfaceManager,
    pub active_buffer: &'a mut Option<WlBuffer>,
    pub layer_surface: Option<&'a ZwlrLayerSurfaceV1>,
    pub popup_wl_surface: Option<&'a WlSurface>,
    pub shm: Option<&'a WlShm>,
    pub qhandle: Option<&'a QueueHandle<WaylanV1Session>>,
}

impl<'a> InputMethodFrontend for WaylanV1Frontend<'a> {
    fn commit_text(&mut self, text: &str) {
        if let Some(context) = self.active_context {
            context.commit_string(self.commit_serial, text.to_string());
            context.preedit_cursor(0);
            context.preedit_string(self.commit_serial, String::new(), String::new());
        }
        self.hide_candidates();
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        if let Some(context) = self.active_context {
            context.preedit_cursor(cursor_byte_idx as i32);
            context.preedit_string(self.commit_serial, text.to_string(), String::new());
        }
    }

    fn update_candidates(&mut self, state: &CandidateBoxState) {
        let params = CandidateSurfaceCommitParams {
            shm: self.shm,
            popup_wl_surface: self.popup_wl_surface,
            qhandle: self.qhandle,
            active_buffer: self.active_buffer,
        };
        commit_candidate_surface(self.surface_manager, state, params);

        if let Some(layer_surf) = self.layer_surface {
            let scale = self.surface_manager.scale.max(1) as u32;
            let (phys_w, phys_h) = self.surface_manager.current_size;
            let logical_w = (phys_w / scale).max(1);
            let logical_h = (phys_h / scale).max(1);
            let (screen_w, screen_h) = self.surface_manager.screen_size;
            let (pos_x, pos_y) = self
                .surface_manager
                .cursor_rect
                .compute_popup_position_constrained(logical_w, logical_h, screen_w, screen_h);
            layer_surf.set_size(logical_w, logical_h);
            layer_surf.set_margin(pos_y.max(0), 0, 0, pos_x.max(0));
            if let Some(surf) = self.popup_wl_surface {
                surf.commit();
            }
        }
    }

    fn hide_candidates(&mut self) {
        self.surface_manager
            .hide_wayland_surface(self.popup_wl_surface, self.active_buffer);
    }

    fn move_cursor(&mut self, offset: i32) {
        if let Some(context) = self.active_context {
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
