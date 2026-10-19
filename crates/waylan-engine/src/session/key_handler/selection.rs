//! 候选词选取、分段提交、V模式与编辑按键处理
//!
//! # 设计原理
//! - **实现初衷**：负责数字选词、空格/回车确认、ESC取消以及拼音退格与分词单引号编辑。
//! - **核心优势**：将输入确认与基础编辑动作聚合在单一模块，保证选词提交与词频学习逻辑严密一致。
//! - **代价与局限**：选词提交需向 PunctuationProcessor 记录提交历史以支持智能成对标点与自动推断。

use crate::input::PunctuationProcessor;
use waylan_algo::AlgoEngine;
use waylan_core::input::constants::{
    KEY_APOSTROPHE, KEY_BACKSPACE, KEY_DELETE, KEY_ENTER, KEY_ESC, KEY_KPENTER, KEY_SPACE,
};
use waylan_core::input::conversion::{keycode_to_digit, keycode_to_v_mode_char};

use crate::session::ImeOutputAction;

/// 选取指定索引候选词并转换为协议决策枚举
pub(crate) fn select_candidate_action(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    cand_idx: usize,
    now: u64,
) -> ImeOutputAction {
    if let Some((committed_text, remaining_len)) = engine.select_candidate(cand_idx, now) {
        punct_processor.record_commit(&committed_text);
        if remaining_len > 0 {
            ImeOutputAction::CommitPartial {
                committed: committed_text,
                remaining_preedit: engine.formatted_preedit(),
            }
        } else {
            ImeOutputAction::Commit(committed_text)
        }
    } else {
        ImeOutputAction::Handled { update_ui: true }
    }
}

/// 处理 V模式字符输入
pub(crate) fn handle_v_mode_key(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
) -> Option<ImeOutputAction> {
    if engine.raw_input().starts_with('v') {
        if let Some(ch) = keycode_to_v_mode_char(key, shift_active) {
            engine.push_char(ch);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
    }
    None
}

/// 处理数字按键选词（支持一级候选与二级展开卡片）
pub(crate) fn handle_digit_selection(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    shift_active: bool,
    now: u64,
) -> Option<ImeOutputAction> {
    if shift_active || engine.raw_input().starts_with('v') {
        return None;
    }

    let digit = keycode_to_digit(key)?;
    if engine.is_expanded() {
        let sel = engine.selected_index();
        let sub_idx = digit.saturating_sub(1);
        if let Some((committed, _)) = engine.select_expanded_item(sel, sub_idx) {
            punct_processor.record_commit(&committed);
            return Some(ImeOutputAction::Commit(committed));
        }
        Some(ImeOutputAction::Handled { update_ui: true })
    } else {
        Some(select_candidate_action(
            engine,
            punct_processor,
            digit.saturating_sub(1),
            now,
        ))
    }
}

/// 处理空格、回车与 ESC 确认或重置按键
pub(crate) fn handle_action_keys(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    now: u64,
) -> Option<ImeOutputAction> {
    if key == KEY_SPACE {
        if engine.cursor() < engine.raw_input().len() {
            engine.move_cursor_to_end();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        let sel = engine.selected_index();
        return Some(select_candidate_action(engine, punct_processor, sel, now));
    }
    if key == KEY_ENTER || key == KEY_KPENTER {
        let raw = engine.raw_input().to_string();
        engine.reset();
        return Some(ImeOutputAction::Commit(raw));
    }
    if key == KEY_ESC {
        if engine.is_expanded() {
            engine.collapse_expanded();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        engine.reset();
        return Some(ImeOutputAction::Reset);
    }
    None
}

/// 处理退格删除与分词单引号
pub(crate) fn handle_editing_keys(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
) -> Option<ImeOutputAction> {
    if key == KEY_BACKSPACE || key == KEY_DELETE {
        if key == KEY_BACKSPACE {
            engine.pop_char();
        } else {
            engine.delete_char_at_cursor();
        }
        return Some(if engine.raw_input().is_empty() {
            ImeOutputAction::Reset
        } else {
            ImeOutputAction::Handled { update_ui: true }
        });
    }
    if !shift_active && key == KEY_APOSTROPHE {
        engine.push_char('\'');
        return Some(ImeOutputAction::Handled { update_ui: true });
    }
    None
}
