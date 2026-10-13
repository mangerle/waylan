use std::fs::File;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_core::{InputMode, TextAction};
use wayland_client::protocol::wl_keyboard::{Event as KeyboardEvent, KeyState, WlKeyboard};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::session_v1::{
    handle_composition_key_with_params, keycode_to_ascii_digit_char,
    keycode_to_punctuation_with_shift, CompositionKeyParams, KeyProcessAction, WaylanV1Session,
    KEY_BACKSPACE, KEY_LEFT, KEY_RIGHT, KEY_SPACE,
};

impl Dispatch<WlKeyboard, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: KeyboardEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let KeyboardEvent::Keymap {
            format: _,
            fd,
            size,
        } = event
        {
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

                // 1. 优先送入 Shift/Ctrl 防误触状态机
                let toggled_mode = state.shift_sm.on_key_event(key, is_press, state.mode);

                // 松开阶段：严格按照按下阶段是否被输入法消费决定是否透传释放事件，保证 Press/Release 对称
                if !is_press {
                    let was_press_consumed = state.consumed_keys.take_release(key);
                    if let Some(new_mode) = toggled_mode {
                        state.mode = new_mode;
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

                        // 若正在输入拼音预编辑阶段切换至英文，直接上屏原始英文字母并清空预编辑与候选框
                        if new_mode == InputMode::English && !state.engine.raw_input().is_empty() {
                            let raw = state.engine.raw_input().to_string();
                            state.engine.reset();
                            state.hide_candidate_surface();
                            context.commit_string(state.commit_serial, raw);
                            context.preedit_string(
                                state.commit_serial,
                                String::new(),
                                String::new(),
                            );
                        }
                    }
                    if !was_press_consumed {
                        context.key(serial, time, key, 0);
                    }
                    return;
                }

                let consumed = handle_v1_key_press(state, &context, time, key);
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
                state.mods_depressed = mods_depressed;
                // 原样透传修饰键状态给当前文本应用
                context.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
    }
}

/// 处理 V1 协议下的按键按下事件，返回该按键是否被输入法内部消费（true=已消费，false=需原样透传给客户端）
fn handle_v1_key_press(
    state: &mut WaylanV1Session,
    context: &ZwpInputMethodContextV1,
    time: u32,
    key: u32,
) -> bool {
    let serial = state.current_serial;
    // 2. 如果当前处于英文模式，或按下了 Ctrl/Alt/Super 快捷键（如 Ctrl+C/V），直接转发原始按键给应用
    if state.mode == InputMode::English || state.has_ctrl_or_alt() {
        return false;
    }

    // 3. 正在组合拼音状态下的专门按键拦截
    if !state.engine.raw_input().is_empty() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let params = CompositionKeyParams {
            key,
            shift_active: state.is_shift_active(),
            page_keys: state.page_keys,
            now,
        };
        if let Some(action) = handle_composition_key_with_params(
            &mut state.engine,
            &mut state.punctuation_processor,
            params,
        ) {
            match action {
                KeyProcessAction::PassThrough => {}
                KeyProcessAction::Handled | KeyProcessAction::ExpandGrid => {
                    let preedit = state.engine.formatted_preedit();
                    let cursor = state.engine.formatted_cursor_byte_offset() as i32;
                    state.refresh_candidate_surface();
                    context.preedit_cursor(cursor);
                    context.preedit_string(state.commit_serial, preedit, String::new());
                    return true;
                }
                KeyProcessAction::Commit(text) => {
                    state.hide_candidate_surface();
                    context.commit_string(state.commit_serial, text);
                    context.preedit_cursor(0);
                    context.preedit_string(state.commit_serial, String::new(), String::new());
                    return true;
                }
                KeyProcessAction::CommitPartial {
                    committed,
                    remaining_preedit,
                } => {
                    let cursor = state.engine.formatted_cursor_byte_offset() as i32;
                    context.commit_string(state.commit_serial, committed);
                    state.refresh_candidate_surface();
                    context.preedit_cursor(cursor);
                    context.preedit_string(state.commit_serial, remaining_preedit, String::new());
                    return true;
                }
                KeyProcessAction::Reset => {
                    state.hide_candidate_surface();
                    context.preedit_cursor(0);
                    context.preedit_string(state.commit_serial, String::new(), String::new());
                    return true;
                }
            }
        }
    }

    // 4. 输入缓冲区为空时的按键逻辑
    if state.engine.raw_input().is_empty() {
        // 退格键与空格键原样透传
        if key == KEY_BACKSPACE || key == KEY_SPACE {
            return false;
        }

        // 中文模式下的智能标点符号拦截（含 Shift 组合标点，如 !?$^_()""<>{}:~）
        if let Some(punct_char) = keycode_to_punctuation_with_shift(key, state.is_shift_active()) {
            if let Some(actions) = state.punctuation_processor.process_punctuation(punct_char) {
                for action in actions {
                    match action {
                        TextAction::Commit(text) => {
                            context.commit_string(state.commit_serial, text);
                        }
                        TextAction::MoveCursor(offset) => {
                            for _ in 0..offset.unsigned_abs() {
                                let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
                                context.key(serial, time, dir_key, 1);
                                context.key(serial, time, dir_key, 0);
                            }
                        }
                        _ => {}
                    }
                }
                return true;
            }
        }

        // 若为未按 Shift 的纯数字键输入，记录数字上下文以便后续触发 3.14 / 12:00 半角标点保护
        if !state.is_shift_active() {
            if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
                state
                    .punctuation_processor
                    .record_commit(&digit_ch.to_string());
                return false;
            }
        }
    }

    // 5. 字母键输入（A-Z）：进入拼音算法管线（支持动态键盘布局）
    if let Some(ch) = state.keymap_layout.get_char(key) {
        state.engine.push_char(ch);
        let preedit = state.engine.formatted_preedit();
        let cursor = state.engine.formatted_cursor_byte_offset() as i32;
        state.refresh_candidate_surface();
        context.preedit_cursor(cursor);
        context.preedit_string(state.commit_serial, preedit, String::new());
        return true;
    }

    // 其他未拦截按键，原样透传给应用
    false
}
