use crate::surface::{CandidateSurfaceManager, CursorRect};
use log::info;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    Event as V2Event, ZwpInputMethodV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::{
    Event as PopupEvent, ZwpInputPopupSurfaceV2,
};

use waylan_core::{CandidateBoxState, InputMode, TextAction};
use crate::session_v1::{
    is_next_page_key, is_prev_page_key, keycode_to_ascii_digit_char, keycode_to_char,
    keycode_to_digit, keycode_to_punctuation_with_shift, PageKeyConfig, KEY_BACKSPACE, KEY_DOWN,
    KEY_ENTER, KEY_ESC, KEY_LEFT, KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_RIGHT,
    KEY_RIGHTALT, KEY_RIGHTCTRL, KEY_RIGHTSHIFT, KEY_SPACE, KEY_TAB, KEY_UP, MOD_ALT, MOD_CTRL,
    MOD_SHIFT, MOD_SUPER,
};

/// 微澜面向现代 Wayland 合成器 (KDE Plasma 6 / GNOME 46+ / Hyprland) 的 V2 会话状态
pub struct WaylanV2Session {
    /// 核心拼音算法引擎
    pub engine: AlgoEngine,
    /// 中英文状态机
    pub shift_state: ShiftToggleStateMachine,
    /// 智能标点符号处理器
    pub punctuation_processor: PunctuationProcessor,
    /// 候选词翻页快捷键模式
    pub page_keys: PageKeyConfig,
    /// 候选框透明悬浮 Surface 管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 当前是否处于激活会话
    pub is_active: bool,
    /// 关联的 zwp_input_method_v2 协议句柄
    pub input_method: Option<ZwpInputMethodV2>,
    /// 硬件键盘捕获句柄
    pub keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    /// 悬浮候选弹出层 Surface
    pub popup_surface: Option<ZwpInputPopupSurfaceV2>,
    /// 关联的共享内存管理器句柄
    pub shm: Option<wayland_client::protocol::wl_shm::WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<wayland_client::protocol::wl_surface::WlSurface>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前跟踪的光标坐标
    pub current_cursor: CursorRect,
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
    /// Done 事件累计序列号（用于 commit 请求原子同步）
    pub done_serial: u32,
    /// 当前按下的修饰键位掩码（XKB mods_depressed）
    pub mods_depressed: u32,
}

impl WaylanV2Session {
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_state: ShiftToggleStateMachine::new(),
            punctuation_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            surface_manager: CandidateSurfaceManager::new(),
            is_active: false,
            input_method: None,
            keyboard_grab: None,
            popup_surface: None,
            shm: None,
            popup_wl_surface: None,
            active_buffer: None,
            qhandle: None,
            current_cursor: CursorRect::default(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            done_serial: 0,
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
            info!("[Wayland v2] 已完成用户词库与运行配置热重载。");
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

    /// 设置关联的 V2 输入法实例
    pub fn set_input_method(&mut self, im: ZwpInputMethodV2) {
        self.input_method = Some(im);
    }

    /// 根据当前算法引擎与输入模式构建候选框渲染状态
    pub fn build_candidate_box_state(&self) -> CandidateBoxState {
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

        CandidateBoxState {
            visible: self.is_active && !self.engine.raw_input().is_empty(),
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

    /// 触发候选框悬浮表面重绘并提交至 Wayland 表面
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

    /// 提交指定文本上屏，并重置预编辑状态
    pub fn commit_text(&mut self, text: &str) {
        if let Some(im) = &self.input_method {
            im.commit_string(text.to_string());
            im.set_preedit_string(String::new(), 0, 0);
            im.commit(self.done_serial);
        }
        self.hide_candidate_surface();
    }

    /// 更新当前 preedit 预编辑字符串到目标客户端
    pub fn update_preedit(&mut self, preedit: &str) {
        if let Some(im) = &self.input_method {
            let cursor = if preedit.is_empty() {
                0
            } else {
                self.engine.formatted_cursor_byte_offset() as i32
            };
            im.set_preedit_string(preedit.to_string(), cursor, cursor);
            im.commit(self.done_serial);
        }
        if preedit.is_empty() {
            self.hide_candidate_surface();
        } else {
            self.refresh_candidate_surface();
        }
    }

    /// 分段确认部分文本上屏，并以剩余拼音继续推进预编辑与候选渲染
    pub fn commit_partial(&mut self, text: &str, remaining_preedit: &str) {
        if let Some(im) = &self.input_method {
            im.commit_string(text.to_string());
            let cursor = self.engine.formatted_cursor_byte_offset() as i32;
            im.set_preedit_string(remaining_preedit.to_string(), cursor, cursor);
            im.commit(self.done_serial);
        }
        self.refresh_candidate_surface();
    }

    /// 取消当前输入，重置引擎与预编辑区
    pub fn cancel_input(&mut self) {
        self.engine.reset();
        self.update_preedit("");
    }

    /// 处理单个硬件按键按下/释放事件（返回本次直接上屏的文本，便于无合成器环境下单元测试验证）
    pub fn handle_key_event(&mut self, key: u32, is_press: bool) -> Option<String> {
        self.update_modifier_by_key(key, is_press);

        // 1. 优先经由 Shift/Ctrl 防误触状态机判定
        if let Some(new_mode) = self.shift_state.on_key_event(key, is_press, self.mode) {
            self.mode = new_mode;
            if let Some(ref zh_flag) = self.shared_is_chinese {
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
            info!("[Wayland v2] 切换输入模式为：{}", mode_str);
            return None;
        }

        // 只在按键按下时处理字符与候选交互
        if !is_press || self.has_ctrl_or_alt() {
            return None;
        }

        // 2. 英文模式：直接将字母（含 Shift 大写）、数字、标点与空格提交上屏
        if self.mode == InputMode::English {
            if let Some(ch) = keycode_to_char(key) {
                let out = if self.is_shift_active() {
                    ch.to_ascii_uppercase().to_string()
                } else {
                    ch.to_string()
                };
                self.punctuation_processor.record_commit(&out);
                self.commit_text(&out);
                return Some(out);
            }
            if !self.is_shift_active() {
                if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
                    let out = digit_ch.to_string();
                    self.punctuation_processor.record_commit(&out);
                    self.commit_text(&out);
                    return Some(out);
                }
            }
            if let Some(punct_ch) = keycode_to_punctuation_with_shift(key, self.is_shift_active()) {
                let out = punct_ch.to_string();
                self.punctuation_processor.record_commit(&out);
                self.commit_text(&out);
                return Some(out);
            }
            if key == KEY_SPACE {
                self.commit_text(" ");
                return Some(" ".to_string());
            }
            return None;
        }

        // 3. 正在组合拼音状态下的专门按键拦截
        if !self.engine.raw_input().is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // 3.1 Tab 键：切换/展开当前高亮候选词的二级动态格式卡片
            if key == KEY_TAB {
                let sel = self.engine.selected_index;
                self.engine.toggle_expand(sel);
                self.refresh_candidate_surface();
                return None;
            }

            // 3.2 处于展开二级状态下的导航与选择
            if self.engine.is_expanded {
                let sel = self.engine.selected_index;
                if key == KEY_DOWN {
                    self.engine.next_expanded_item(sel);
                    self.refresh_candidate_surface();
                    return None;
                }
                if key == KEY_UP {
                    self.engine.prev_expanded_item(sel);
                    self.refresh_candidate_surface();
                    return None;
                }
                if key == KEY_ENTER || key == KEY_SPACE {
                    if let Some((committed, _)) = self
                        .engine
                        .select_expanded_item(sel, self.engine.expanded_selected_index)
                    {
                        self.punctuation_processor.record_commit(&committed);
                        self.commit_text(&committed);
                        return Some(committed);
                    }
                }
            } else {
                // 非展开态下，上下方向键在当前页候选词间循环移动高亮胶囊
                if key == KEY_DOWN {
                    self.engine.next_candidate();
                    self.refresh_candidate_surface();
                    return None;
                }
                if key == KEY_UP {
                    self.engine.prev_candidate();
                    self.refresh_candidate_surface();
                    return None;
                }
            }

            // 3.3 数字键 1-9（未按 Shift 时）：选择对应位候选词
            if !self.is_shift_active() {
                if let Some(digit) = keycode_to_digit(key) {
                    let cand_idx = digit.saturating_sub(1);
                    if let Some((committed_text, remaining_len)) =
                        self.engine.select_candidate(cand_idx, now)
                    {
                        self.punctuation_processor.record_commit(&committed_text);
                        if remaining_len > 0 {
                            let preedit = self.engine.formatted_preedit();
                            self.commit_partial(&committed_text, &preedit);
                        } else {
                            self.commit_text(&committed_text);
                        }
                        return Some(committed_text);
                    }
                }
            }

            // 3.4 空格键：当前高亮候选词上屏
            if key == KEY_SPACE {
                let sel = self.engine.selected_index;
                if let Some((committed_text, remaining_len)) =
                    self.engine.select_candidate(sel, now)
                {
                    self.punctuation_processor.record_commit(&committed_text);
                    if remaining_len > 0 {
                        let preedit = self.engine.formatted_preedit();
                        self.commit_partial(&committed_text, &preedit);
                    } else {
                        self.commit_text(&committed_text);
                    }
                    return Some(committed_text);
                }
            }

            // 3.5 回车键（Enter）：直接上屏原始拼音字母
            if key == KEY_ENTER {
                let raw = self.engine.raw_input().to_string();
                self.engine.reset();
                self.commit_text(&raw);
                return Some(raw);
            }

            // 3.6 退格键（Backspace）：删除一个拼文字母
            if key == KEY_BACKSPACE {
                self.engine.pop_char();
                let preedit = self.engine.formatted_preedit();
                self.update_preedit(&preedit);
                return None;
            }

            // 3.7 候选词翻页（根据配置的翻页快捷键模式判定）
            if !self.is_shift_active() && is_prev_page_key(key, self.page_keys) {
                self.engine.prev_page();
                self.refresh_candidate_surface();
                return None;
            }
            if !self.is_shift_active() && is_next_page_key(key, self.page_keys) {
                self.engine.next_page();
                self.refresh_candidate_surface();
                return None;
            }

            // 3.8 预编辑区内光标左移/右移
            if key == KEY_LEFT {
                self.engine.move_cursor_left();
                let preedit = self.engine.formatted_preedit();
                self.update_preedit(&preedit);
                return None;
            }
            if key == KEY_RIGHT {
                self.engine.move_cursor_right();
                let preedit = self.engine.formatted_preedit();
                self.update_preedit(&preedit);
                return None;
            }

            // 3.9 ESC 键：取消当前输入并清空
            if key == KEY_ESC {
                self.cancel_input();
                return None;
            }
        }

        // 4. 输入缓冲区为空时的中文标点符号、数字与空格上屏处理
        if self.engine.raw_input().is_empty() {
            if key == KEY_SPACE {
                self.commit_text(" ");
                return Some(" ".to_string());
            }

            if let Some(punct_char) =
                keycode_to_punctuation_with_shift(key, self.is_shift_active())
            {
                if let Some(actions) =
                    self.punctuation_processor.process_punctuation(punct_char)
                {
                    let mut committed_out = None;
                    for action in actions {
                        if let TextAction::Commit(text) = action {
                            self.commit_text(&text);
                            committed_out = Some(text);
                        }
                    }
                    return committed_out;
                }
            }

            if !self.is_shift_active() {
                if let Some(digit_ch) = keycode_to_ascii_digit_char(key) {
                    let s = digit_ch.to_string();
                    self.punctuation_processor.record_commit(&s);
                    self.commit_text(&s);
                    return Some(s);
                }
            }
        }

        // 5. 字母键输入（A-Z）：进入拼音算法管线
        if let Some(ch) = keycode_to_char(key) {
            self.engine.push_char(ch);
            let preedit = self.engine.formatted_preedit();
            self.update_preedit(&preedit);
        }

        None
    }
}

// 为 zwp_input_method_v2 实现事件分发
impl Dispatch<ZwpInputMethodV2, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        proxy: &ZwpInputMethodV2,
        event: V2Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            V2Event::Activate => {
                info!("[Wayland v2] 收到会话激活事件：输入焦点已建立。");
                state.sync_external_state();
                state.is_active = true;
                state.mods_depressed = 0;
                if state.keyboard_grab.is_none() {
                    let grab = proxy.grab_keyboard(qhandle, ());
                    state.keyboard_grab = Some(grab);
                }
                state.engine.reset();
            }
            V2Event::Deactivate => {
                info!("[Wayland v2] 收到会话失焦事件：输入焦点已离开。");
                state.is_active = false;
                state.mods_depressed = 0;
                if let Some(grab) = state.keyboard_grab.take() {
                    grab.release();
                }
                state.engine.reset();
                state.hide_candidate_surface();
            }
            V2Event::SurroundingText { text, cursor, anchor } => {
                info!(
                    "[Wayland v2] 上下文环境文本更新: 长度={}, 光标={}, 锚点={}",
                    text.len(), cursor, anchor
                );
            }
            V2Event::TextChangeCause { cause: _ } => {}
            V2Event::ContentType { hint: _, purpose: _ } => {}
            V2Event::Done => {
                // 批次状态同步已完成，原子序列号递增
                state.done_serial = state.done_serial.wrapping_add(1);
            }
            V2Event::Unavailable => {
                info!("[Wayland v2] 输入法 v2 协议不可用，正在自动降级兼容模式。");
                state.is_active = false;
            }
            _ => {}
        }
    }
}

// 为 zwp_input_popup_surface_v2 实现事件分发（接收光标位置）
impl Dispatch<ZwpInputPopupSurfaceV2, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputPopupSurfaceV2,
        event: PopupEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let PopupEvent::TextInputRectangle { x, y, width, height } = event {
            info!(
                "[Wayland v2 光标矩形] 捕获焦点光标变化: x={}, y={}, w={}, h={}",
                x, y, width, height
            );
            state.current_cursor = CursorRect::new(x, y, width, height);
            state.surface_manager.update_cursor_rect(x, y, width, height);
        }
    }
}

// 为 zwp_input_method_keyboard_grab_v2 实现事件分发
impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodKeyboardGrabV2,
        event: wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::Event as GrabEvent;
        state.sync_external_state();
        match event {
            GrabEvent::Keymap { format, fd, size } => {
                info!(
                    "[Wayland v2 键盘抓取] 收到键盘映射配置: format={:?}, 大小={} 字节",
                    format, size
                );
                let _ = fd;
            }
            GrabEvent::Key {
                serial: _,
                time: _,
                key,
                state: key_state,
            } => {
                let is_press = matches!(
                    key_state,
                    wayland_client::WEnum::Value(
                        wayland_client::protocol::wl_keyboard::KeyState::Pressed
                    )
                );
                state.handle_key_event(key, is_press);
            }
            GrabEvent::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched: _,
                mods_locked: _,
                group: _,
            } => {
                state.mods_depressed = mods_depressed;
                info!(
                    "[Wayland v2 键盘抓取] 收到修饰键变更: depressed={}",
                    mods_depressed
                );
            }
            GrabEvent::RepeatInfo { rate, delay } => {
                info!(
                    "[Wayland v2 键盘抓取] 收到按键连击设置: rate={}, delay={}",
                    rate, delay
                );
            }
            _ => {}
        }
    }
}

// 为 wl_seat 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()> for WaylanV2Session {
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

// 为 zwp_input_method_manager_v2 实现 Dispatch
impl
    Dispatch<
        wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        (),
    > for WaylanV2Session
{
    fn event(
        _state: &mut Self,
        _proxy: &wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _event: <wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_compositor 实现 Dispatch
impl Dispatch<wayland_client::protocol::wl_compositor::WlCompositor, ()> for WaylanV2Session {
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
impl Dispatch<wayland_client::protocol::wl_shm::WlShm, ()> for WaylanV2Session {
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
impl Dispatch<wayland_client::protocol::wl_surface::WlSurface, ()> for WaylanV2Session {
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
impl Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> for WaylanV2Session {
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
impl Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> for WaylanV2Session {
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
    > for WaylanV2Session
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_v1::{KEY_1, KEY_4, KEY_9, KEY_APOSTROPHE, KEY_SLASH};
    use waylan_algo::types::AlgoConfig;

    #[test]
    fn test_v2_session_initialization() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let session = WaylanV2Session::new(engine);

        assert!(!session.is_active);
        assert_eq!(session.mode, InputMode::Chinese);
        assert_eq!(session.done_serial, 0);
        assert!(session.keyboard_grab.is_none());
        assert!(session.input_method.is_none());

        let box_state = session.build_candidate_box_state();
        assert!(!box_state.visible);
        assert_eq!(box_state.mode_badge, "中");
    }

    #[test]
    fn test_v2_session_preedit_and_cancel() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut session = WaylanV2Session::new(engine);
        session.is_active = true;

        session.engine.push_char('n');
        session.engine.push_char('i');

        let box_state = session.build_candidate_box_state();
        assert!(box_state.visible);
        assert!(!box_state.candidates.is_empty());
        assert_eq!(box_state.candidates[0].text, "你");

        session.cancel_input();
        let box_state_after = session.build_candidate_box_state();
        assert!(!box_state_after.visible);
        assert!(session.engine.raw_input().is_empty());
    }

    #[test]
    fn test_shift_punctuation_and_modifier_tracking() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut session = WaylanV2Session::new(engine);

        assert_eq!(keycode_to_punctuation_with_shift(KEY_1, true), Some('!'));
        assert_eq!(keycode_to_punctuation_with_shift(KEY_4, true), Some('$'));
        assert_eq!(keycode_to_punctuation_with_shift(KEY_9, true), Some('('));
        assert_eq!(
            keycode_to_punctuation_with_shift(KEY_APOSTROPHE, true),
            Some('"')
        );
        assert_eq!(keycode_to_punctuation_with_shift(KEY_SLASH, true), Some('?'));
        assert_eq!(
            keycode_to_punctuation_with_shift(KEY_SLASH, false),
            Some('/')
        );

        session.update_modifier_by_key(KEY_LEFTSHIFT, true);
        assert!(session.is_shift_active());
        assert!(!session.has_ctrl_or_alt());
        session.update_modifier_by_key(KEY_LEFTSHIFT, false);
        assert!(!session.is_shift_active());

        session.update_modifier_by_key(KEY_LEFTCTRL, true);
        assert!(session.has_ctrl_or_alt());
        session.update_modifier_by_key(KEY_LEFTCTRL, false);
        assert!(!session.has_ctrl_or_alt());
    }

    #[test]
    fn test_candidate_selection_navigation() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut session = WaylanV2Session::new(engine);
        session.is_active = true;

        for ch in "xian".chars() {
            session.engine.push_char(ch);
        }
        assert!(session.engine.candidates().len() >= 2);

        assert_eq!(session.build_candidate_box_state().selected_index, 0);
        session.engine.next_candidate();
        assert_eq!(session.build_candidate_box_state().selected_index, 1);
        session.engine.prev_candidate();
        assert_eq!(session.build_candidate_box_state().selected_index, 0);
    }

    #[test]
    fn test_formatted_cursor_byte_offset_sync() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "women".chars() {
            engine.push_char(ch);
        }
        assert_eq!(engine.formatted_preedit(), "wo'men");
        // 光标在末尾："wo'men|" -> 6
        assert_eq!(engine.formatted_cursor_byte_offset(), 6);

        // 左移 3 次后位于 'o' 之后："wo|'men" -> 2
        engine.move_cursor_left();
        engine.move_cursor_left();
        engine.move_cursor_left();
        assert_eq!(engine.formatted_cursor_byte_offset(), 2);

        // 再右移 1 次位于 'm' 之后："wo'm|en" -> 4
        engine.move_cursor_right();
        assert_eq!(engine.formatted_cursor_byte_offset(), 4);
    }

    #[test]
    fn test_v2_english_mode_and_empty_buffer_commits() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut session = WaylanV2Session::new(engine);

        // 1. 中文模式空缓冲区：数字键 1 (KEY_1) 与空格键 (KEY_SPACE) 应直接上屏
        assert_eq!(session.handle_key_event(KEY_1, true), Some("1".to_string()));
        session.handle_key_event(KEY_1, false);
        assert_eq!(session.handle_key_event(KEY_SPACE, true), Some(" ".to_string()));
        session.handle_key_event(KEY_SPACE, false);

        // 2. 切换至英文模式：普通字母键 a (keycode 30)、Shift+a、数字及标点直接以英文上屏
        session.mode = InputMode::English;
        assert_eq!(session.handle_key_event(30, true), Some("a".to_string()));
        session.handle_key_event(30, false);

        session.handle_key_event(KEY_LEFTSHIFT, true);
        assert_eq!(session.handle_key_event(30, true), Some("A".to_string()));
        session.handle_key_event(30, false);
        assert_eq!(session.handle_key_event(KEY_1, true), Some("!".to_string()));
        session.handle_key_event(KEY_1, false);
        session.handle_key_event(KEY_LEFTSHIFT, false);
    }

    #[test]
    fn test_watched_paths_mtime_triggers_reload() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut session = WaylanV2Session::new(engine);

        let temp_file = std::env::temp_dir().join("waylan_test_mtime_watch.toml");
        let _ = std::fs::write(&temp_file, "page_size = 7\n");

        session.watched_paths = vec![temp_file.clone()];
        session.reload_callback = Some(Box::new(|s| {
            s.engine.config.page_size = 9;
        }));

        // 首次同步仅记录基准时间戳，不触发重载
        session.sync_external_state();
        assert_eq!(session.engine.config.page_size, 7);

        // 模拟外部进程将历史时间戳退后，使得当前文件时间戳更新于基准时间戳
        session.last_watched_mtime =
            Some(std::time::SystemTime::now() - std::time::Duration::from_secs(60));
        session.sync_external_state();
        assert_eq!(
            session.engine.config.page_size, 9,
            "检测到被监控文件修改时间更新后应自动触发 reload_callback"
        );

        let _ = std::fs::remove_file(&temp_file);
    }
}
