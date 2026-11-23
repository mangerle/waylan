//! 拼音组词按键状态机与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责将键盘输入的按键（如翻页、光标编辑、二级展开、数字选词、空格确认）
//!   映射为输入法组词状态的下一步动作（[`ImeOutputAction`]）。
//! - **核心优势**：纯领域逻辑，与特定显示服务协议（Wayland、IBus 等）完全解耦，提供跨平台统一的拼音交互体验。
//! - **代价与局限**：对部分双功能快捷键（如 Shift 标点与 V 模式）需要依赖外部传入修饰键状态。

mod expanded;
mod navigation;
mod selection;

use waylan_core::input::page_keys::PageKeyConfig;

use crate::session::ImeCoreSession;
pub use crate::session::ImeOutputAction;

/// 拼音组词状态按键决策输入参数
#[derive(Debug, Clone, Copy)]
pub struct CompositionKeyParams {
    pub key: u32,
    pub shift_active: bool,
    pub ctrl_active: bool,
    pub page_keys: PageKeyConfig,
    pub now: u64,
}

/// 使用收敛的参数结构体处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
///
/// # 设计原理
/// - 按键优先级自上而下严格固定：二级展开卡片 -> 智能方向键导航 -> 选词/编辑/翻页类按键，
///   高优先级分支一旦命中即返回，杜绝同一次按键被多层状态机重复消费。
pub fn handle_composition_key_with_params(
    session: &mut ImeCoreSession,
    params: CompositionKeyParams,
) -> Option<ImeOutputAction> {
    // 1. 二级展开卡片优先拦截（Tab 键展开/收起与卡片内部导航）
    if let Some(act) = expanded::handle_expanded_tab_key(session, params.key) {
        return Some(act);
    }

    // 2. 未展开卡片时才允许智能方向键导航接管按键
    if !session.is_card_expanded()
        && let Some(act) = navigation::handle_smart_navigation_key(session, params.key)
    {
        return Some(act);
    }

    // 3. 选词、编辑与翻页类按键决策
    handle_composition_selection_keys(session, params)
}

/// 选词、编辑与翻页类按键的顺序决策
///
/// # 设计原理
/// - 判定顺序固定为：V 模式字符 -> 数字选词 -> 确认/取消动作键 -> 退格与分词编辑 -> 翻页快捷键；
///   命中后按动作语义决定是否收起二级卡片（文本上屏与状态重置必然收起）。
fn handle_composition_selection_keys(
    session: &mut ImeCoreSession,
    params: CompositionKeyParams,
) -> Option<ImeOutputAction> {
    let CompositionKeyParams {
        key,
        shift_active,
        ctrl_active: _,
        page_keys,
        now,
    } = params;

    if let Some(act) = selection::handle_forget_candidate_key(session, params) {
        session.collapse_card();
        return Some(act);
    }

    if let Some(act) = selection::handle_v_mode_key(session, key, shift_active) {
        session.collapse_card();
        return Some(act);
    }

    if let Some(act) = selection::handle_digit_selection(session, key, shift_active, now) {
        if matches!(act, ImeOutputAction::Commit(_)) {
            session.collapse_card();
        }
        return Some(act);
    }

    if let Some(act) = selection::handle_action_keys(session, key, now) {
        if matches!(act, ImeOutputAction::Commit(_) | ImeOutputAction::Reset) {
            session.collapse_card();
        }
        return Some(act);
    }

    if let Some(act) = selection::handle_editing_keys(session, key, shift_active) {
        session.collapse_card();
        return Some(act);
    }

    if let Some(act) = navigation::handle_page_flip_keys(session, key, shift_active, page_keys) {
        session.collapse_card();
        return Some(act);
    }

    None
}
