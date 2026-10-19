//! 拼音光标编辑与候选词流转导航按键处理
//!
//! # 设计原理
//! - **实现初衷**：在横排候选框视觉直觉（左右选词、上下翻页）与拼音字母长句纠错（左右移动光标）之间实现无缝自适应流转。
//! - **代价与局限**：对部分快捷键按键进行方向映射。

use waylan_algo::AlgoEngine;
use waylan_core::input::constants::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};
use waylan_core::input::page_keys::PageKeyConfig;

use crate::session::ImeOutputAction;

/// 处理智能上下文流转方向键导航（现代输入流转模型）
pub(crate) fn handle_smart_navigation_key(
    engine: &mut AlgoEngine,
    key: u32,
) -> Option<ImeOutputAction> {
    let is_cursor_at_end = engine.cursor() >= engine.raw_input().len();

    match key {
        KEY_RIGHT => {
            if is_cursor_at_end {
                let cand_len = engine.candidates().len();
                if cand_len > 0 {
                    if engine.selected_index() + 1 < cand_len {
                        engine.set_selected_index(engine.selected_index() + 1);
                    } else if engine.can_next_page() {
                        engine.next_page();
                    }
                }
            } else {
                engine.move_cursor_right();
            }
            Some(ImeOutputAction::Handled { update_ui: true })
        }
        KEY_LEFT => {
            if is_cursor_at_end {
                if engine.selected_index() > 0 {
                    engine.set_selected_index(engine.selected_index() - 1);
                } else if engine.can_prev_page() {
                    engine.prev_page();
                    engine.set_selected_index(engine.candidates().len().saturating_sub(1));
                } else {
                    engine.move_cursor_left();
                }
            } else {
                engine.move_cursor_left();
            }
            Some(ImeOutputAction::Handled { update_ui: true })
        }
        KEY_DOWN => {
            if engine.can_next_page() {
                engine.next_page();
                Some(ImeOutputAction::Handled { update_ui: true })
            } else {
                Some(ImeOutputAction::ExpandGrid)
            }
        }
        KEY_UP => {
            if engine.can_prev_page() {
                engine.prev_page();
            }
            Some(ImeOutputAction::Handled { update_ui: true })
        }
        _ => None,
    }
}

/// 处理候选词翻页快捷键
pub(crate) fn handle_page_flip_keys(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
    page_keys: PageKeyConfig,
) -> Option<ImeOutputAction> {
    if !shift_active && (page_keys.is_prev_page(key) || page_keys.is_next_page(key)) {
        if page_keys.is_prev_page(key) {
            engine.prev_page();
        } else {
            engine.next_page();
        }
        return Some(ImeOutputAction::Handled { update_ui: true });
    }
    None
}
