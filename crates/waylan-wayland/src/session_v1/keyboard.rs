use std::fs::File;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_algo::ImeAction;
use waylan_core::input::*;
use waylan_core::InputMode;
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
            .keymap_layout
            .update_from_xkb_text(text.trim_end_matches('\0'));
        debug!("[Wayland v1 键盘抓取] 成功从 XKB Keymap 动态同步键盘字符布局映射。");
    }
}

/// 处理 CapsLock 按键事件并在有拼音未上屏时提交英文字符
fn handle_v1_capslock(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    serial: u32,
    time: u32,
    key: u32,
    is_press: bool,
) {
    let (was_consumed, committed) = state.core.handle_capslock(is_press);
    if let Some(raw) = committed {
        state.hide_candidate_surface();
        context.commit_string(state.commit_serial, raw);
        context.preedit_string(state.commit_serial, String::new(), String::new());
    }
    if is_press {
        context.key(serial, time, key, 1);
    } else if !was_consumed {
        context.key(serial, time, key, 0);
    }
}

/// 处理按键释放阶段的 Shift/Ctrl 状态机模式切换
fn handle_v1_key_release(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    serial: u32,
    time: u32,
    key: u32,
) {
    let (was_press_consumed, toggled_mode, committed) = state.core.handle_key_release(key);
    if let Some(new_mode) = toggled_mode {
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
    }
    if !was_press_consumed {
        context.key(serial, time, key, 0);
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
    let prev_caps = state.is_caps_locked();
    state.core.set_modifiers(mods_depressed, mods_locked);
    let curr_caps = state.is_caps_locked();
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

/// 处理 V1 协议下的按键按下事件，返回该按键是否被输入法内部消费（true=已消费，false=需原样透传给客户端）
fn handle_v1_key_press(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    serial: u32,
    time: u32,
    key: u32,
) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let action = state.core.handle_key_press(key, now, None);
    match action {
        ImeAction::PassThrough => false,
        ImeAction::Handled | ImeAction::ExpandGrid => {
            let preedit = state.engine.formatted_preedit();
            let cursor = state.engine.formatted_cursor_byte_offset() as i32;
            state.refresh_candidate_surface();
            context.preedit_cursor(cursor);
            context.preedit_string(state.commit_serial, preedit, String::new());
            true
        }
        ImeAction::Commit(text) => {
            state.hide_candidate_surface();
            context.commit_string(state.commit_serial, text);
            context.preedit_cursor(0);
            context.preedit_string(state.commit_serial, String::new(), String::new());
            true
        }
        ImeAction::CommitPunctuation {
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
            true
        }
        ImeAction::CommitPartial {
            committed,
            remaining_preedit,
        } => {
            let cursor = state.engine.formatted_cursor_byte_offset() as i32;
            context.commit_string(state.commit_serial, committed);
            state.refresh_candidate_surface();
            context.preedit_cursor(cursor);
            context.preedit_string(state.commit_serial, remaining_preedit, String::new());
            true
        }
        ImeAction::Reset => {
            state.hide_candidate_surface();
            context.preedit_cursor(0);
            context.preedit_string(state.commit_serial, String::new(), String::new());
            true
        }
        ImeAction::ShortcutToggled(_toggle) => {
            if let Some(ref notify_tx) = state.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            state.refresh_candidate_surface();
            true
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
                state.update_modifier_by_key(key, is_press);

                // 外部快捷键拦截器（如语音听写）优先捕获
                if let Some(mut filter) = state.key_filter.take() {
                    let filter_outcome = filter(key, is_press, state.core.mods_depressed());
                    state.key_filter = Some(filter);
                    match filter_outcome {
                        crate::session_common::KeyFilterResult::Consumed => {
                            if is_press {
                                state.consumed_keys.record_press(key, true);
                            } else {
                                state.consumed_keys.take_release(key);
                            }
                            return;
                        }
                        crate::session_common::KeyFilterResult::Commit(text) => {
                            context.commit_string(state.commit_serial, text.clone());
                            context.preedit_string(
                                state.commit_serial,
                                String::new(),
                                String::new(),
                            );
                            state.punctuation_processor.record_commit(&text);
                            state.hide_candidate_surface();
                            if is_press {
                                state.consumed_keys.record_press(key, true);
                            } else {
                                state.consumed_keys.take_release(key);
                            }
                            return;
                        }
                        crate::session_common::KeyFilterResult::PassThrough => {}
                    }
                }

                if key == KEY_CAPSLOCK {
                    handle_v1_capslock(state, &context, serial, time, key, is_press);
                    return;
                }

                if !is_press {
                    handle_v1_key_release(state, &context, serial, time, key);
                    return;
                }

                let consumed = handle_v1_key_press(state, &context, serial, time, key);
                state.consumed_keys.record_press(key, consumed);
                if !consumed {
                    context.key(serial, time, key, 1);
                }
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
