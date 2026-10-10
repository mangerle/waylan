use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::{
    Event as ContextEvent, ZwpInputMethodContextV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_method_v1::{
    Event as MethodEvent, ZwpInputMethodV1,
};
use log::info;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;
use waylan_core::InputMode;
use wayland_client::protocol::wl_keyboard::{Event as KeyboardEvent, WlKeyboard};
use wayland_client::{Connection, Dispatch, QueueHandle};

/// Linux 常见按键键码常量（基于 linux/input-event-codes.h）
pub const KEY_ESC: u32 = 1;
pub const KEY_MINUS: u32 = 12;
pub const KEY_EQUAL: u32 = 13;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_SPACE: u32 = 57;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_RIGHTSHIFT: u32 = 54;
pub const KEY_UP: u32 = 103;
pub const KEY_PAGEUP: u32 = 104;
pub const KEY_LEFT: u32 = 105;
pub const KEY_RIGHT: u32 = 106;
pub const KEY_DOWN: u32 = 108;
pub const KEY_PAGEDOWN: u32 = 109;
pub const KEY_SLASH: u32 = 53;
pub const KEY_COMMA: u32 = 51;
pub const KEY_DOT: u32 = 52;
pub const KEY_SEMICOLON: u32 = 39;
pub const KEY_APOSTROPHE: u32 = 40;
pub const KEY_LEFTBRACE: u32 = 26;
pub const KEY_RIGHTBRACE: u32 = 27;

pub const KEY_1: u32 = 2;
pub const KEY_2: u32 = 3;
pub const KEY_3: u32 = 4;
pub const KEY_4: u32 = 5;
pub const KEY_5: u32 = 6;
pub const KEY_6: u32 = 7;
pub const KEY_7: u32 = 8;
pub const KEY_8: u32 = 9;
pub const KEY_9: u32 = 10;

/// 将 Linux 扫描码转换为 ASCII 小写英文字母
pub fn keycode_to_char(keycode: u32) -> Option<char> {
    match keycode {
        30 => Some('a'),
        48 => Some('b'),
        46 => Some('c'),
        32 => Some('d'),
        18 => Some('e'),
        33 => Some('f'),
        34 => Some('g'),
        35 => Some('h'),
        23 => Some('i'),
        36 => Some('j'),
        37 => Some('k'),
        38 => Some('l'),
        50 => Some('m'),
        49 => Some('n'),
        24 => Some('o'),
        25 => Some('p'),
        16 => Some('q'),
        19 => Some('r'),
        31 => Some('s'),
        20 => Some('t'),
        22 => Some('u'),
        47 => Some('v'),
        17 => Some('w'),
        45 => Some('x'),
        21 => Some('y'),
        44 => Some('z'),
        _ => None,
    }
}

/// 将扫描码转换为 1-indexed 选词序号
pub fn keycode_to_digit(keycode: u32) -> Option<usize> {
    match keycode {
        KEY_1 => Some(1),
        KEY_2 => Some(2),
        KEY_3 => Some(3),
        KEY_4 => Some(4),
        KEY_5 => Some(5),
        KEY_6 => Some(6),
        KEY_7 => Some(7),
        KEY_8 => Some(8),
        KEY_9 => Some(9),
        _ => None,
    }
}

/// 将扫描码转换为常见中文标点符号字符
pub fn keycode_to_punctuation(keycode: u32) -> Option<char> {
    match keycode {
        KEY_COMMA => Some(','),
        KEY_DOT => Some('.'),
        KEY_SLASH => Some('/'),
        KEY_SEMICOLON => Some(';'),
        KEY_APOSTROPHE => Some('\''),
        KEY_LEFTBRACE => Some('['),
        KEY_RIGHTBRACE => Some(']'),
        _ => None,
    }
}

/// 微澜输入法会话状态机
pub struct WaylanV1Session {
    /// 拼音算法引擎核心
    pub engine: AlgoEngine,
    /// 标点符号智能处理器
    pub punctuation_processor: PunctuationProcessor,
    /// Shift 防误触状态机
    pub shift_sm: ShiftToggleStateMachine,
    /// 候选框悬浮图层管理器
    pub surface_manager: crate::surface::CandidateSurfaceManager,
    /// 当前输入语言模式
    pub mode: InputMode,
    /// 当前激活的应用上下文引用
    pub active_context: Option<ZwpInputMethodContextV1>,
    /// 捕获的键盘对象
    pub grabbed_keyboard: Option<WlKeyboard>,
    /// 当前上下文递增序号
    pub current_serial: u32,
}

impl Default for WaylanV1Session {
    fn default() -> Self {
        Self {
            engine: AlgoEngine::with_sample_data(AlgoConfig::default()),
            punctuation_processor: PunctuationProcessor::new(),
            shift_sm: ShiftToggleStateMachine::new(),
            surface_manager: crate::surface::CandidateSurfaceManager::new(),
            mode: InputMode::Chinese,
            active_context: None,
            grabbed_keyboard: None,
            current_serial: 0,
        }
    }
}

impl WaylanV1Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用外部预加载并配置好的算法引擎构造输入法会话
    pub fn with_engine(engine: AlgoEngine) -> Self {
        Self {
            engine,
            punctuation_processor: PunctuationProcessor::new(),
            shift_sm: ShiftToggleStateMachine::new(),
            surface_manager: crate::surface::CandidateSurfaceManager::new(),
            mode: InputMode::Chinese,
            active_context: None,
            grabbed_keyboard: None,
            current_serial: 0,
        }
    }
}

// 为 zwp_input_method_v1 实现事件分发
impl Dispatch<ZwpInputMethodV1, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodV1,
        event: MethodEvent,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            MethodEvent::Activate { id } => {
                info!("[微澜] 收到焦点激活事件：用户已进入文本输入框。");
                // 捕获硬件键盘输入
                let keyboard = id.grab_keyboard(qhandle, ());
                state.grabbed_keyboard = Some(keyboard);
                state.active_context = Some(id);
                state.engine.reset();
            }
            MethodEvent::Deactivate { context } => {
                info!("[微澜] 收到失焦事件：用户已离开文本输入框。");
                context.destroy();
                state.active_context = None;
                state.grabbed_keyboard = None;
                state.engine.reset();
            }
        }
    }
}

// 为 zwp_input_method_context_v1 实现生命周期分发
impl Dispatch<ZwpInputMethodContextV1, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodContextV1,
        event: ContextEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let ContextEvent::Reset = event {
            state.engine.reset();
        }
    }
}

// 为通过 grab_keyboard 捕获的 WlKeyboard 实现按键拦截与处理
impl Dispatch<WlKeyboard, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: KeyboardEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let Some(context) = state.active_context.as_ref() else {
            return;
        };

        match event {
            KeyboardEvent::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                state.current_serial = serial;
                let is_press = matches!(
                    key_state,
                    wayland_client::WEnum::Value(
                        wayland_client::protocol::wl_keyboard::KeyState::Pressed
                    )
                );

                // 1. 优先送入 Shift 防误触状态机
                if let Some(new_mode) = state.shift_sm.on_key_event(key, is_press, state.mode) {
                    state.mode = new_mode;
                    let mode_str = match new_mode {
                        InputMode::Chinese => "中文",
                        InputMode::English => "英文",
                        InputMode::VMode => "V模式",
                    };
                    info!("[微澜] 切换输入模式为：{}", mode_str);
                    return;
                }

                // 只在按下时处理输入，松开阶段默认透传
                if !is_press {
                    context.key(serial, time, key, 0);
                    return;
                }

                // 2. 如果当前处于英文模式，直接转发原始按键给应用
                if state.mode == InputMode::English {
                    context.key(serial, time, key, 1);
                    return;
                }

                // 3. 正在组合拼音状态下的专门按键拦截
                if !state.engine.raw_input().is_empty() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    // 3.1 Tab 键或方向键下：切换/展开二级动态格式卡片
                    if (key == KEY_TAB || (key == KEY_DOWN && !state.engine.is_expanded))
                        && state.engine.toggle_expand(0) {
                            return;
                        }

                    // 3.2 处于展开二级状态下的导航与选择
                    if state.engine.is_expanded {
                        if key == KEY_DOWN {
                            state.engine.next_expanded_item(0);
                            return;
                        }
                        if key == KEY_UP {
                            state.engine.prev_expanded_item(0);
                            return;
                        }
                        if key == KEY_ENTER || key == KEY_SPACE {
                            if let Some((committed, _)) =
                                state.engine.select_expanded_item(0, state.engine.expanded_selected_index)
                            {
                                state.punctuation_processor.record_commit(&committed);
                                context.commit_string(serial, committed);
                                context.preedit_string(serial, String::new(), String::new());
                                return;
                            }
                        }
                    }

                    // 3.3 数字键 1-9：选择对应位候选词
                    if let Some(digit) = keycode_to_digit(key) {
                        let cand_idx = digit.saturating_sub(1);
                        if let Some((committed_text, remaining_len)) =
                            state.engine.select_candidate(cand_idx, now)
                        {
                            state.punctuation_processor.record_commit(&committed_text);
                            context.commit_string(serial, committed_text);
                            let preedit = if remaining_len > 0 {
                                state.engine.formatted_preedit()
                            } else {
                                String::new()
                            };
                            context.preedit_string(serial, preedit, String::new());
                            return;
                        }
                    }

                    // 3.4 空格键：首选项上屏
                    if key == KEY_SPACE {
                        if let Some((committed_text, remaining_len)) =
                            state.engine.select_candidate(0, now)
                        {
                            state.punctuation_processor.record_commit(&committed_text);
                            context.commit_string(serial, committed_text);
                            let preedit = if remaining_len > 0 {
                                state.engine.formatted_preedit()
                            } else {
                                String::new()
                            };
                            context.preedit_string(serial, preedit, String::new());
                            return;
                        }
                    }

                    // 3.5 回车键（Enter）：直接上屏原始未转换的英文字母
                    if key == KEY_ENTER {
                        let raw = state.engine.raw_input().to_string();
                        state.engine.reset();
                        state.surface_manager.on_input_inactive();
                        context.commit_string(serial, raw);
                        context.preedit_string(serial, String::new(), String::new());
                        return;
                    }

                    // 3.6 退格键（Backspace）：删除一个拼文字母
                    if key == KEY_BACKSPACE {
                        state.engine.pop_char();
                        let preedit = state.engine.formatted_preedit();
                        if preedit.is_empty() {
                            state.surface_manager.on_input_inactive();
                        }
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    // 3.7 候选词翻页（PageUp/PageDown, 减号/等号, 方括号）
                    if key == KEY_PAGEUP || key == KEY_MINUS || key == KEY_LEFTBRACE {
                        state.engine.prev_page();
                        return;
                    }
                    if key == KEY_PAGEDOWN || key == KEY_EQUAL || key == KEY_RIGHTBRACE {
                        state.engine.next_page();
                        return;
                    }

                    // 3.8 预编辑区内光标左移/右移
                    if key == KEY_LEFT {
                        state.engine.move_cursor_left();
                        let preedit = state.engine.formatted_preedit();
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }
                    if key == KEY_RIGHT {
                        state.engine.move_cursor_right();
                        let preedit = state.engine.formatted_preedit();
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    // 3.9 ESC 键：取消当前输入并清空
                    if key == KEY_ESC {
                        state.engine.reset();
                        state.surface_manager.on_input_inactive();
                        context.preedit_string(serial, String::new(), String::new());
                        return;
                    }
                }

                // 4. 输入缓冲区为空时的按键逻辑
                if state.engine.raw_input().is_empty() {
                    // 退格键原样透传
                    if key == KEY_BACKSPACE {
                        context.key(serial, time, key, 1);
                        return;
                    }

                    // 空格键原样透传
                    if key == KEY_SPACE {
                        context.key(serial, time, key, 1);
                        return;
                    }

                    // 中文模式下的智能标点符号拦截
                    if let Some(punct_char) = keycode_to_punctuation(key) {
                        if let Some(actions) =
                            state.punctuation_processor.process_punctuation(punct_char)
                        {
                            for action in actions {
                                match action {
                                    waylan_core::TextAction::Commit(text) => {
                                        context.commit_string(serial, text);
                                    }
                                    waylan_core::TextAction::MoveCursor(_) => {
                                        // 光标左移已包含在双括号或引号文本中
                                    }
                                    _ => {}
                                }
                            }
                            return;
                        }
                    }
                }

                // 5. 字母键输入（A-Z）：进入拼音算法管线
                if let Some(ch) = keycode_to_char(key) {
                    state.engine.push_char(ch);
                    let preedit = state.engine.formatted_preedit();
                    state.surface_manager.on_input_active(320, 42);
                    context.preedit_string(serial, preedit, String::new());
                    return;
                }

                // 其他未拦截按键，原样透传给应用
                context.key(serial, time, key, 1);
            }
            KeyboardEvent::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                // 原样透传修饰键状态给当前文本应用
                context.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
    }
}

// 为 wl_seat 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_seat::WlSeat,
        _event: <wayland_client::protocol::wl_seat::WlSeat as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_registry 实现 Dispatch
impl
    Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for WaylanV1Session
{
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_registry::WlRegistry,
        _event: <wayland_client::protocol::wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _data: &wayland_client::globals::GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}
