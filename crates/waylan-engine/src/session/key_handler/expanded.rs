//! 二级展开卡片按键处理与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责处理 Tab 键展开/收起二级候选卡片，以及展开状态下的上下导航与选词确认。
//! - **核心优势**：将二级卡片特定的复杂状态转移与基础选词流程解耦，保持状态机正交。
//! - **代价与局限**：依赖 AlgoEngine 的 expanded 状态机与标点处理器。

use crate::input::PunctuationProcessor;
use waylan_algo::AlgoEngine;
use waylan_core::input::constants::{KEY_DOWN, KEY_ENTER, KEY_KPENTER, KEY_SPACE, KEY_TAB, KEY_UP};
use waylan_core::ExpandedCardState;

use crate::session::ImeOutputAction;

/// 处理二级展开卡片导航（Tab 键展开与卡片内部上下移动）
pub(crate) fn handle_expanded_tab_key(
    engine: &mut AlgoEngine,
    card: &mut ExpandedCardState,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
) -> Option<ImeOutputAction> {
    if key == KEY_TAB {
        let sel = engine.selected_index();
        if card.is_expanded() || engine.is_expanded() {
            *card = ExpandedCardState::Collapsed;
            engine.toggle_expand(sel);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }

        let is_expandable = engine
            .candidates()
            .get(sel)
            .is_some_and(|c| c.is_expandable && !c.sub_candidates.is_empty());

        if is_expandable {
            *card = ExpandedCardState::Expanded { selected_index: 0 };
            engine.toggle_expand(sel);
            return Some(ImeOutputAction::Handled { update_ui: true });
        } else if engine.toggle_expand(sel) {
            *card = ExpandedCardState::Expanded { selected_index: 0 };
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        return Some(ImeOutputAction::Handled { update_ui: false });
    }

    if card.is_expanded() || engine.is_expanded() {
        let sel = engine.selected_index();
        if key == KEY_DOWN {
            engine.next_expanded_item(sel);
            *card = ExpandedCardState::Expanded {
                selected_index: engine.expanded_selected_index(),
            };
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_UP {
            engine.prev_expanded_item(sel);
            *card = ExpandedCardState::Expanded {
                selected_index: engine.expanded_selected_index(),
            };
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_ENTER || key == KEY_KPENTER || key == KEY_SPACE {
            let sub_idx = card
                .selected_index()
                .unwrap_or_else(|| engine.expanded_selected_index());
            *card = ExpandedCardState::Collapsed;
            if let Some((committed, _)) = engine.select_expanded_item(sel, sub_idx) {
                punct_processor.record_commit(&committed);
                return Some(ImeOutputAction::Commit(committed));
            }
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
    }
    None
}
