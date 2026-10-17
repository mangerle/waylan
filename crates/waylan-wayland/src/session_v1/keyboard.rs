//! V1 协议键盘事件分发与协议交互实现

use std::fs::File;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_algo::{KeyProcessContext, KeyProcessResult};
use waylan_core::input::*;
use waylan_core::InputMode;
use waylan_voice::VoiceKeyAction;
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
    let prev_caps = state.core.is_caps_locked();
    state.core.set_modifiers(mods_depressed, mods_locked);
    let curr_caps = state.core.is_caps_locked();
    if prev_caps != curr_caps {
        if let Some(ref caps_flag) = state.shared_is_caps_locked {
            caps_flag.store(curr_caps, Ordering::Release);
        }
        if let Some(ref notify_tx) = state.mode_notify_tx {
            let _ = notify_tx.try_send(());
        }
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
    // 1. 统一语音按键状态机优先分发（与 IBus 体验对齐）
    if let Some(ref mut vm) = state.voice_manager {
        match vm.handle_key_event(key, is_press, state.core.mods_depressed()) {
            VoiceKeyAction::Consumed => {
                if is_press {
                    state.core.consumed_keys.record_press(key, true);
                } else {
                    state.core.consumed_keys.take_release(key);
                }
                state.refresh_candidate_surface();
                return;
            }
            VoiceKeyAction::Commit(text) => {
                context.commit_string(state.commit_serial, text.clone());
                context.preedit_string(state.commit_serial, String::new(), String::new());
                state.core.punctuation_processor.record_commit(&text);
                state.hide_candidate_surface();
                if is_press {
                    state.core.consumed_keys.record_press(key, true);
                } else {
                    state.core.consumed_keys.take_release(key);
                }
                return;
            }
            VoiceKeyAction::PassThrough => {}
        }
    }

    // 2. 外部快捷键拦截器优先捕获
    if let Some(mut filter) = state.key_filter.take() {
        let filter_outcome = filter(key, is_press, state.core.mods_depressed());
        state.key_filter = Some(filter);
        match filter_outcome {
            crate::session_common::KeyFilterResult::Consumed => {
                if is_press {
                    state.core.consumed_keys.record_press(key, true);
                } else {
                    state.core.consumed_keys.take_release(key);
                }
                return;
            }
            crate::session_common::KeyFilterResult::Commit(text) => {
                context.commit_string(state.commit_serial, text.clone());
                context.preedit_string(state.commit_serial, String::new(), String::new());
                state.core.punctuation_processor.record_commit(&text);
                state.hide_candidate_surface();
                if is_press {
                    state.core.consumed_keys.record_press(key, true);
                } else {
                    state.core.consumed_keys.take_release(key);
                }
                return;
            }
            crate::session_common::KeyFilterResult::PassThrough => {}
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 3. 调用统一核心会话决策流水线
    let result = state.core.process_key(
        KeyProcessContext {
            keycode: key,
            is_press,
            is_password_or_locked: false,
            surrounding_prefix: None,
        },
        now,
    );

    // 4. 将决策结果映射为 V1 协议操作
    match result {
        KeyProcessResult::PassThrough => {
            context.key(serial, time, key, if is_press { 1 } else { 0 });
        }
        KeyProcessResult::Handled { update_ui } => {
            if update_ui {
                let preedit = state.core.engine.formatted_preedit();
                let cursor = state.core.engine.formatted_cursor_byte_offset() as i32;
                state.refresh_candidate_surface();
                context.preedit_cursor(cursor);
                context.preedit_string(state.commit_serial, preedit, String::new());
            }
        }
        KeyProcessResult::ExpandGrid => {
            let preedit = state.core.engine.formatted_preedit();
            let cursor = state.core.engine.formatted_cursor_byte_offset() as i32;
            state.refresh_candidate_surface();
            context.preedit_cursor(cursor);
            context.preedit_string(state.commit_serial, preedit, String::new());
        }
        KeyProcessResult::Commit(text) => {
            state.hide_candidate_surface();
            context.commit_string(state.commit_serial, text);
            context.preedit_cursor(0);
            context.preedit_string(state.commit_serial, String::new(), String::new());
        }
        KeyProcessResult::CommitAndPassThrough(text) => {
            state.hide_candidate_surface();
            context.commit_string(state.commit_serial, text);
            context.preedit_cursor(0);
            context.preedit_string(state.commit_serial, String::new(), String::new());
            context.key(serial, time, key, if is_press { 1 } else { 0 });
        }
        KeyProcessResult::CommitPunctuation {
            text,
            cursor_offset,
        } => {
            context.commit_string(state.commit_serial, text);
            let dir_key = if cursor_offset < 0 {
                KEY_LEFT
            } else {
                KEY_RIGHT
            };
            for _ in 0..cursor_offset.unsigned_abs() {
                context.key(serial, time, dir_key, 1);
                context.key(serial, time, dir_key, 0);
            }
        }
        KeyProcessResult::CommitPartial {
            committed,
            remaining_preedit,
        } => {
            let cursor = state.core.engine.formatted_cursor_byte_offset() as i32;
            context.commit_string(state.commit_serial, committed);
            state.refresh_candidate_surface();
            context.preedit_cursor(cursor);
            context.preedit_string(state.commit_serial, remaining_preedit, String::new());
        }
        KeyProcessResult::Reset => {
            state.hide_candidate_surface();
            context.preedit_cursor(0);
            context.preedit_string(state.commit_serial, String::new(), String::new());
        }
        KeyProcessResult::ShortcutToggled(_toggle) => {
            if let Some(ref notify_tx) = state.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            state.refresh_candidate_surface();
        }
        KeyProcessResult::ModeChanged {
            new_mode,
            committed,
            was_consumed,
        } => {
            if let Some(ref zh_flag) = state.shared_is_chinese {
                zh_flag.store(
                    matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                    Ordering::Release,
                );
            }
            if let Some(ref notify_tx) = state.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            let mode_str = match new_mode {
                InputMode::Chinese => "中文",
                InputMode::English => "英文",
                InputMode::VMode => "V模式",
            };
            debug!("[微澜] 切换输入模式为：{}", mode_str);

            if let Some(raw) = committed {
                state.hide_candidate_surface();
                context.commit_string(state.commit_serial, raw);
                context.preedit_string(state.commit_serial, String::new(), String::new());
            }
            if !was_consumed {
                context.key(serial, time, key, 0);
            }
        }
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
