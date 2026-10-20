//! V1 协议键盘事件分发与协议交互实现

use std::fs::File;
use std::io::Read;

use log::debug;
use wayland_client::protocol::wl_keyboard::{Event as KeyboardEvent, KeyState, WlKeyboard};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::session_common::WaylandSessionLifecycle;
use crate::session_v1::{WaylanV1Frontend, WaylanV1Session};

/// 处理 XKB 键盘字符布局映射同步
fn handle_v1_keymap(state: &mut WaylanV1Session, fd: std::os::fd::OwnedFd, size: u32) {
    let mut file = File::from(fd);
    let read_len = (size as usize).min(1024 * 1024);
    let mut buf = vec![0u8; read_len];
    if file.read_exact(&mut buf).is_ok() {
        let text = String::from_utf8_lossy(&buf);
        state
            .runtime
            .core
            .keymap_layout
            .update_from_xkb_text(text.trim_end_matches('\0'));
        debug!("[Wayland v1 键盘抓取] 成功从 XKB Keymap 动态同步键盘字符布局映射。");
    }
}

/// V1 键盘按键事件参数对象
struct V1KeyEventParams {
    serial: u32,
    time: u32,
    key: u32,
    is_press: bool,
}

/// V1 键盘修饰键变更参数对象
struct V1ModifiersParams {
    serial: u32,
    mods_depressed: u32,
    mods_latched: u32,
    mods_locked: u32,
    group: u32,
}

/// 处理修饰键状态变更同步
fn handle_v1_modifiers(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    params: V1ModifiersParams,
) {
    let prev_caps = state.runtime.core.is_caps_locked();
    state
        .runtime
        .core
        .set_modifiers(params.mods_depressed, params.mods_locked);
    let curr_caps = state.runtime.core.is_caps_locked();
    if prev_caps != curr_caps {
        state.runtime.update_caps_lock_state(curr_caps);
    }
    context.modifiers(
        params.serial,
        params.mods_depressed,
        params.mods_latched,
        params.mods_locked,
        params.group,
    );
}

/// 统一分发 V1 键盘按键事件
fn handle_v1_key_event(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    params: V1KeyEventParams,
) {
    // 1. 原生借用：前端借用 protocol + surface，runtime 单独借用，借用检查器天然支持不相交借用
    let mut frontend =
        WaylanV1Frontend::new(&state.protocol, &mut state.surface, state.qhandle.as_ref());
    let (consumed, _commit) = state.runtime.process_key_event(
        &mut frontend,
        waylan_engine::ProcessKeyEventParams {
            key: params.key,
            is_press: params.is_press,
            is_password_or_locked: false, // V1 协议不具备 password 状态标记
            surrounding_text: None,       // V1 协议不具备 surrounding text 协议扩展
            inline_preedit: false,        // inline_preedit 在 V1 下为 false
        },
    );

    // 2. 若未被输入法消费，通过协议转发给上层应用
    if !consumed {
        context.key(
            params.serial,
            params.time,
            params.key,
            if params.is_press { 1 } else { 0 },
        );
    }
}

impl Dispatch<WlKeyboard, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: KeyboardEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let KeyboardEvent::Keymap { fd, size, .. } = event {
            handle_v1_keymap(state, fd, size);
            return;
        }

        let Some(context) = state.protocol.active_context.clone() else {
            return;
        };

        state.sync_atomic_state();

        match event {
            KeyboardEvent::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                state.protocol.current_serial = serial;
                let is_press = matches!(key_state, WEnum::Value(KeyState::Pressed));
                handle_v1_key_event(
                    state,
                    &context,
                    V1KeyEventParams {
                        serial,
                        time,
                        key,
                        is_press,
                    },
                );
            }
            KeyboardEvent::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                handle_v1_modifiers(
                    state,
                    &context,
                    V1ModifiersParams {
                        serial,
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    },
                );
            }
            _ => {}
        }
    }
}
