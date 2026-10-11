use super::WaylanV2Session;
use crate::input_core::*;
use waylan_core::{InputMode, TextAction};

/// 处理英文模式下的按键输入（所有常规按键不拦截，由席位原生透传以支持 Key Repeat 与系统布局）
pub fn process_english_mode_key(
    _session: &mut WaylanV2Session,
    _key: u32,
) -> (bool, Option<String>) {
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

/// 处理输入缓冲区为空时的空闲按键输入（仅拦截中文标点，其余编辑键与数字键保持原生透传）
pub fn process_idle_key(session: &mut WaylanV2Session, key: u32) -> Option<(bool, Option<String>)> {
    // 中文模式下的智能标点符号拦截（含 Shift 组合标点，如 !?$^_()""<>{}:~）
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
                        let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
                        for _ in 0..offset.unsigned_abs() {
                            session.forward_key_to_seat(0, dir_key, true);
                            session.forward_key_to_seat(0, dir_key, false);
                        }
                    }
                    _ => {}
                }
            }
            return Some((true, committed_out));
        }
    }

    // 若为未按 Shift 的纯数字键输入，记录数字上下文以便后续触发 3.14 / 12:00 半角标点保护，但按键本身保持原生透传
    if !session.is_shift_active() {
        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
            let s = digit_ch.to_string();
            session.punctuation_processor.record_commit(&s);
        }
    }

    // 其他空闲编辑键（回车、退格、空格、Tab、数字等）不拦截，原生透传给焦点应用
    None
}

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    session.update_modifier_by_key(key, is_press);

    // 1. 优先经由 Shift/Ctrl 防误触状态机判定
    let toggled_mode = session
        .shift_state
        .on_key_event(key, is_press, session.mode);

    // 按键释放阶段：依据该按键按下阶段是否被输入法消费决定是否消费释放事件，确保 Press/Release 严格对称
    if !is_press {
        if session
            .repeat_state
            .is_some_and(|(rep_key, _)| rep_key == key)
        {
            session.repeat_state = None;
        }
        let was_press_consumed = session.consumed_keys.take_release(key);
        if let Some(new_mode) = toggled_mode {
            session.mode = new_mode;
            if let Some(ref zh_flag) = session.shared_is_chinese {
                zh_flag.store(
                    matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                    std::sync::atomic::Ordering::Release,
                );
            }
            if let Some(ref notify_tx) = session.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            let mode_str = match new_mode {
                InputMode::Chinese => "中文",
                InputMode::English => "英文",
                InputMode::VMode => "V模式",
            };
            log::info!("[Wayland v2] 切换输入模式为：{}", mode_str);

            if new_mode == InputMode::English && !session.engine.raw_input().is_empty() {
                let raw = session.engine.raw_input().to_string();
                session.commit_text(&raw);
                session.engine.reset();
                return (was_press_consumed, Some(raw));
            }
        }
        return (was_press_consumed, None);
    }

    let result = process_key_press_inner(session, key);
    session.consumed_keys.record_press(key, result.0);

    // 若按键被输入法内部消费（如退格、方向键、拼音字母）且合成器开启了长按连击（rate > 0），启动连击计时
    let (rate, delay) = session.repeat_info;
    if result.0 && rate > 0 && delay > 0 && !session.engine.raw_input().is_empty() {
        let next_fire =
            std::time::Instant::now() + std::time::Duration::from_millis(delay.max(50) as u64);
        session.repeat_state = Some((key, next_fire));
    } else {
        session.repeat_state = None;
    }

    result
}

/// 处理按键按下阶段的核心分发逻辑
pub fn process_key_press_inner(session: &mut WaylanV2Session, key: u32) -> (bool, Option<String>) {
    // 快捷键穿透：Ctrl / Alt / Super 等修饰组合键透传
    if session.has_ctrl_or_alt() {
        return (false, None);
    }

    // 2. 英文模式
    if session.mode == InputMode::English {
        return process_english_mode_key(session, key);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 3. 正在组合拼音状态下的专门按键拦截
    if !session.engine.raw_input().is_empty() {
        if let Some(result) = process_composition_key(session, key, now) {
            return result;
        }
    }

    // 4. 输入缓冲区为空时的按键处理
    if session.engine.raw_input().is_empty() {
        if let Some(result) = process_idle_key(session, key) {
            return result;
        }
    }

    // 5. 字母键输入（A-Z）：进入拼音算法管线
    if let Some(ch) = keycode_to_char(key) {
        session.engine.push_char(ch);
        let preedit = session.engine.formatted_preedit();
        session.update_preedit(&preedit);
        return (true, None);
    }

    (false, None)
}

/// 计算事件循环下一次 poll 的等待超时毫秒数（若存在活跃按键连击，按剩余到期时间缩短超时）
pub fn next_poll_timeout_ms(session: &WaylanV2Session, default_ms: i32) -> i32 {
    let Some((_, next_fire)) = session.repeat_state else {
        return default_ms;
    };
    let now = std::time::Instant::now();
    if next_fire <= now {
        return 0;
    }
    let remain_ms = next_fire.duration_since(now).as_millis() as i32;
    remain_ms.clamp(1, default_ms)
}

/// 检查并触发已到期的长按按键连击事件
pub fn drive_key_repeat(session: &mut WaylanV2Session) {
    let Some((key, next_fire)) = session.repeat_state else {
        return;
    };
    let (rate, _) = session.repeat_info;
    if rate <= 0 || !session.is_active || session.engine.raw_input().is_empty() {
        session.repeat_state = None;
        return;
    }

    let now = std::time::Instant::now();
    if now >= next_fire {
        let (consumed, _) = process_key_press_inner(session, key);
        if consumed && !session.engine.raw_input().is_empty() {
            let interval_ms = (1000 / rate.max(1)).max(10) as u64;
            session.repeat_state = Some((key, now + std::time::Duration::from_millis(interval_ms)));
        } else {
            session.repeat_state = None;
        }
    }
}
