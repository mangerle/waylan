//! 二级展开卡片按键处理与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责处理 Tab 键展开/收起二级候选卡片，以及展开状态下的上下导航与选词确认。
//! - **核心优势**：将二级卡片特定的复杂状态转移与基础选词流程解耦，保持状态机正交。
//! - **代价与局限**：依赖 AlgoEngine 的 expanded 状态机与标点处理器。

use waylan_algo::preprocessor::PunctuationProcessor;
use waylan_algo::AlgoEngine;
use waylan_core::input::constants::{KEY_DOWN, KEY_ENTER, KEY_KPENTER, KEY_SPACE, KEY_TAB, KEY_UP};

use crate::session::ImeOutputAction;

/// 处理二级展开卡片导航（Tab 键展开）
pub(crate) fn handle_expanded_tab_key(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
) -> Option<ImeOutputAction> {
    if key == KEY_TAB {
        let sel = engine.selected_index();
        engine.toggle_expand(sel);
        return Some(ImeOutputAction::Handled { update_ui: true });
    }

    if engine.is_expanded() {
        let sel = engine.selected_index();
        if key == KEY_DOWN {
            engine.next_expanded_item(sel);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_UP {
            engine.prev_expanded_item(sel);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_ENTER || key == KEY_KPENTER || key == KEY_SPACE {
            if let Some((committed, _)) =
                engine.select_expanded_item(sel, engine.expanded_selected_index())
            {
                punct_processor.record_commit(&committed);
                return Some(ImeOutputAction::Commit(committed));
            }
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
    }
    None
}
