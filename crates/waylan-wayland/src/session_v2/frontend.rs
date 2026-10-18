//! Wayland V2 协议前端轻量管道适配器 (InputMethodFrontend 实现)
//!
//! # 设计原理
//! - **实现初衷**：将 Wayland V2 协议的具体文字提交、预编辑设定与图层渲染指令从输入法会话状态机中剥离。
//! - **核心优势**：纯净协议管道；与 [`waylan_engine::ImeRuntime`] 解耦，满足借用拆分需求，实现按键决策全权委托。
//! - **代价与局限**：作为生命周期绑定的借用结构体传递给运行时。

use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_engine::InputMethodFrontend;
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm::WlShm, wl_surface::WlSurface};
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use super::WaylanV2Session;
use crate::session_common::{
    commit_candidate_surface, hide_candidate_surface_buffer, CandidateSurfaceCommitParams,
};
use crate::surface::CandidateSurfaceManager;

/// Wayland V2 协议前端轻量借用管道上下文
pub struct WaylanV2Frontend<'a> {
    pub input_method: Option<&'a ZwpInputMethodV2>,
    pub surface_manager: &'a mut CandidateSurfaceManager,
    pub popup_wl_surface: Option<&'a WlSurface>,
    pub active_buffer: &'a mut Option<WlBuffer>,
    pub shm: Option<&'a WlShm>,
    pub qhandle: Option<&'a QueueHandle<WaylanV2Session>>,
    pub virtual_keyboard: Option<&'a ZwpVirtualKeyboardV1>,
    pub has_virtual_keymap: bool,
    pub done_serial: u32,
}

impl<'a> InputMethodFrontend for WaylanV2Frontend<'a> {
    fn commit_text(&mut self, text: &str) {
        if let Some(im) = self.input_method {
            im.commit_string(text.to_string());
            im.set_preedit_string(String::new(), 0, 0);
            im.commit(self.done_serial);
        }
        self.hide_candidates();
    }

    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize) {
        if let Some(im) = self.input_method {
            let cursor = cursor_byte_idx as i32;
            im.set_preedit_string(text.to_string(), cursor, cursor);
            im.commit(self.done_serial);
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
    }

    fn hide_candidates(&mut self) {
        hide_candidate_surface_buffer(
            self.surface_manager,
            self.popup_wl_surface,
            self.active_buffer,
        );
    }

    fn move_cursor(&mut self, offset: i32) {
        let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
        if !self.has_virtual_keymap {
            return;
        }
        if let Some(vk) = self.virtual_keyboard {
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
