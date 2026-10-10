use crate::session_v1::{
    is_next_page_key, is_prev_page_key, keycode_to_ascii_digit_char, keycode_to_char,
    keycode_to_digit, keycode_to_punctuation_with_shift, WaylanV1Session, KEY_BACKSPACE,
    KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_SPACE, KEY_TAB, KEY_UP,
};
use log::info;
use waylan_core::InputMode;
use wayland_client::protocol::wl_keyboard::{Event as KeyboardEvent, WlKeyboard};
use wayland_client::{Connection, Dispatch, QueueHandle};

impl Dispatch<WlKeyboard, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: KeyboardEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let Some(context) = state.active_context.clone() else {
            return;
        };

        state.sync_external_state();

        match event {
            KeyboardEvent::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                state.current_serial = serial;
                let is_press = matches!(
                    key_state,
                    wayland_client::WEnum::Value(
                        wayland_client::protocol::wl_keyboard::KeyState::Pressed
                    )
                );

                state.update_modifier_by_key(key, is_press);

                // 1. 优先送入 Shift/Ctrl 防误触状态机
                if let Some(new_mode) = state.shift_sm.on_key_event(key, is_press, state.mode) {
                    state.mode = new_mode;
                    if let Some(ref zh_flag) = state.shared_is_chinese {
                        zh_flag.store(
                            matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    let mode_str = match new_mode {
                        InputMode::Chinese => "中文",
                        InputMode::English => "英文",
                        InputMode::VMode => "V模式",
                    };
                    info!("[微澜] 切换输入模式为：{}", mode_str);

                    // 若正在输入拼音预编辑阶段切换至英文，直接上屏原始英文字母并清空预编辑与候选框
                    if new_mode == InputMode::English && !state.engine.raw_input().is_empty() {
                        let raw = state.engine.raw_input().to_string();
                        state.engine.reset();
                        state.hide_candidate_surface();
                        context.commit_string(serial, raw);
                        context.preedit_string(serial, String::new(), String::new());
                    }
                    return;
                }

                // 只在按下时处理输入，松开阶段默认透传
                if !is_press {
                    context.key(serial, time, key, 0);
                    return;
                }

                // 2. 如果当前处于英文模式，或按下了 Ctrl/Alt/Super 快捷键（如 Ctrl+C/V），直接转发原始按键给应用
                if state.mode == InputMode::English || state.has_ctrl_or_alt() {
                    context.key(serial, time, key, 1);
                    return;
                }

                // 3. 正在组合拼音状态下的专门按键拦截
                if !state.engine.raw_input().is_empty() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    // 3.1 Tab 键：切换/展开当前高亮候选词的二级动态格式卡片
                    if key == KEY_TAB {
                        let sel = state.engine.selected_index;
                        state.engine.toggle_expand(sel);
                        state.refresh_candidate_surface();
                        return;
                    }

                    // 3.2 处于展开二级状态下的导航与选择
                    if state.engine.is_expanded {
                        let sel = state.engine.selected_index;
                        if key == KEY_DOWN {
                            state.engine.next_expanded_item(sel);
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_UP {
                            state.engine.prev_expanded_item(sel);
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_ENTER || key == KEY_SPACE {
                            if let Some((committed, _)) = state
                                .engine
                                .select_expanded_item(sel, state.engine.expanded_selected_index)
                            {
                                state.punctuation_processor.record_commit(&committed);
                                state.hide_candidate_surface();
                                context.commit_string(serial, committed);
                                context.preedit_string(serial, String::new(), String::new());
                                return;
                            }
                        }
                    } else {
                        // 非展开态下，上下方向键在当前页候选词间循环移动高亮胶囊
                        if key == KEY_DOWN {
                            state.engine.next_candidate();
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_UP {
                            state.engine.prev_candidate();
                            state.refresh_candidate_surface();
                            return;
                        }
                    }

                    // 3.3 数字键 1-9（未按 Shift 时）：选择对应位候选词
                    if !state.is_shift_active() {
                        if let Some(digit) = keycode_to_digit(key) {
                            let cand_idx = digit.saturating_sub(1);
                            if let Some((committed_text, remaining_len)) =
                                state.engine.select_candidate(cand_idx, now)
                            {
                                state.punctuation_processor.record_commit(&committed_text);
                                context.commit_string(serial, committed_text);
                                if remaining_len > 0 {
                                    state.refresh_candidate_surface();
                                    context.preedit_string(
                                        serial,
                                        state.engine.formatted_preedit(),
                                        String::new(),
                                    );
                                } else {
                                    state.hide_candidate_surface();
                                    context.preedit_string(serial, String::new(), String::new());
                                }
                                return;
                            }
                        }
                    }

                    // 3.4 空格键：当前高亮候选词上屏
                    if key == KEY_SPACE {
                        let sel = state.engine.selected_index;
                        if let Some((committed_text, remaining_len)) =
                            state.engine.select_candidate(sel, now)
                        {
                            state.punctuation_processor.record_commit(&committed_text);
                            context.commit_string(serial, committed_text);
                            if remaining_len > 0 {
                                state.refresh_candidate_surface();
                                context.preedit_string(
                                    serial,
                                    state.engine.formatted_preedit(),
                                    String::new(),
                                );
                            } else {
                                state.hide_candidate_surface();
                                context.preedit_string(serial, String::new(), String::new());
                            }
                            return;
                        }
                    }

                    // 3.5 回车键（Enter）：直接上屏原始未转换的英文字母
                    if key == KEY_ENTER {
                        let raw = state.engine.raw_input().to_string();
                        state.engine.reset();
                        state.hide_candidate_surface();
                        context.commit_string(serial, raw);
                        context.preedit_string(serial, String::new(), String::new());
                        return;
                    }

                    // 3.6 退格键（Backspace）：删除一个拼文字母
                    if key == KEY_BACKSPACE {
                        state.engine.pop_char();
                        let preedit = state.engine.formatted_preedit();
                        if preedit.is_empty() {
                            state.hide_candidate_surface();
                        } else {
                            state.refresh_candidate_surface();
                        }
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    // 3.7 候选词翻页（根据配置的翻页快捷键模式判定）
                    if !state.is_shift_active() && is_prev_page_key(key, state.page_keys) {
                        state.engine.prev_page();
                        state.refresh_candidate_surface();
                        return;
                    }
                    if !state.is_shift_active() && is_next_page_key(key, state.page_keys) {
                        state.engine.next_page();
                        state.refresh_candidate_surface();
                        return;
                    }

                    // 3.8 预编辑区内光标左移/右移
                    if key == KEY_LEFT {
                        state.engine.move_cursor_left();
                        let preedit = state.engine.formatted_preedit();
                        state.refresh_candidate_surface();
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }
                    if key == KEY_RIGHT {
                        state.engine.move_cursor_right();
                        let preedit = state.engine.formatted_preedit();
                        state.refresh_candidate_surface();
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    if key == KEY_DELETE {
                        state.engine.delete_char_at_cursor();
                        if state.engine.raw_input().is_empty() {
                            state.hide_candidate_surface();
                            context.preedit_cursor(0);
                            context.preedit_string(serial, String::new(), String::new());
                        } else {
                            let preedit = state.engine.formatted_preedit();
                            state.refresh_candidate_surface();
                            context
                                .preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                            context.preedit_string(serial, preedit, String::new());
                        }
                        return;
                    }

                    // 3.9 ESC 键：取消当前输入并清空
                    if key == KEY_ESC {
                        state.engine.reset();
                        state.hide_candidate_surface();
                        context.preedit_cursor(0);
                        context.preedit_string(serial, String::new(), String::new());
                        return;
                    }
                }

                // 4. 输入缓冲区为空时的按键逻辑
                if state.engine.raw_input().is_empty() {
                    // 退格键原样透传
                    if key == KEY_BACKSPACE {
                        context.key(serial, time, key, 1);
                        return;
                    }

                    // 空格键原样透传
                    if key == KEY_SPACE {
                        context.key(serial, time, key, 1);
                        return;
                    }

                    // 中文模式下的智能标点符号拦截（含 Shift 组合标点，如 !?$^_()""<>{}:~）
                    if let Some(punct_char) =
                        keycode_to_punctuation_with_shift(key, state.is_shift_active())
                    {
                        if let Some(actions) =
                            state.punctuation_processor.process_punctuation(punct_char)
                        {
                            for action in actions {
                                match action {
                                    waylan_core::TextAction::Commit(text) => {
                                        context.commit_string(serial, text);
                                    }
                                    waylan_core::TextAction::MoveCursor(offset) => {
                                        for _ in 0..offset.unsigned_abs() {
                                            let dir_key =
                                                if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
                                            context.key(serial, time, dir_key, 1);
                                            context.key(serial, time, dir_key, 0);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            return;
                        }
                    }

                    // 若为未按 Shift 的纯数字键输入，记录数字上下文以便后续触发 3.14 / 12:00 半角标点保护
                    if !state.is_shift_active() {
                        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
                            state
                                .punctuation_processor
                                .record_commit(&digit_ch.to_string());
                            context.key(serial, time, key, 1);
                            return;
                        }
                    }
                }

                // 5. 字母键输入（A-Z）：进入拼音算法管线
                if let Some(ch) = keycode_to_char(key) {
                    state.engine.push_char(ch);
                    let preedit = state.engine.formatted_preedit();
                    state.refresh_candidate_surface();
                    context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                    context.preedit_string(serial, preedit, String::new());
                    return;
                }

                // 其他未拦截按键，原样透传给应用
                context.key(serial, time, key, 1);
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
