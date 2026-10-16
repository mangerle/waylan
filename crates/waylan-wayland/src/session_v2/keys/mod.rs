//! V2 协议键盘输入分发与状态机管理模块

mod grid;
mod repeat;

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_algo::ImeAction;
use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};

use super::surrounding::{clamp_char_boundary_ceil, clamp_char_boundary_floor};
use super::WaylanV2Session;

pub(crate) use repeat::{drive_key_repeat, next_poll_timeout_ms};

/// 根据当前 V2 会话算法引擎与输入模式构建候选框渲染状态
pub(crate) fn build_candidate_box_state(session: &WaylanV2Session) -> CandidateBoxState {
    crate::session_common::build_common_candidate_box_state(
        &session.engine,
        session.mode,
        session.is_active,
        session.is_grid_expanded,
        session.grid_selected_index,
        true,
    )
}

/// 处理按键按下阶段的核心分发逻辑
pub(super) fn process_key_press_inner(
    session: &mut WaylanV2Session,
    key: u32,
) -> (bool, Option<String>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 1. 若大网格候选面板处于展开态，优先由网格导航分发拦截
    if session.is_grid_expanded {
        if let Some(res) = grid::process_grid_expanded_key(session, key, now) {
            return res;
        }
    }

    // 2. 提取周围文本上下文（用于中文成对标点与数字上下文半角判定）
    let context_buf = session.surrounding_text.as_ref().map(|(text, cursor, _)| {
        let cursor_idx = (*cursor as usize).min(text.len());
        let safe_cursor = clamp_char_boundary_floor(text, cursor_idx);
        let start = safe_cursor.saturating_sub(64);
        let safe_start = clamp_char_boundary_ceil(text, start);
        text[safe_start..safe_cursor].to_string()
    });

    // 3. 由输入法核心会话状态机做出统一按键决策
    let action = session
        .core
        .handle_key_press(key, now, context_buf.as_deref());

    match action {
        ImeAction::PassThrough => (false, None),
        ImeAction::Handled => {
            session.grid_selected_index = 0;
            let preedit = session.engine.formatted_preedit();
            session.update_preedit(&preedit);
            (true, None)
        }
        ImeAction::ExpandGrid => {
            session.is_grid_expanded = true;
            session.grid_selected_index = session.engine.selected_index;
            session.refresh_candidate_surface();
            (true, None)
        }
        ImeAction::Commit(text) => {
            session.commit_text(&text);
            (true, Some(text))
        }
        ImeAction::CommitPunctuation {
            text,
            cursor_offset,
        } => {
            session.commit_text(&text);
            let dir_key = if cursor_offset < 0 {
                KEY_LEFT
            } else {
                KEY_RIGHT
            };
            for _ in 0..cursor_offset.unsigned_abs() {
                session.forward_key_to_seat(0, dir_key, true);
                session.forward_key_to_seat(0, dir_key, false);
            }
            (true, Some(text))
        }
        ImeAction::CommitPartial {
            committed,
            remaining_preedit,
        } => {
            session.commit_partial(&committed, &remaining_preedit);
            (true, Some(committed))
        }
        ImeAction::Reset => {
            session.cancel_input();
            (true, None)
        }
    }
}

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub(crate) fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    session.update_modifier_by_key(key, is_press);

    // 外部快捷键拦截器（如语音听写）优先捕获
    if let Some(mut filter) = session.key_filter.take() {
        let filter_outcome = filter(key, is_press, session.core.mods_depressed());
        session.key_filter = Some(filter);
        match filter_outcome {
            crate::session_common::KeyFilterResult::Consumed => {
                if is_press {
                    session.consumed_keys.record_press(key, true);
                } else {
                    session.consumed_keys.take_release(key);
                }
                return (true, None);
            }
            crate::session_common::KeyFilterResult::Commit(text) => {
                session.commit_text(&text);
                session.punctuation_processor.record_commit(&text);
                session.refresh_candidate_surface();
                if is_press {
                    session.consumed_keys.record_press(key, true);
                } else {
                    session.consumed_keys.take_release(key);
                }
                return (true, Some(text));
            }
            crate::session_common::KeyFilterResult::PassThrough => {}
        }
    }

    // 密码或 PIN 码输入框自动规避拼音拦截，全量直通按键
    if session.is_password_context {
        session.repeat_state = None;
        session.core.shift_sm.reset();
        if !is_press {
            return (session.consumed_keys.take_release(key), None);
        }
        session.consumed_keys.record_press(key, false);
        return (false, None);
    }

    // CapsLock 截断提交处理
    if key == KEY_CAPSLOCK {
        session.repeat_state = None;
        let (was_consumed, committed) = session.core.handle_capslock(is_press);
        if let Some(ref raw) = committed {
            session.commit_text(raw);
            session.refresh_candidate_surface();
        }
        return (was_consumed, committed);
    }

    // 按键释放事件处理
    if !is_press {
        if session
            .repeat_state
            .is_some_and(|(rep_key, _)| rep_key == key)
        {
            session.repeat_state = None;
        }
        let (was_consumed, toggled_mode, committed) = session.core.handle_key_release(key);
        if let Some(new_mode) = toggled_mode {
            if let Some(ref zh_flag) = session.shared_is_chinese {
                zh_flag.store(
                    matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                    Ordering::Release,
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
            debug!("[Wayland v2] 切换输入模式为：{}", mode_str);

            if let Some(ref raw) = committed {
                session.commit_text(raw);
            }
        }
        return (was_consumed, committed);
    }

    // 按键按下事件核心分发
    let result = process_key_press_inner(session, key);
    session.consumed_keys.record_press(key, result.0);
    repeat::update_key_repeat(session, key, result.0);
    result
}
