//! 二级展开卡片按键处理与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责处理 Tab 键展开/收起二级候选卡片，以及展开状态下的上下导航与选词确认。
//! - **核心优势**：将二级卡片特定的复杂状态转移与基础选词流程解耦，状态由会话层统一纳管。
//! - **代价与局限**：对部分双功能按键（如 Enter、Space）在展开态进行专用拦截。

use waylan_core::input::constants::{KEY_DOWN, KEY_ENTER, KEY_KPENTER, KEY_SPACE, KEY_TAB, KEY_UP};

use crate::session::{ImeCoreSession, ImeOutputAction};

/// 处理二级展开卡片导航（Tab 键展开与卡片内部上下移动）
pub(crate) fn handle_expanded_tab_key(
    session: &mut ImeCoreSession,
    key: u32,
) -> Option<ImeOutputAction> {
    if key == KEY_TAB {
        if session.is_card_expanded() {
            session.collapse_card();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }

        let sel = session.selected_global_index();
        let is_expandable = session
            .engine
            .candidates()
            .get(sel)
            .is_some_and(|c| c.is_expandable && !c.sub_candidates.is_empty());

        if is_expandable {
            session.expand_card(0);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }

        // 若当前选中的候选词不可展开，但在当前展示页内存在可展开候选，自动聚焦并展开
        if let Some((page_idx, _)) = session
            .current_page_candidates()
            .iter()
            .enumerate()
            .find(|(_, c)| c.is_expandable && !c.sub_candidates.is_empty())
        {
            session.selected_index = page_idx;
            session.expand_card(0);
            return Some(ImeOutputAction::Handled { update_ui: true });
        }

        return Some(ImeOutputAction::Handled { update_ui: false });
    }

    if session.is_card_expanded() {
        if key == KEY_DOWN {
            session.next_card_item();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_UP {
            session.prev_card_item();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_ENTER || key == KEY_KPENTER || key == KEY_SPACE {
            let sel = session.selected_global_index();
            let sub_idx = session.card_selected_index();
            session.collapse_card();

            if let Some(cand) = session.engine.candidates().get(sel) {
                if let Some(sub_text) = cand.sub_candidates.get(sub_idx).cloned() {
                    let (committed, _) = session.engine.commit_custom_text(&sub_text);
                    session.reset_viewport();
                    session.punctuation_processor.record_commit(&committed);
                    return Some(ImeOutputAction::Commit(committed));
                }
            }
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
    }
    None
}
