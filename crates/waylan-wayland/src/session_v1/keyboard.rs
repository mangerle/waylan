//! V1 协议键盘事件分发与协议交互实现

use std::fs::File;
use std::io::Read;

use log::debug;
use wayland_client::protocol::wl_keyboard::{Event as KeyboardEvent, KeyState, WlKeyboard};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::session_v1::WaylanV1Session;

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

/// 处理修饰键状态变更同步
fn handle_v1_modifiers(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    serial: u32,
    mods_depressed: u32,
    mods_latched: u32,
    mods_locked: u32,
    group: u32,
) {
    let prev_caps = state.runtime.core.is_caps_locked();
    state
        .runtime
        .core
        .set_modifiers(mods_depressed, mods_locked);
    let curr_caps = state.runtime.core.is_caps_locked();
    if prev_caps != curr_caps {
        state.runtime.update_caps_lock_state(curr_caps);
    }
    context.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
}

/// 统一分发 V1 键盘按键事件
fn handle_v1_key_event(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    serial: u32,
    time: u32,
    key: u32,
    is_press: bool,
) {
    // 1. 外部快捷键拦截器优先捕获
    if let Some(mut filter) = state.key_filter.take() {
        let filter_outcome = filter(key, is_press, state.runtime.core.mods_depressed());
        state.key_filter = Some(filter);
        match filter_outcome {
            crate::session_common::KeyFilterResult::Consumed => {
                if is_press {
                    state.runtime.core.consumed_keys.record_press(key, true);
                } else {
                    state.runtime.core.consumed_keys.take_release(key);
                }
                return;
            }
            crate::session_common::KeyFilterResult::Commit(text) => {
                context.commit_string(state.commit_serial, text.clone());
                context.preedit_string(state.commit_serial, String::new(), String::new());
                state
                    .runtime
                    .core
                    .punctuation_processor
                    .record_commit(&text);
                state.hide_candidate_surface();
                if is_press {
                    state.runtime.core.consumed_keys.record_press(key, true);
                } else {
                    state.runtime.core.consumed_keys.take_release(key);
                }
                return;
            }
            crate::session_common::KeyFilterResult::PassThrough => {}
        }
    }

    // 2. 全权委托统一运行时 ImeRuntime 处理（包含语音分发、核心决策与前端 UI 刷新）
    let (runtime, mut frontend) = state.split_runtime_and_frontend();
    let (consumed, _commit) = runtime.process_key_event(
        &mut frontend,
        key,
        is_press,
        false, // V1 协议不具备 password 状态标记
        None,  // V1 协议不具备 surrounding text 协议扩展
        false, // inline_preedit 在 V1 下为 false
    );

    // 3. 若未被输入法消费，通过协议转发给上层应用
    if !consumed {
        context.key(serial, time, key, if is_press { 1 } else { 0 });
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

        let Some(context) = state.active_context.clone() else {
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
                state.current_serial = serial;
                let is_press = matches!(key_state, WEnum::Value(KeyState::Pressed));
                handle_v1_key_event(state, &context, serial, time, key, is_press);
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
                    serial,
                    mods_depressed,
                    mods_latched,
                    mods_locked,
                    group,
                );
            }
            _ => {}
        }
    }
}
