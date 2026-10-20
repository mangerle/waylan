//! 候选词选取、分段提交、V模式与编辑按键处理
//!
//! # 设计原理
//! - **实现初衷**：负责数字选词、空格/回车确认、ESC取消以及拼音退格与分词单引号编辑。
//! - **核心优势**：将输入确认与基础编辑动作聚合在单一模块，保证选词提交与视口状态流转严密一致。
//! - **代价与局限**：对部分双功能按键进行多层条件拦截。

use waylan_core::input::constants::{
    KEY_APOSTROPHE, KEY_BACKSPACE, KEY_DELETE, KEY_ENTER, KEY_ESC, KEY_KPENTER, KEY_SPACE,
};
use waylan_core::input::conversion::{keycode_to_digit, keycode_to_v_mode_char};

use crate::session::{ImeCoreSession, ImeOutputAction};

/// 处理 V模式字符输入
pub(crate) fn handle_v_mode_key(
    session: &mut ImeCoreSession,
    key: u32,
    shift_active: bool,
) -> Option<ImeOutputAction> {
    if session.engine.raw_input().starts_with('v') {
        if let Some(ch) = keycode_to_v_mode_char(key, shift_active) {
            session.engine.push_char(ch);
            session.reset_viewport();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
    }
    None
}

/// 处理数字按键选词（支持一级候选与二级展开卡片）
pub(crate) fn handle_digit_selection(
    session: &mut ImeCoreSession,
    key: u32,
    shift_active: bool,
    now: u64,
) -> Option<ImeOutputAction> {
    if session.engine.raw_input().starts_with('v') {
        return None;
    }
    // Shift+数字默认用于输入符号；仅在用户开启“Shift 选词”后拦截为候选选择
    if shift_active && !session.shift_select_second {
        return None;
    }

    let digit = keycode_to_digit(key)?;
    if session.is_card_expanded() {
        let sub_idx = digit.saturating_sub(1);
        let sel = session.selected_global_index();
        session.collapse_card();

        if let Some(cand) = session.engine.candidates().get(sel) {
            if let Some(sub_text) = cand.sub_candidates.get(sub_idx).cloned() {
                let (committed, _) = session.engine.commit_custom_text(&sub_text);
                session.reset_viewport();
                session.punctuation_processor.record_commit(&committed);
                return Some(ImeOutputAction::Commit(committed));
            }
        }
        Some(ImeOutputAction::Handled { update_ui: true })
    } else {
        let offset = digit.saturating_sub(1);
        if offset < session.current_page_candidates().len() {
            let target_idx = session.page_index.saturating_mul(session.page_size()) + offset;
            Some(session.commit_candidate_by_global_index(target_idx, now))
        } else {
            Some(ImeOutputAction::Handled { update_ui: false })
        }
    }
}

/// 处理空格、回车与 ESC 确认或重置按键
pub(crate) fn handle_action_keys(
    session: &mut ImeCoreSession,
    key: u32,
    now: u64,
) -> Option<ImeOutputAction> {
    if key == KEY_SPACE {
        if session.engine.cursor() < session.engine.raw_input().len() {
            session.engine.move_cursor_to_end();
            session.page_index = 0;
            session.selected_index = 0;
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        let target_idx = session.selected_global_index();
        return Some(session.commit_candidate_by_global_index(target_idx, now));
    }
    if key == KEY_ENTER || key == KEY_KPENTER {
        let raw = session.engine.raw_input().to_string();
        session.reset_context();
        return Some(ImeOutputAction::Commit(raw));
    }
    if key == KEY_ESC {
        if session.is_card_expanded() {
            session.collapse_card();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        session.reset_context();
        return Some(ImeOutputAction::Reset);
    }
    None
}

/// 处理退格删除与分词单引号
pub(crate) fn handle_editing_keys(
    session: &mut ImeCoreSession,
    key: u32,
    shift_active: bool,
) -> Option<ImeOutputAction> {
    if key == KEY_BACKSPACE || key == KEY_DELETE {
        if key == KEY_BACKSPACE {
            session.engine.pop_char();
        } else {
            session.engine.delete_char_at_cursor();
        }
        return Some(if session.engine.raw_input().is_empty() {
            session.reset_context();
            ImeOutputAction::Reset
        } else {
            session.page_index = 0;
            session.selected_index = 0;
            session.collapse_card();
            ImeOutputAction::Handled { update_ui: true }
        });
    }
    if !shift_active && key == KEY_APOSTROPHE {
        session.engine.push_char('\'');
        session.page_index = 0;
        session.selected_index = 0;
        session.collapse_card();
        return Some(ImeOutputAction::Handled { update_ui: true });
    }
    None
}
