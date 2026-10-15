use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_core::{CandidateBoxState, InputMode, TextAction};

use super::surrounding::{clamp_char_boundary_ceil, clamp_char_boundary_floor};
use super::WaylanV2Session;
use crate::input_core::*;

/// 根据当前 V2 会话算法引擎与输入模式构建候选框渲染状态
pub fn build_candidate_box_state(session: &WaylanV2Session) -> CandidateBoxState {
    crate::session_common::build_common_candidate_box_state(
        &session.engine,
        session.mode,
        session.is_active,
        session.is_grid_expanded,
        session.grid_selected_index,
        true,
    )
}

/// 处理英文模式下的按键输入（所有常规按键不拦截，由席位原生透传以支持 Key Repeat 与系统布局）
pub fn process_english_mode_key(
    _session: &mut WaylanV2Session,
    _key: u32,
) -> (bool, Option<String>) {
    (false, None)
}

/// 提交指定全局索引的网格候选词上屏并退出大网格展开态
fn commit_grid_candidate_by_index(
    session: &mut WaylanV2Session,
    target_idx: usize,
    now: u64,
) -> Option<(bool, Option<String>)> {
    if let Some((committed_text, remaining_len)) =
        session.engine.select_candidate_from_all(target_idx, now)
    {
        session.is_grid_expanded = false;
        session.grid_selected_index = 0;
        session.punctuation_processor.record_commit(&committed_text);
        if remaining_len > 0 {
            let remaining_preedit = session.engine.formatted_preedit();
            session.commit_partial(&committed_text, &remaining_preedit);
        } else {
            session.commit_text(&committed_text);
        }
        Some((true, Some(committed_text)))
    } else {
        Some((true, None))
    }
}

/// 处理网格大候选面板的方向导航与 ESC 折叠键
fn handle_grid_navigation_key(
    session: &mut WaylanV2Session,
    key: u32,
    total: usize,
    row_count: usize,
    row: usize,
    col: usize,
) -> Option<bool> {
    if key == KEY_DOWN {
        let next_row = (row + 1) % row_count;
        session.grid_selected_index = (next_row * 6 + col).min(total.saturating_sub(1));
        session.refresh_candidate_surface();
        return Some(true);
    }
    if key == KEY_UP {
        let prev_row = if row == 0 {
            row_count.saturating_sub(1)
        } else {
            row - 1
        };
        session.grid_selected_index = (prev_row * 6 + col).min(total.saturating_sub(1));
        session.refresh_candidate_surface();
        return Some(true);
    }
    if key == KEY_RIGHT {
        session.grid_selected_index = (session.grid_selected_index + 1) % total;
        session.refresh_candidate_surface();
        return Some(true);
    }
    if key == KEY_LEFT {
        session.grid_selected_index = if session.grid_selected_index == 0 {
            total.saturating_sub(1)
        } else {
            session.grid_selected_index - 1
        };
        session.refresh_candidate_surface();
        return Some(true);
    }
    if key == KEY_ESC {
        session.is_grid_expanded = false;
        session.grid_selected_index = 0;
        session.refresh_candidate_surface();
        return Some(true);
    }
    None
}

/// 处理网格大候选面板的数字键选词与空格键确认
fn handle_grid_selection_key(
    session: &mut WaylanV2Session,
    key: u32,
    now: u64,
    total: usize,
    row: usize,
) -> Option<Option<(bool, Option<String>)>> {
    if !session.is_shift_active() {
        if let Some(digit) = keycode_to_digit(key) {
            if (1..=6).contains(&digit) {
                let target_idx = row * 6 + (digit - 1);
                if target_idx < total {
                    return Some(commit_grid_candidate_by_index(session, target_idx, now));
                }
            }
            return Some(Some((true, None)));
        }
    }
    if key == KEY_SPACE {
        let idx = session.grid_selected_index.min(total.saturating_sub(1));
        return Some(commit_grid_candidate_by_index(session, idx, now));
    }
    None
}

/// 处理处于 5 行网格大候选词面板展开态下的专属键盘导航与选词
fn process_grid_expanded_key(
    session: &mut WaylanV2Session,
    key: u32,
    now: u64,
) -> Option<(bool, Option<String>)> {
    let total = session.engine.all_candidates().len().min(30);
    if total == 0 {
        return None;
    }
    const GRID_COLS: usize = 6;
    let row_count = total.div_ceil(GRID_COLS).clamp(1, 5);
    let row = session.grid_selected_index / GRID_COLS;
    let col = session.grid_selected_index % GRID_COLS;

    if let Some(consumed) = handle_grid_navigation_key(session, key, total, row_count, row, col) {
        return Some((consumed, None));
    }

    if let Some(res) = handle_grid_selection_key(session, key, now, total, row) {
        return res;
    }

    None
}

/// 执行拼音组词按键动作状态流转
fn apply_composition_action(
    session: &mut WaylanV2Session,
    action: KeyProcessAction,
) -> (bool, Option<String>) {
    match action {
        KeyProcessAction::PassThrough => (false, None),
        KeyProcessAction::Handled => {
            session.grid_selected_index = 0;
            let preedit = session.engine.formatted_preedit();
            session.update_preedit(&preedit);
            (true, None)
        }
        KeyProcessAction::ExpandGrid => {
            session.is_grid_expanded = true;
            session.grid_selected_index = session.engine.selected_index;
            session.refresh_candidate_surface();
            (true, None)
        }
        KeyProcessAction::Commit(committed) => {
            session.commit_text(&committed);
            (true, Some(committed))
        }
        KeyProcessAction::CommitPartial {
            committed,
            remaining_preedit,
        } => {
            session.commit_partial(&committed, &remaining_preedit);
            (true, Some(committed))
        }
        KeyProcessAction::Reset => {
            session.cancel_input();
            (true, None)
        }
    }
}

/// 处理处于拼音组词状态下的键盘拦截
pub fn process_composition_key(
    session: &mut WaylanV2Session,
    key: u32,
    now: u64,
) -> Option<(bool, Option<String>)> {
    if session.is_grid_expanded {
        if let Some(res) = process_grid_expanded_key(session, key, now) {
            return Some(res);
        }
    }

    let params = CompositionKeyParams {
        key,
        shift_active: session.is_shift_active(),
        page_keys: session.page_keys,
        now,
    };
    let action = handle_composition_key_with_params(
        &mut session.engine,
        &mut session.punctuation_processor,
        params,
    )?;

    if matches!(action, KeyProcessAction::PassThrough) {
        None
    } else {
        Some(apply_composition_action(session, action))
    }
}

/// 处理空闲状态下的中文智能标点符号拦截
fn handle_idle_punctuation(
    session: &mut WaylanV2Session,
    key: u32,
) -> Option<(bool, Option<String>)> {
    let punct_char = keycode_to_punctuation_with_shift(key, session.is_shift_active())?;
    let context_buf = session.surrounding_text.as_ref().map(|(text, cursor, _)| {
        let cursor_idx = (*cursor as usize).min(text.len());
        let safe_cursor = clamp_char_boundary_floor(text, cursor_idx);
        let start = safe_cursor.saturating_sub(64);
        let safe_start = clamp_char_boundary_ceil(text, start);
        text[safe_start..safe_cursor].to_string()
    });

    let actions = match context_buf {
        Some(ref ctx) => session
            .punctuation_processor
            .process_punctuation_with_context(punct_char, ctx)?,
        None => session
            .punctuation_processor
            .process_punctuation(punct_char)?,
    };

    let mut committed_out = None;
    for action in actions {
        match action {
            TextAction::Commit(text) => {
                session.commit_text(&text);
                session.punctuation_processor.record_commit(&text);
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
    Some((true, committed_out))
}

/// 处理输入缓冲区为空时的空闲按键输入（仅拦截中文标点，其余编辑键与数字键保持原生透传）
pub fn process_idle_key(session: &mut WaylanV2Session, key: u32) -> Option<(bool, Option<String>)> {
    if let Some(res) = handle_idle_punctuation(session, key) {
        return Some(res);
    }

    if !session.is_shift_active() {
        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
            let s = digit_ch.to_string();
            session.punctuation_processor.record_commit(&s);
        }
    }

    None
}

/// 处理 CapsLock 键事件并在有拼音未上屏时提交英文字符
fn handle_capslock_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    session.repeat_state = None;
    if is_press {
        if !session.engine.raw_input().is_empty() {
            let raw = session.engine.raw_input().to_string();
            session.commit_text(&raw);
            session.engine.reset();
            session.refresh_candidate_surface();
            session.consumed_keys.record_press(key, false);
            return (false, Some(raw));
        }
        session.consumed_keys.record_press(key, false);
        (false, None)
    } else {
        (session.consumed_keys.take_release(key), None)
    }
}

/// 处理按键释放阶段的 Shift/Ctrl 状态机模式切换
fn handle_key_release_mode_toggle(
    session: &mut WaylanV2Session,
    key: u32,
    toggled_mode: Option<InputMode>,
) -> (bool, Option<String>) {
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

        if new_mode == InputMode::English && !session.engine.raw_input().is_empty() {
            let raw = session.engine.raw_input().to_string();
            session.commit_text(&raw);
            session.engine.reset();
            return (was_press_consumed, Some(raw));
        }
    }
    (was_press_consumed, None)
}

/// 根据本次按键消费状态与连击设置更新连击计时器
fn update_key_repeat(session: &mut WaylanV2Session, key: u32, consumed: bool) {
    let (rate, delay) = session.repeat_info;
    if consumed && rate > 0 && delay > 0 && !session.engine.raw_input().is_empty() {
        let next_fire = Instant::now() + Duration::from_millis(delay.max(50) as u64);
        session.repeat_state = Some((key, next_fire));
    } else {
        session.repeat_state = None;
    }
}

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    session.update_modifier_by_key(key, is_press);

    // 密码或 PIN 码输入框自动规避拼音拦截，全量直通按键
    if session.is_password_context {
        session.repeat_state = None;
        session.shift_state.reset();
        if !is_press {
            return (session.consumed_keys.take_release(key), None);
        }
        session.consumed_keys.record_press(key, false);
        return (false, None);
    }

    if key == KEY_CAPSLOCK {
        return handle_capslock_event(session, key, is_press);
    }

    let toggled_mode = session
        .shift_state
        .on_key_event(key, is_press, session.mode);

    if !is_press {
        return handle_key_release_mode_toggle(session, key, toggled_mode);
    }

    let result = process_key_press_inner(session, key);
    session.consumed_keys.record_press(key, result.0);
    update_key_repeat(session, key, result.0);
    result
}

/// 处理按键按下阶段的核心分发逻辑
pub fn process_key_press_inner(session: &mut WaylanV2Session, key: u32) -> (bool, Option<String>) {
    // 快捷键穿透：Ctrl / Alt / Super 等修饰组合键透传
    if session.has_ctrl_or_alt() {
        return (false, None);
    }

    // 处于 CapsLock 大写锁定状态时，所有按键保持原生透传，由宿主应用原生输出大写英文字母
    if session.is_caps_locked() {
        return (false, None);
    }

    // 2. 英文模式
    if session.mode == InputMode::English {
        return process_english_mode_key(session, key);
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
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

    // 5. 字母键输入（A-Z）：进入拼音算法管线（支持动态键盘布局）
    if let Some(ch) = session.keymap_layout.get_char(key) {
        session.engine.push_char(ch);
        session.grid_selected_index = 0;
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
    let now = Instant::now();
    if next_fire <= now {
        return 0;
    }
    let remain_ms = next_fire.duration_since(now).as_millis() as i32;
    if default_ms < 0 {
        remain_ms.max(1)
    } else {
        remain_ms.clamp(1, default_ms)
    }
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
    };

    let now = Instant::now();
    if now >= next_fire {
        let (consumed, _) = process_key_press_inner(session, key);
        if consumed && !session.engine.raw_input().is_empty() {
            let interval_ms = (1000 / rate.max(1)).max(10) as u64;
            session.repeat_state = Some((key, now + Duration::from_millis(interval_ms)));
        } else {
            session.repeat_state = None;
        }
    }
}
