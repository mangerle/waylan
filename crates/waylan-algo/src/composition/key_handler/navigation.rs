//! 拼音光标编辑与候选词流转导航按键处理
//!
//! # 设计原理
//! - **实现初衷**：在横排候选框视觉直觉（左右选词、上下翻页）与拼音字母长句纠错（左右移动光标）之间实现无缝自适应流转。
//! - **选词模式（光标在末尾）**：
//!   - `KEY_RIGHT`：向右移动候选词高亮，到达页末时自动翻下一页；
//!   - `KEY_LEFT`：向左移动候选词高亮；若已在第 1 页第 1 候选词，再次按左键触发边缘溢出，进入拼音光标编辑模式；
//!     若在非第 1 页第 1 候选词，则翻回上一页末尾；
//!   - `KEY_DOWN`：翻下一页；若已在末页或无法翻页，唤出大网格候选面板；
//!   - `KEY_UP`：翻上一页；
//! - **拼音光标编辑模式（光标在拼音内部）**：
//!   - `KEY_LEFT`：光标向左移动一个字符；
//!   - `KEY_RIGHT`：光标向右移动一个字符，移到末尾时自动恢复为选词模式；
//!   - `KEY_DOWN`：翻下一页；末页时展开大网格；
//!   - `KEY_UP`：翻上一页。

use waylan_core::input::constants::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};
use waylan_core::input::page_keys::PageKeyConfig;

use crate::session::ImeOutputAction;
use crate::AlgoEngine;

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
                    if engine.viewport.selected_index + 1 < cand_len {
                        engine.viewport.selected_index += 1;
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
                if engine.viewport.selected_index > 0 {
                    engine.viewport.selected_index -= 1;
                } else if engine.can_prev_page() {
                    engine.prev_page();
                    engine.viewport.selected_index = engine.candidates().len().saturating_sub(1);
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
