//! 拼音组词按键状态机与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责将键盘输入的按键（如翻页、光标编辑、二级展开、数字选词、空格确认）
//!   映射为输入法组词状态的下一步动作（[`KeyProcessAction`]）。
//! - **核心优势**：纯领域逻辑，与特定显示服务协议（Wayland、IBus 等）完全解耦，提供跨平台统一的拼音交互体验。
//! - **代价与局限**：对部分双功能快捷键（如 Shift 标点与 V 模式）需要依赖外部传入修饰键状态。

mod expanded;
mod navigation;
mod selection;

use waylan_algo::preprocessor::PunctuationProcessor;
use waylan_algo::AlgoEngine;
use waylan_core::input::page_keys::PageKeyConfig;

pub use crate::session::{ImeOutputAction, KeyProcessAction};

/// 拼音组词状态按键决策输入参数
#[derive(Debug, Clone, Copy)]
pub struct CompositionKeyParams {
    pub key: u32,
    pub shift_active: bool,
    pub page_keys: PageKeyConfig,
    pub now: u64,
}

/// 使用收敛的参数结构体处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
pub fn handle_composition_key_with_params(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    params: CompositionKeyParams,
) -> Option<KeyProcessAction> {
    let CompositionKeyParams {
        key,
        shift_active,
        page_keys,
        now,
    } = params;

    if let Some(act) = expanded::handle_expanded_tab_key(engine, punct_processor, key) {
        return Some(act);
    }

    if !engine.is_expanded() {
        if let Some(act) = navigation::handle_smart_navigation_key(engine, key) {
            return Some(act);
        }
    }

    if let Some(act) = selection::handle_v_mode_key(engine, key, shift_active) {
        return Some(act);
    }

    if let Some(act) =
        selection::handle_digit_selection(engine, punct_processor, key, shift_active, now)
    {
        return Some(act);
    }

    if let Some(act) = selection::handle_action_keys(engine, punct_processor, key, now) {
        return Some(act);
    }

    if let Some(act) = selection::handle_editing_keys(engine, key, shift_active) {
        return Some(act);
    }

    navigation::handle_page_flip_keys(engine, key, shift_active, page_keys)
}

/// 处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
pub fn handle_composition_key(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    shift_active: bool,
    page_keys: PageKeyConfig,
    now: u64,
) -> Option<KeyProcessAction> {
    handle_composition_key_with_params(
        engine,
        punct_processor,
        CompositionKeyParams {
            key,
            shift_active,
            page_keys,
            now,
        },
    )
}
