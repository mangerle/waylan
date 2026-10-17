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

use waylan_core::input::page_keys::PageKeyConfig;

use crate::preprocessor::PunctuationProcessor;
use crate::AlgoEngine;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AlgoConfig;
    use waylan_core::input::constants::{
        KEY_0, KEY_2, KEY_8, KEY_APOSTROPHE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_KP1, KEY_KP5,
        KEY_KPENTER, KEY_LEFT, KEY_RIGHT, KEY_SPACE, KEY_UP,
    };

    #[test]
    fn test_handle_composition_key() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut punct = PunctuationProcessor::new();
        let page_keys = PageKeyConfig::default();

        engine.push_char('n');
        engine.push_char('i');
        assert_eq!(engine.raw_input(), "ni");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));

        let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));

        let act = handle_composition_key(&mut engine, &mut punct, KEY_ENTER, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Commit("ni".to_string())));
        assert!(engine.raw_input().is_empty());

        engine.push_char('x');
        engine.push_char('i');
        let act =
            handle_composition_key(&mut engine, &mut punct, KEY_APOSTROPHE, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.raw_input(), "xi'");
        engine.push_char('a');
        engine.push_char('n');
        assert_eq!(engine.raw_input(), "xi'an");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_ESC, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Reset));
        assert!(engine.raw_input().is_empty());

        engine.push_char('h');
        engine.push_char('a');
        let act = handle_composition_key(&mut engine, &mut punct, KEY_KPENTER, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Commit("ha".to_string())));
        assert!(engine.raw_input().is_empty());

        engine.push_char('n');
        engine.push_char('i');
        let act = handle_composition_key(&mut engine, &mut punct, KEY_KP1, false, page_keys, 0);
        assert!(matches!(act, Some(ImeOutputAction::Commit(_))));
        assert!(engine.raw_input().is_empty());
    }

    #[test]
    fn test_v_mode_composition_keys() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut punct = PunctuationProcessor::new();
        let page_keys = PageKeyConfig::default();

        engine.push_char('v');
        assert_eq!(engine.raw_input(), "v");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_2, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.raw_input(), "v2");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_0, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.raw_input(), "v20");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_8, true, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.raw_input(), "v20*");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_KP5, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.raw_input(), "v20*5");

        let cands = engine.candidates();
        assert!(!cands.is_empty());
        assert_eq!(cands[0].text, "100");

        let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Commit("100".to_string())));
        assert!(engine.raw_input().is_empty());
    }

    #[test]
    fn test_smart_navigation_selection_and_overflow_to_cursor_edit() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut punct = PunctuationProcessor::new();
        let page_keys = PageKeyConfig::default();

        for ch in "xian".chars() {
            engine.push_char(ch);
        }
        assert_eq!(engine.cursor(), 4);
        assert_eq!(engine.selected_index(), 0);
        assert!(engine.candidates().len() >= 2);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.selected_index(), 1);
        assert_eq!(engine.cursor(), 4);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.selected_index(), 0);
        assert_eq!(engine.cursor(), 4);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.cursor(), 3);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.cursor(), 2);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.cursor(), 3);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        assert_eq!(engine.cursor(), 4);
        assert_eq!(engine.selected_index(), 0);

        let act = handle_composition_key(&mut engine, &mut punct, KEY_SPACE, false, page_keys, 0);
        assert!(matches!(act, Some(ImeOutputAction::Commit(_))));
        assert!(engine.raw_input().is_empty());
    }

    #[test]
    fn test_smart_navigation_down_page_and_expand_grid() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut punct = PunctuationProcessor::new();
        let page_keys = PageKeyConfig::default();

        engine.push_char('n');
        engine.push_char('i');

        while engine.can_next_page() {
            let act =
                handle_composition_key(&mut engine, &mut punct, KEY_DOWN, false, page_keys, 0);
            assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        }

        let act = handle_composition_key(&mut engine, &mut punct, KEY_DOWN, false, page_keys, 0);
        assert_eq!(act, Some(ImeOutputAction::ExpandGrid));

        if engine.can_prev_page() {
            let act = handle_composition_key(&mut engine, &mut punct, KEY_UP, false, page_keys, 0);
            assert_eq!(act, Some(ImeOutputAction::Handled { update_ui: true }));
        }
    }
}
