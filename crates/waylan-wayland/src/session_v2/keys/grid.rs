//! V2 协议会话网格大候选面板导航与选词逻辑

use super::super::WaylanV2Session;
use waylan_core::input::*;

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
pub(super) fn process_grid_expanded_key(
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
