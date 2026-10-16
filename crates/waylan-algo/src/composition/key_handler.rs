//! 拼音组词按键状态机与交互决策
//!
//! # 设计原理
//! - **实现初衷**：负责将键盘输入的按键（如翻页、光标编辑、二级展开、数字选词、空格确认）
//!   映射为输入法组词状态的下一步动作（[`KeyProcessAction`]）。
//! - **核心优势**：纯领域逻辑，与特定显示服务协议（Wayland、IBus 等）完全解耦，提供跨平台统一的拼音交互体验。
//! - **代价与局限**：对部分双功能快捷键（如 Shift 标点与 V 模式）需要依赖外部传入修饰键状态。

use waylan_core::input::constants::{
    KEY_APOSTROPHE, KEY_BACKSPACE, KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_KPENTER, KEY_LEFT,
    KEY_RIGHT, KEY_SPACE, KEY_TAB, KEY_UP,
};
use waylan_core::input::conversion::{keycode_to_digit, keycode_to_v_mode_char};
use waylan_core::input::page_keys::PageKeyConfig;

use crate::preprocessor::PunctuationProcessor;
use crate::AlgoEngine;

/// 核心按键决策结果
///
/// # 设计原理
/// - **实现初衷**：解耦按键消费决策与具体的输入协议（Wayland zwp_input_method 与 IBus 等），
///   将拼音预编辑与候选词选择的通用状态机决策收敛至领域核心层。
/// - **核心优势**：消除不同协议实现之间高度重合的按键分支代码，防止逻辑分叉。
#[derive(Debug, PartialEq, Eq)]
pub enum KeyProcessAction {
    /// 原样透传给客户端应用
    PassThrough,
    /// 内部消费，刷新候选表面（无需提交）
    Handled,
    /// 提交整段文字上屏，并隐藏候选表面与清空预编辑
    Commit(String),
    /// 提交部分文字，并保留/刷新剩余拼音预编辑
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 清空预编辑并重置状态（如 ESC）
    Reset,
    /// 唤出大网格候选面板
    ExpandGrid,
}

/// 拼音组词状态按键决策输入参数
#[derive(Debug, Clone, Copy)]
pub struct CompositionKeyParams {
    pub key: u32,
    pub shift_active: bool,
    pub page_keys: PageKeyConfig,
    pub now: u64,
}

/// 处理二级展开卡片导航（Tab 键展开）
fn handle_expanded_tab_key(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
) -> Option<KeyProcessAction> {
    if key == KEY_TAB {
        let sel = engine.selected_index;
        engine.toggle_expand(sel);
        return Some(KeyProcessAction::Handled);
    }

    if engine.is_expanded {
        let sel = engine.selected_index;
        if key == KEY_DOWN {
            engine.next_expanded_item(sel);
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_UP {
            engine.prev_expanded_item(sel);
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_ENTER || key == KEY_KPENTER || key == KEY_SPACE {
            if let Some((committed, _)) =
                engine.select_expanded_item(sel, engine.expanded_selected_index)
            {
                punct_processor.record_commit(&committed);
                return Some(KeyProcessAction::Commit(committed));
            }
            return Some(KeyProcessAction::Handled);
        }
    }
    None
}

/// 处理智能上下文流转方向键导航（现代输入流转模型）
///
/// # 设计原理
/// - **实现初衷**：在横排候选框视觉直觉（左右选词、上下翻页）与拼音字母长句纠错（左右移动光标）之间实现无缝自适应流转。
/// - **选词模式（光标在末尾）**：
///   - `KEY_RIGHT`：向右移动候选词高亮，到达页末时自动翻下一页；
///   - `KEY_LEFT`：向左移动候选词高亮；若已在第 1 页第 1 候选词，再次按左键触发边缘溢出，进入拼音光标编辑模式；
///     若在非第 1 页第 1 候选词，则翻回上一页末尾；
///   - `KEY_DOWN`：翻下一页；若已在末页或无法翻页，唤出大网格候选面板；
///   - `KEY_UP`：翻上一页；
/// - **拼音光标编辑模式（光标在拼音内部）**：
///   - `KEY_LEFT`：光标向左移动一个字符；
///   - `KEY_RIGHT`：光标向右移动一个字符，移到末尾时自动恢复为选词模式；
///   - `KEY_DOWN`：翻下一页；末页时展开大网格；
///   - `KEY_UP`：翻上一页。
fn handle_smart_navigation_key(engine: &mut AlgoEngine, key: u32) -> Option<KeyProcessAction> {
    let is_cursor_at_end = engine.cursor() >= engine.raw_input().len();

    match key {
        KEY_RIGHT => {
            if is_cursor_at_end {
                let cand_len = engine.candidates().len();
                if cand_len > 0 {
                    if engine.selected_index + 1 < cand_len {
                        engine.selected_index += 1;
                    } else if engine.can_next_page() {
                        engine.next_page();
                    }
                }
            } else {
                engine.move_cursor_right();
            }
            Some(KeyProcessAction::Handled)
        }
        KEY_LEFT => {
            if is_cursor_at_end {
                if engine.selected_index > 0 {
                    engine.selected_index -= 1;
                } else if engine.can_prev_page() {
                    engine.prev_page();
                    engine.selected_index = engine.candidates().len().saturating_sub(1);
                } else {
                    engine.move_cursor_left();
                }
            } else {
                engine.move_cursor_left();
            }
            Some(KeyProcessAction::Handled)
        }
        KEY_DOWN => {
            if engine.can_next_page() {
                engine.next_page();
                Some(KeyProcessAction::Handled)
            } else {
                Some(KeyProcessAction::ExpandGrid)
            }
        }
        KEY_UP => {
            if engine.can_prev_page() {
                engine.prev_page();
            }
            Some(KeyProcessAction::Handled)
        }
        _ => None,
    }
}

/// 选取指定索引候选词并转换为协议决策枚举
fn select_candidate_action(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    cand_idx: usize,
    now: u64,
) -> KeyProcessAction {
    if let Some((committed_text, remaining_len)) = engine.select_candidate(cand_idx, now) {
        punct_processor.record_commit(&committed_text);
        if remaining_len > 0 {
            KeyProcessAction::CommitPartial {
                committed: committed_text,
                remaining_preedit: engine.formatted_preedit(),
            }
        } else {
            KeyProcessAction::Commit(committed_text)
        }
    } else {
        KeyProcessAction::Handled
    }
}

/// 处理 V模式字符输入
fn handle_v_mode_key(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
) -> Option<KeyProcessAction> {
    if engine.raw_input().starts_with('v') {
        if let Some(ch) = keycode_to_v_mode_char(key, shift_active) {
            engine.push_char(ch);
            return Some(KeyProcessAction::Handled);
        }
    }
    None
}

/// 处理数字按键选词（支持一级候选与二级展开卡片）
fn handle_digit_selection(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    shift_active: bool,
    now: u64,
) -> Option<KeyProcessAction> {
    if shift_active || engine.raw_input().starts_with('v') {
        return None;
    }

    let digit = keycode_to_digit(key)?;
    if engine.is_expanded {
        let sel = engine.selected_index;
        let sub_idx = digit.saturating_sub(1);
        if let Some((committed, _)) = engine.select_expanded_item(sel, sub_idx) {
            punct_processor.record_commit(&committed);
            return Some(KeyProcessAction::Commit(committed));
        }
        Some(KeyProcessAction::Handled)
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
fn handle_action_keys(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    now: u64,
) -> Option<KeyProcessAction> {
    if key == KEY_SPACE {
        if engine.cursor() < engine.raw_input().len() {
            engine.move_cursor_to_end();
            return Some(KeyProcessAction::Handled);
        }
        let sel = engine.selected_index;
        return Some(select_candidate_action(engine, punct_processor, sel, now));
    }
    if key == KEY_ENTER || key == KEY_KPENTER {
        let raw = engine.raw_input().to_string();
        engine.reset();
        return Some(KeyProcessAction::Commit(raw));
    }
    if key == KEY_ESC {
        if engine.is_expanded {
            engine.is_expanded = false;
            engine.expanded_selected_index = 0;
            return Some(KeyProcessAction::Handled);
        }
        engine.reset();
        return Some(KeyProcessAction::Reset);
    }
    None
}

/// 处理退格删除与分词单引号
fn handle_editing_keys(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
) -> Option<KeyProcessAction> {
    if key == KEY_BACKSPACE || key == KEY_DELETE {
        if key == KEY_BACKSPACE {
            engine.pop_char();
        } else {
            engine.delete_char_at_cursor();
        }
        return Some(if engine.raw_input().is_empty() {
            KeyProcessAction::Reset
        } else {
            KeyProcessAction::Handled
        });
    }
    if !shift_active && key == KEY_APOSTROPHE {
        engine.push_char('\'');
        return Some(KeyProcessAction::Handled);
    }
    None
}

/// 处理候选词翻页快捷键
fn handle_page_flip_keys(
    engine: &mut AlgoEngine,
    key: u32,
    shift_active: bool,
    page_keys: PageKeyConfig,
) -> Option<KeyProcessAction> {
    if !shift_active && (page_keys.is_prev_page(key) || page_keys.is_next_page(key)) {
        if page_keys.is_prev_page(key) {
            engine.prev_page();
        } else {
            engine.next_page();
        }
        return Some(KeyProcessAction::Handled);
    }
    None
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

    if let Some(act) = handle_expanded_tab_key(engine, punct_processor, key) {
        return Some(act);
    }

    if !engine.is_expanded {
        if let Some(act) = handle_smart_navigation_key(engine, key) {
            return Some(act);
        }
    }

    if let Some(act) = handle_v_mode_key(engine, key, shift_active) {
        return Some(act);
    }

    if let Some(act) = handle_digit_selection(engine, punct_processor, key, shift_active, now) {
        return Some(act);
    }

    if let Some(act) = handle_action_keys(engine, punct_processor, key, now) {
        return Some(act);
    }

    if let Some(act) = handle_editing_keys(engine, key, shift_active) {
        return Some(act);
    }

    handle_page_flip_keys(engine, key, shift_active, page_keys)
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
