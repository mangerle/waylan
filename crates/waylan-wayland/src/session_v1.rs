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
pub const KEY_0: u32 = 11;
pub const KEY_GRAVE: u32 = 41;
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_RIGHTCTRL: u32 = 97;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_RIGHTALT: u32 = 100;

/// 标准 XKB 修饰键位掩码常量
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 64;

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

/// 将主键盘数字键扫描码转换为对应的 ASCII 数字字符 ('0'..='9')
pub fn keycode_to_ascii_digit_char(keycode: u32) -> Option<char> {
    match keycode {
        KEY_1 => Some('1'),
        KEY_2 => Some('2'),
        KEY_3 => Some('3'),
        KEY_4 => Some('4'),
        KEY_5 => Some('5'),
        KEY_6 => Some('6'),
        KEY_7 => Some('7'),
        KEY_8 => Some('8'),
        KEY_9 => Some('9'),
        KEY_0 => Some('0'),
        _ => None,
    }
}

/// 将扫描码转换为常见中文标点符号字符（未按 Shift）
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

/// 结合 Shift 修饰键状态将扫描码转换为中文标点源字符（支持 !?$^_()""<>{}:~ 等 Shift 标点）
pub fn keycode_to_punctuation_with_shift(keycode: u32, shift_pressed: bool) -> Option<char> {
    if shift_pressed {
        match keycode {
            KEY_1 => Some('!'),
            KEY_4 => Some('$'),
            KEY_6 => Some('^'),
            KEY_9 => Some('('),
            KEY_0 => Some(')'),
            KEY_MINUS => Some('_'),
            KEY_LEFTBRACE => Some('{'),
            KEY_RIGHTBRACE => Some('}'),
            KEY_SEMICOLON => Some(':'),
            KEY_APOSTROPHE => Some('"'),
            KEY_GRAVE => Some('~'),
            KEY_COMMA => Some('<'),
            KEY_DOT => Some('>'),
            KEY_SLASH => Some('?'),
            _ => None,
        }
    } else {
        keycode_to_punctuation(keycode)
    }
}

/// 候选词翻页快捷键模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageKeyConfig {
    /// 减号与等号 (- / =)
    #[default]
    MinusEqual,
    /// 左右中括号 ([ / ])
    Bracket,
    /// 逗号与句号 (, / .)
    CommaPeriod,
}

/// 判断按键是否为向上翻页键
pub fn is_prev_page_key(keycode: u32, page_keys: PageKeyConfig) -> bool {
    if keycode == KEY_PAGEUP {
        return true;
    }
    match page_keys {
        PageKeyConfig::MinusEqual => keycode == KEY_MINUS,
        PageKeyConfig::Bracket => keycode == KEY_LEFTBRACE,
        PageKeyConfig::CommaPeriod => keycode == KEY_COMMA,
    }
}

/// 判断按键是否为向下翻页键
pub fn is_next_page_key(keycode: u32, page_keys: PageKeyConfig) -> bool {
    if keycode == KEY_PAGEDOWN {
        return true;
    }
    match page_keys {
        PageKeyConfig::MinusEqual => keycode == KEY_EQUAL,
        PageKeyConfig::Bracket => keycode == KEY_RIGHTBRACE,
        PageKeyConfig::CommaPeriod => keycode == KEY_DOT,
    }
}

/// 微澜输入法会话状态机
pub struct WaylanV1Session {
    /// 拼音算法引擎核心
    pub engine: AlgoEngine,
    /// 标点符号智能处理器
    pub punctuation_processor: PunctuationProcessor,
    /// Shift/Ctrl 防误触状态机
    pub shift_sm: ShiftToggleStateMachine,
    /// 候选词翻页快捷键模式
    pub page_keys: PageKeyConfig,
    /// 候选框悬浮图层管理器
    pub surface_manager: crate::surface::CandidateSurfaceManager,
    /// 当前输入语言模式
    pub mode: InputMode,
    /// 与外部系统托盘共享的中英文状态标志
    pub shared_is_chinese: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    pub reload_requested: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<Box<dyn FnMut(&mut Self) + Send>>,
    /// 监控外部进程修改的配置文件与词库路径列表（用于在焦点或按键事件时自动感知变更）
    pub watched_paths: Vec<std::path::PathBuf>,
    /// 上次记录的被监控文件最新修改时间戳
    pub last_watched_mtime: Option<std::time::SystemTime>,
    /// 当前激活的应用上下文引用
    pub active_context: Option<ZwpInputMethodContextV1>,
    /// 捕获的键盘对象
    pub grabbed_keyboard: Option<WlKeyboard>,
    /// 关联的共享内存管理器句柄
    pub shm: Option<wayland_client::protocol::wl_shm::WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<wayland_client::protocol::wl_surface::WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前上下文递增序号
    pub current_serial: u32,
    /// 当前按下的修饰键位掩码（XKB mods_depressed）
    pub mods_depressed: u32,
}

impl Default for WaylanV1Session {
    fn default() -> Self {
        Self {
            engine: AlgoEngine::with_sample_data(AlgoConfig::default()),
            punctuation_processor: PunctuationProcessor::new(),
            shift_sm: ShiftToggleStateMachine::new(),
            page_keys: PageKeyConfig::default(),
            surface_manager: crate::surface::CandidateSurfaceManager::new(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            active_context: None,
            grabbed_keyboard: None,
            shm: None,
            popup_wl_surface: None,
            active_buffer: None,
            qhandle: None,
            current_serial: 0,
            mods_depressed: 0,
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
            page_keys: PageKeyConfig::default(),
            surface_manager: crate::surface::CandidateSurfaceManager::new(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            active_context: None,
            grabbed_keyboard: None,
            shm: None,
            popup_wl_surface: None,
            active_buffer: None,
            qhandle: None,
            current_serial: 0,
            mods_depressed: 0,
        }
    }

    /// 根据按键按下/释放事件更新修饰键位掩码
    pub fn update_modifier_by_key(&mut self, keycode: u32, is_press: bool) {
        let mask = match keycode {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => MOD_SHIFT,
            KEY_LEFTCTRL | KEY_RIGHTCTRL => MOD_CTRL,
            KEY_LEFTALT | KEY_RIGHTALT => MOD_ALT,
            _ => 0,
        };
        if mask != 0 {
            if is_press {
                self.mods_depressed |= mask;
            } else {
                self.mods_depressed &= !mask;
            }
        }
    }

    /// 判断当前是否处于 Shift 按下状态
    pub fn is_shift_active(&self) -> bool {
        (self.mods_depressed & MOD_SHIFT) != 0
    }

    /// 判断当前是否按下了 Ctrl / Alt / Super 组合修饰键
    pub fn has_ctrl_or_alt(&self) -> bool {
        (self.mods_depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（同时自动感知配置文件修改时间戳变更）
    pub fn sync_external_state(&mut self) {
        use std::sync::atomic::Ordering;

        let mut should_reload = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::Relaxed))
            .unwrap_or(false);

        if !self.watched_paths.is_empty() {
            let mut latest_mtime: Option<std::time::SystemTime> = None;
            for p in &self.watched_paths {
                if let Ok(meta) = std::fs::metadata(p) {
                    if let Ok(mtime) = meta.modified() {
                        latest_mtime = Some(match latest_mtime {
                            Some(prev) => prev.max(mtime),
                            None => mtime,
                        });
                    }
                }
            }
            match (self.last_watched_mtime, latest_mtime) {
                (Some(old), Some(new)) if new > old => {
                    self.last_watched_mtime = Some(new);
                    should_reload = true;
                }
                (None, Some(new)) => {
                    self.last_watched_mtime = Some(new);
                }
                _ => {}
            }
        }

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
                self.engine.custom_phrases =
                    waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
                if let Some(ref wal_path) = self.engine.user_dict_path.clone() {
                    self.engine.set_user_dict_path(wal_path);
                }
            }
            info!("[微澜] 已完成用户词库与运行配置热重载。");
        }

        if let Some(ref zh_flag) = self.shared_is_chinese {
            let external_zh = zh_flag.load(Ordering::Relaxed);
            let current_zh = matches!(self.mode, InputMode::Chinese | InputMode::VMode);
            if external_zh != current_zh {
                self.mode = if external_zh {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
            }
        }
    }

    /// 构造当前候选框的渲染状态
    pub fn build_candidate_box_state(&self) -> waylan_core::CandidateBoxState {
        let candidates = self.engine.candidates();
        let core_candidates: Vec<waylan_core::Candidate> = candidates
            .iter()
            .map(|c| waylan_core::Candidate {
                text: c.text.clone(),
                pinyin: c.pinyin.clone(),
                score: c.score,
                is_expandable: c.is_expandable,
                sub_candidates: c.sub_candidates.clone(),
            })
            .collect();

        waylan_core::CandidateBoxState {
            visible: !self.engine.raw_input().is_empty(),
            preedit: self.engine.formatted_preedit(),
            candidates: core_candidates,
            selected_index: self.engine.selected_index,
            is_expanded: self.engine.is_expanded,
            expanded_selected_index: self.engine.expanded_selected_index,
            mode_badge: match self.mode {
                InputMode::Chinese => "中".to_string(),
                InputMode::English => "英".to_string(),
                InputMode::VMode => "V".to_string(),
            },
        }
    }

    /// 触发候选框悬浮表面位图重绘并提交至 Wayland 表面
    pub fn refresh_candidate_surface(&mut self) {
        let box_state = self.build_candidate_box_state();
        self.surface_manager.commit_to_wayland(
            &box_state,
            self.shm.as_ref(),
            self.popup_wl_surface.as_ref(),
            self.qhandle.as_ref(),
            &mut self.active_buffer,
        );
    }

    /// 隐藏候选框并卸载 Wayland 表面缓冲
    pub fn hide_candidate_surface(&mut self) {
        self.surface_manager
            .hide_wayland_surface(self.popup_wl_surface.as_ref(), &mut self.active_buffer);
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
                state.sync_external_state();
                // 捕获硬件键盘输入
                let keyboard = id.grab_keyboard(qhandle, ());
                state.grabbed_keyboard = Some(keyboard);
                state.active_context = Some(id);
                state.mods_depressed = 0;
                state.engine.reset();
            }
            MethodEvent::Deactivate { context } => {
                info!("[微澜] 收到失焦事件：用户已离开文本输入框。");
                context.destroy();
                state.active_context = None;
                state.grabbed_keyboard = None;
                state.mods_depressed = 0;
                state.engine.reset();
                state.hide_candidate_surface();
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
            state.hide_candidate_surface();
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
        let Some(context) = state.active_context.clone() else {
            return;
        };

        state.sync_external_state();

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

                state.update_modifier_by_key(key, is_press);

                // 1. 优先送入 Shift/Ctrl 防误触状态机
                if let Some(new_mode) = state.shift_sm.on_key_event(key, is_press, state.mode) {
                    state.mode = new_mode;
                    if let Some(ref zh_flag) = state.shared_is_chinese {
                        zh_flag.store(
                            matches!(new_mode, InputMode::Chinese | InputMode::VMode),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
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

                // 2. 如果当前处于英文模式，或按下了 Ctrl/Alt/Super 快捷键（如 Ctrl+C/V），直接转发原始按键给应用
                if state.mode == InputMode::English || state.has_ctrl_or_alt() {
                    context.key(serial, time, key, 1);
                    return;
                }

                // 3. 正在组合拼音状态下的专门按键拦截
                if !state.engine.raw_input().is_empty() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    // 3.1 Tab 键：切换/展开当前高亮候选词的二级动态格式卡片
                    if key == KEY_TAB {
                        let sel = state.engine.selected_index;
                        state.engine.toggle_expand(sel);
                        state.refresh_candidate_surface();
                        return;
                    }

                    // 3.2 处于展开二级状态下的导航与选择
                    if state.engine.is_expanded {
                        let sel = state.engine.selected_index;
                        if key == KEY_DOWN {
                            state.engine.next_expanded_item(sel);
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_UP {
                            state.engine.prev_expanded_item(sel);
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_ENTER || key == KEY_SPACE {
                            if let Some((committed, _)) = state
                                .engine
                                .select_expanded_item(sel, state.engine.expanded_selected_index)
                            {
                                state.punctuation_processor.record_commit(&committed);
                                state.hide_candidate_surface();
                                context.commit_string(serial, committed);
                                context.preedit_string(serial, String::new(), String::new());
                                return;
                            }
                        }
                    } else {
                        // 非展开态下，上下方向键在当前页候选词间循环移动高亮胶囊
                        if key == KEY_DOWN {
                            state.engine.next_candidate();
                            state.refresh_candidate_surface();
                            return;
                        }
                        if key == KEY_UP {
                            state.engine.prev_candidate();
                            state.refresh_candidate_surface();
                            return;
                        }
                    }

                    // 3.3 数字键 1-9（未按 Shift 时）：选择对应位候选词
                    if !state.is_shift_active() {
                        if let Some(digit) = keycode_to_digit(key) {
                            let cand_idx = digit.saturating_sub(1);
                            if let Some((committed_text, remaining_len)) =
                                state.engine.select_candidate(cand_idx, now)
                            {
                                state.punctuation_processor.record_commit(&committed_text);
                                context.commit_string(serial, committed_text);
                                if remaining_len > 0 {
                                    state.refresh_candidate_surface();
                                    context.preedit_string(serial, state.engine.formatted_preedit(), String::new());
                                } else {
                                    state.hide_candidate_surface();
                                    context.preedit_string(serial, String::new(), String::new());
                                }
                                return;
                            }
                        }
                    }

                    // 3.4 空格键：当前高亮候选词上屏
                    if key == KEY_SPACE {
                        let sel = state.engine.selected_index;
                        if let Some((committed_text, remaining_len)) =
                            state.engine.select_candidate(sel, now)
                        {
                            state.punctuation_processor.record_commit(&committed_text);
                            context.commit_string(serial, committed_text);
                            if remaining_len > 0 {
                                state.refresh_candidate_surface();
                                context.preedit_string(serial, state.engine.formatted_preedit(), String::new());
                            } else {
                                state.hide_candidate_surface();
                                context.preedit_string(serial, String::new(), String::new());
                            }
                            return;
                        }
                    }

                    // 3.5 回车键（Enter）：直接上屏原始未转换的英文字母
                    if key == KEY_ENTER {
                        let raw = state.engine.raw_input().to_string();
                        state.engine.reset();
                        state.hide_candidate_surface();
                        context.commit_string(serial, raw);
                        context.preedit_string(serial, String::new(), String::new());
                        return;
                    }

                    // 3.6 退格键（Backspace）：删除一个拼文字母
                    if key == KEY_BACKSPACE {
                        state.engine.pop_char();
                        let preedit = state.engine.formatted_preedit();
                        if preedit.is_empty() {
                            state.hide_candidate_surface();
                        } else {
                            state.refresh_candidate_surface();
                        }
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    // 3.7 候选词翻页（根据配置的翻页快捷键模式判定）
                    if !state.is_shift_active() && is_prev_page_key(key, state.page_keys) {
                        state.engine.prev_page();
                        state.refresh_candidate_surface();
                        return;
                    }
                    if !state.is_shift_active() && is_next_page_key(key, state.page_keys) {
                        state.engine.next_page();
                        state.refresh_candidate_surface();
                        return;
                    }

                    // 3.8 预编辑区内光标左移/右移
                    if key == KEY_LEFT {
                        state.engine.move_cursor_left();
                        let preedit = state.engine.formatted_preedit();
                        state.refresh_candidate_surface();
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }
                    if key == KEY_RIGHT {
                        state.engine.move_cursor_right();
                        let preedit = state.engine.formatted_preedit();
                        state.refresh_candidate_surface();
                        context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
                        context.preedit_string(serial, preedit, String::new());
                        return;
                    }

                    // 3.9 ESC 键：取消当前输入并清空
                    if key == KEY_ESC {
                        state.engine.reset();
                        state.hide_candidate_surface();
                        context.preedit_cursor(0);
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

                    // 中文模式下的智能标点符号拦截（含 Shift 组合标点，如 !?$^_()""<>{}:~）
                    if let Some(punct_char) =
                        keycode_to_punctuation_with_shift(key, state.is_shift_active())
                    {
                        if let Some(actions) =
                            state.punctuation_processor.process_punctuation(punct_char)
                        {
                            for action in actions {
                                match action {
                                    waylan_core::TextAction::Commit(text) => {
                                        context.commit_string(serial, text);
                                    }
                                    waylan_core::TextAction::MoveCursor(offset) => {
                                        for _ in 0..offset.unsigned_abs() {
                                            let dir_key = if offset < 0 { KEY_LEFT } else { KEY_RIGHT };
                                            context.key(serial, time, dir_key, 1);
                                            context.key(serial, time, dir_key, 0);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            return;
                        }
                    }

                    // 若为未按 Shift 的纯数字键输入，记录数字上下文以便后续触发 3.14 / 12:00 半角标点保护
                    if !state.is_shift_active() {
                        if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
                            state
                                .punctuation_processor
                                .record_commit(&digit_ch.to_string());
                            context.key(serial, time, key, 1);
                            return;
                        }
                    }
                }

                // 5. 字母键输入（A-Z）：进入拼音算法管线
                if let Some(ch) = keycode_to_char(key) {
                    state.engine.push_char(ch);
                    let preedit = state.engine.formatted_preedit();
                    state.refresh_candidate_surface();
                    context.preedit_cursor(state.engine.formatted_cursor_byte_offset() as i32);
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
                state.mods_depressed = mods_depressed;
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

// 为 wl_compositor 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_compositor::WlCompositor, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_compositor::WlCompositor,
        _event: <wayland_client::protocol::wl_compositor::WlCompositor as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_shm 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_shm::WlShm, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_shm::WlShm,
        _event: <wayland_client::protocol::wl_shm::WlShm as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_surface 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_surface::WlSurface, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_surface::WlSurface,
        _event: <wayland_client::protocol::wl_surface::WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_shm_pool 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_shm_pool::WlShmPool,
        _event: <wayland_client::protocol::wl_shm_pool::WlShmPool as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_buffer 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
        _event: <wayland_client::protocol::wl_buffer::WlBuffer as wayland_client::Proxy>::Event,
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
