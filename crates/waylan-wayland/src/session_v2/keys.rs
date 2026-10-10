use super::WaylanV2Session;
use crate::input_core::*;
use waylan_core::TextAction;

/// 处理英文模式下的按键输入
pub fn process_english_mode_key(session: &mut WaylanV2Session, key: u32) -> (bool, Option<String>) {
    if key == KEY_BACKSPACE {
        session.delete_backward();
        return (true, None);
    }
    if key == KEY_DELETE {
        session.delete_forward();
        return (true, None);
    }
    if key == KEY_ENTER {
        session.commit_text("\n");
        return (true, Some("\n".to_string()));
    }
    if key == KEY_TAB {
        session.commit_text("\t");
        return (true, Some("\t".to_string()));
    }
    if let Some(ch) = keycode_to_char(key) {
        let out = if session.is_shift_active() {
            ch.to_ascii_uppercase().to_string()
        } else {
            ch.to_string()
        };
        session.punctuation_processor.record_commit(&out);
        session.commit_text(&out);
        return (true, Some(out));
    }
    if !session.is_shift_active() {
        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
            let out = digit_ch.to_string();
            session.punctuation_processor.record_commit(&out);
            session.commit_text(&out);
            return (true, Some(out));
        }
    }
    if let Some(punct_ch) = keycode_to_punctuation_with_shift(key, session.is_shift_active()) {
        let out = punct_ch.to_string();
        session.punctuation_processor.record_commit(&out);
        session.commit_text(&out);
        return (true, Some(out));
    }
    if key == KEY_SPACE {
        session.commit_text(" ");
        return (true, Some(" ".to_string()));
    }
    (false, None)
}

/// 处理处于拼音组词状态下的键盘拦截
pub fn process_composition_key(
    session: &mut WaylanV2Session,
    key: u32,
    now: u64,
) -> Option<(bool, Option<String>)> {
    if key == KEY_TAB {
        let sel = session.engine.selected_index;
        session.engine.toggle_expand(sel);
        session.refresh_candidate_surface();
        return Some((true, None));
    }

    if session.engine.is_expanded {
        let sel = session.engine.selected_index;
        if key == KEY_DOWN {
            session.engine.next_expanded_item(sel);
            session.refresh_candidate_surface();
            return Some((true, None));
        }
        if key == KEY_UP {
            session.engine.prev_expanded_item(sel);
            session.refresh_candidate_surface();
            return Some((true, None));
        }
        if key == KEY_ENTER || key == KEY_SPACE {
            if let Some((committed, _)) = session
                .engine
                .select_expanded_item(sel, session.engine.expanded_selected_index)
            {
                session.punctuation_processor.record_commit(&committed);
                session.commit_text(&committed);
                return Some((true, Some(committed)));
            }
        }
    } else {
        if key == KEY_DOWN {
            session.engine.next_candidate();
            session.refresh_candidate_surface();
            return Some((true, None));
        }
        if key == KEY_UP {
            session.engine.prev_candidate();
            session.refresh_candidate_surface();
            return Some((true, None));
        }
    }

    if !session.is_shift_active() {
        if let Some(digit) = keycode_to_digit(key) {
            let cand_idx = digit.saturating_sub(1);
            if let Some((committed_text, remaining_len)) =
                session.engine.select_candidate(cand_idx, now)
            {
                session.punctuation_processor.record_commit(&committed_text);
                if remaining_len > 0 {
                    let preedit = session.engine.formatted_preedit();
                    session.commit_partial(&committed_text, &preedit);
                } else {
                    session.commit_text(&committed_text);
                }
                return Some((true, Some(committed_text)));
            }
        }
    }

    if key == KEY_SPACE {
        let sel = session.engine.selected_index;
        if let Some((committed_text, remaining_len)) = session.engine.select_candidate(sel, now) {
            session.punctuation_processor.record_commit(&committed_text);
            if remaining_len > 0 {
                let preedit = session.engine.formatted_preedit();
                session.commit_partial(&committed_text, &preedit);
            } else {
                session.commit_text(&committed_text);
            }
            return Some((true, Some(committed_text)));
        }
    }

    if key == KEY_ENTER {
        let raw = session.engine.raw_input().to_string();
        session.engine.reset();
        session.commit_text(&raw);
        return Some((true, Some(raw)));
    }

    if key == KEY_BACKSPACE {
        session.engine.pop_char();
        let preedit = session.engine.formatted_preedit();
        session.update_preedit(&preedit);
        return Some((true, None));
    }

    if !session.is_shift_active() && is_prev_page_key(key, session.page_keys) {
        session.engine.prev_page();
        session.refresh_candidate_surface();
        return Some((true, None));
    }
    if !session.is_shift_active() && is_next_page_key(key, session.page_keys) {
        session.engine.next_page();
        session.refresh_candidate_surface();
        return Some((true, None));
    }

    if key == KEY_LEFT {
        session.engine.move_cursor_left();
        let preedit = session.engine.formatted_preedit();
        session.update_preedit(&preedit);
        return Some((true, None));
    }
    if key == KEY_RIGHT {
        session.engine.move_cursor_right();
        let preedit = session.engine.formatted_preedit();
        session.update_preedit(&preedit);
        return Some((true, None));
    }
    if key == KEY_DELETE {
        session.engine.delete_char_at_cursor();
        let preedit = session.engine.formatted_preedit();
        session.update_preedit(&preedit);
        return Some((true, None));
    }

    if key == KEY_ESC {
        session.cancel_input();
        return Some((true, None));
    }

    None
}

/// 处理输入缓冲区为空时的空闲按键输入
pub fn process_idle_key(session: &mut WaylanV2Session, key: u32) -> Option<(bool, Option<String>)> {
    if key == KEY_BACKSPACE {
        session.delete_backward();
        return Some((true, None));
    }
    if key == KEY_DELETE {
        session.delete_forward();
        return Some((true, None));
    }
    if key == KEY_ENTER {
        session.commit_text("\n");
        return Some((true, Some("\n".to_string())));
    }
    if key == KEY_TAB {
        session.commit_text("\t");
        return Some((true, Some("\t".to_string())));
    }
    if key == KEY_SPACE {
        session.commit_text(" ");
        return Some((true, Some(" ".to_string())));
    }

    if let Some(punct_char) = keycode_to_punctuation_with_shift(key, session.is_shift_active()) {
        if let Some(actions) = session
            .punctuation_processor
            .process_punctuation(punct_char)
        {
            let mut committed_out = None;
            for action in actions {
                match action {
                    TextAction::Commit(text) => {
                        session.commit_text(&text);
                        committed_out = Some(text);
                    }
                    TextAction::MoveCursor(offset) => {
                        if let Some(ref vk) = session.virtual_keyboard {
                            let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
                            for _ in 0..offset.unsigned_abs() {
                                vk.key(0, dir_key, 1);
                                vk.key(0, dir_key, 0);
                            }
                        }
                    }
                    _ => {}
                }
            }
            return Some((true, committed_out));
        }
    }

    if !session.is_shift_active() {
        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
            let s = digit_ch.to_string();
            session.punctuation_processor.record_commit(&s);
            session.commit_text(&s);
            return Some((true, Some(s)));
        }
    }

    None
}
