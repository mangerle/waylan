//! 二级展开卡片按键处理与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责处理 Tab 键展开/收起二级候选卡片，以及展开状态下的上下导航与选词确认。
//! - **核心优势**：将二级卡片特定的复杂状态转移与基础选词流程解耦，状态由会话层统一纳管。
//! - **代价与局限**：对部分双功能按键（如 Enter、Space）在展开态进行专用拦截。

use waylan_core::input::constants::{KEY_DOWN, KEY_ENTER, KEY_KPENTER, KEY_SPACE, KEY_TAB, KEY_UP};

use crate::session::{ImeCoreSession, ImeOutputAction};

/// 处理二级展开卡片导航（Tab 键展开与卡片内部上下移动）
///
/// # 设计原理
/// - 判定顺序固定为：Tab 键展开/收起 -> 卡片展开态下的上下导航 -> 子词条确认，
///   任何一级命中后立即返回，避免同一次按键被多层状态机重复消费。
pub(crate) fn handle_expanded_tab_key(
    session: &mut ImeCoreSession,
    key: u32,
) -> Option<ImeOutputAction> {
    // 1. Tab 键负责二级候选卡片的展开与收起
    if key == KEY_TAB {
        return toggle_expanded_card(session);
    }

    // 2. 卡片展开态下拦截上下导航与子词条确认
    if session.is_card_expanded() {
        match key {
            KEY_DOWN => {
                session.next_card_item();
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_UP => {
                session.prev_card_item();
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_ENTER | KEY_KPENTER | KEY_SPACE => {
                return Some(commit_expanded_card_selection(session));
            }
            _ => {}
        }
    }

    None
}

/// 切换二级候选卡片的展开与收起状态
///
/// # 设计原理
/// - 优先收起已展开的卡片；当前选中候选词不可展开时，自动在当前展示页内
///   聚焦首个可展开候选词并展开，保证 Tab 键始终有确定性的响应。
fn toggle_expanded_card(session: &mut ImeCoreSession) -> Option<ImeOutputAction> {
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

    Some(ImeOutputAction::Handled { update_ui: false })
}

/// 提交二级卡片中当前选中的子词条
///
/// # 设计原理
/// - 无论子词条是否命中都先收起卡片，避免卡片残留于展开态遮挡后续候选；
///   命中子词条时按自定义文本上屏并同步登记标点上下文。
fn commit_expanded_card_selection(session: &mut ImeCoreSession) -> ImeOutputAction {
    let sel = session.selected_global_index();
    let sub_idx = session.card_selected_index();
    session.collapse_card();

    if let Some(cand) = session.engine.candidates().get(sel)
        && let Some(sub_text) = cand.sub_candidates.get(sub_idx).cloned()
    {
        let (committed, _) = session.engine.commit_custom_text(&sub_text);
        session.reset_viewport();
        session.punctuation_processor.record_commit(&committed);
        return ImeOutputAction::Commit(committed);
    }
    ImeOutputAction::Handled { update_ui: true }
}
