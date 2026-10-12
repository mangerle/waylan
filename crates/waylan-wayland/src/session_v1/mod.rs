use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::SyncSender,
    Arc,
};
use std::time::SystemTime;

use log::{debug, info};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;
use waylan_core::{Candidate, CandidateBoxState, ExpandedCardState, InputMode};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_keyboard::WlKeyboard, wl_shm::WlShm, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use crate::protocol_v1::input_method_v1::zwp_input_method_context_v1::{
    Event as ContextEvent, ZwpInputMethodContextV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_method_v1::{
    Event as MethodEvent, ZwpInputMethodV1,
};
use crate::protocol_v1::input_method_v1::zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1;
use crate::surface::CandidateSurfaceManager;

pub use crate::input_core::*;

/// V1 会话配置与词库热重载回调函数类型
pub type V1ReloadCallback = Box<dyn FnMut(&mut WaylanV1Session) + Send>;

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
    pub surface_manager: CandidateSurfaceManager,
    /// 当前输入语言模式
    pub mode: InputMode,
    /// 与外部系统托盘共享的中英文状态标志
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    /// 键盘切换中英文时唤醒系统托盘刷新图标的通知发送端
    pub mode_notify_tx: Option<SyncSender<()>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
    /// 外部注入的配置与词库热重载回调函数
    pub reload_callback: Option<V1ReloadCallback>,
    /// 监控外部进程修改的配置文件与词库路径列表（用于在焦点或按键事件时自动感知变更）
    pub watched_paths: Vec<PathBuf>,
    /// 上次记录的被监控文件最新修改时间戳
    pub last_watched_mtime: Option<SystemTime>,
    /// 当前激活的应用上下文引用
    pub active_context: Option<ZwpInputMethodContextV1>,
    /// 捕获的键盘对象
    pub grabbed_keyboard: Option<WlKeyboard>,
    /// 关联的共享内存管理器句柄
    pub shm: Option<WlShm>,
    /// 候选框渲染目标 Wayland 表面
    pub popup_wl_surface: Option<WlSurface>,
    /// V1 输入面板表面角色对象 (zwp_input_panel_surface_v1)
    pub panel_surface: Option<ZwpInputPanelSurfaceV1>,
    /// WLR Layer Shell 表面角色对象（用于 Sway / wlroots 等环境）
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    /// 当前挂载在表面上的活跃像素缓冲区
    pub active_buffer: Option<WlBuffer>,
    /// Wayland 事件队列句柄
    pub qhandle: Option<QueueHandle<Self>>,
    /// 当前硬件按键事件序号
    pub current_serial: u32,
    /// 文本输入上下文状态同步序号（由 commit_state 事件更新，用于 commit_string 与 preedit_string）
    pub commit_serial: u32,
    /// 当前按下的修饰键位掩码（XKB mods_depressed）
    pub mods_depressed: u32,
    /// 左右物理修饰键独立按下位掩码
    pub phys_mods: u8,
    /// 按键按下与释放对称消费跟踪器
    pub consumed_keys: ConsumedKeyTracker,
    /// 物理按键与动态键位布局映射表（支持 Dvorak / Colemak 等动态布局）
    pub keymap_layout: KeymapLayout,
    /// 与外部系统托盘共享的优雅退出请求标志
    pub exit_requested: Option<Arc<AtomicBool>>,
}

impl Default for WaylanV1Session {
    fn default() -> Self {
        Self {
            engine: AlgoEngine::with_sample_data(AlgoConfig::default()),
            punctuation_processor: PunctuationProcessor::new(),
            shift_sm: ShiftToggleStateMachine::new(),
            page_keys: PageKeyConfig::default(),
            surface_manager: CandidateSurfaceManager::new(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            mode_notify_tx: None,
            reload_requested: None,
            reload_callback: None,
            watched_paths: Vec::new(),
            last_watched_mtime: None,
            active_context: None,
            grabbed_keyboard: None,
            shm: None,
            popup_wl_surface: None,
            panel_surface: None,
            layer_surface: None,
            active_buffer: None,
            qhandle: None,
            current_serial: 0,
            commit_serial: 0,
            mods_depressed: 0,
            phys_mods: 0,
            consumed_keys: ConsumedKeyTracker::new(),
            keymap_layout: KeymapLayout::default(),
            exit_requested: None,
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
            ..Self::default()
        }
    }

    /// 根据按键按下/释放事件更新修饰键位掩码
    pub fn update_modifier_by_key(&mut self, keycode: u32, is_press: bool) {
        update_modifier_masks(
            &mut self.mods_depressed,
            &mut self.phys_mods,
            keycode,
            is_press,
        );
    }

    /// 判断当前是否处于 Shift 按下状态
    #[inline]
    pub fn is_shift_active(&self) -> bool {
        ModifierState {
            depressed: self.mods_depressed,
            phys_mods: self.phys_mods,
        }
        .is_shift_active()
    }

    /// 判断当前是否按下了 Ctrl / Alt / Super 组合修饰键
    #[inline]
    pub fn has_ctrl_or_alt(&self) -> bool {
        ModifierState {
            depressed: self.mods_depressed,
            phys_mods: self.phys_mods,
        }
        .has_ctrl_or_alt()
    }

    /// 同步外部系统托盘的中英文状态与热重载请求（同时自动感知配置文件修改时间戳变更）
    pub fn sync_external_state(&mut self) {
        let mut should_reload = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::Relaxed))
            .unwrap_or(false);

        let changed =
            check_watched_paths_changed(&self.watched_paths, &mut self.last_watched_mtime);
        if changed {
            should_reload = true;
        }

        if should_reload {
            if let Some(mut cb) = self.reload_callback.take() {
                cb(self);
                self.reload_callback = Some(cb);
            } else {
                let phrase_path = CustomPhraseManager::default_path();
                self.engine.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
                let wal_opt = self.engine.user_dict_path.clone();
                if let Some(ref wal_path) = wal_opt {
                    self.engine.set_user_dict_path(wal_path);
                }
            }
            info!("[微澜] 已完成用户词库与运行配置热重载。");
        }

        self.sync_mode_from_shared_flag();
    }

    /// 按键热路径轻量同步：仅检查原子标志位，避免每次敲击按键触发磁盘文件 metadata 系统调用
    pub fn sync_atomic_state(&mut self) {
        let requested = self
            .reload_requested
            .as_ref()
            .map(|flag| flag.load(Ordering::Acquire))
            .unwrap_or(false);
        if requested {
            self.sync_external_state();
            return;
        }

        self.sync_mode_from_shared_flag();
    }

    /// 从共享原子标志同步输入模式，若在拼音组合期间切至英文则立即清理残留预编辑与候选框
    fn sync_mode_from_shared_flag(&mut self) {
        if let Some(ref zh_flag) = self.shared_is_chinese {
            let external_zh = zh_flag.load(Ordering::Acquire);
            let current_zh = matches!(self.mode, InputMode::Chinese | InputMode::VMode);
            if external_zh != current_zh {
                self.mode = if external_zh {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
                if self.mode == InputMode::English && !self.engine.raw_input().is_empty() {
                    self.engine.reset();
                    self.hide_candidate_surface();
                    if let Some(ref ctx) = self.active_context {
                        ctx.preedit_cursor(0);
                        ctx.preedit_string(self.commit_serial, String::new(), String::new());
                    }
                }
            }
        }
    }

    /// 构造当前候选框的渲染状态
    pub fn build_candidate_box_state(&self) -> CandidateBoxState {
        let candidates = self.engine.candidates();
        let core_candidates: Vec<Candidate> = candidates
            .iter()
            .map(|c| Candidate {
                text: c.text.clone(),
                pinyin: c.pinyin.clone(),
                score: c.score,
                is_expandable: c.is_expandable,
                sub_candidates: c.sub_candidates.clone(),
            })
            .collect();

        let expanded_card = if self.engine.is_expanded {
            ExpandedCardState::Expanded {
                selected_index: self.engine.expanded_selected_index,
            }
        } else {
            ExpandedCardState::Collapsed
        };

        CandidateBoxState {
            visible: !self.engine.raw_input().is_empty(),
            preedit: self.engine.formatted_preedit(),
            candidates: core_candidates,
            selected_index: self.engine.selected_index,
            expanded_card,
            mode_badge: match self.mode {
                InputMode::Chinese => "中".to_string(),
                InputMode::English => "英".to_string(),
                InputMode::VMode => "V".to_string(),
            },
            is_grid_expanded: false,
            grid_candidates: Vec::new(),
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

        if let Some(ref layer_surf) = self.layer_surface {
            let scale = self.surface_manager.scale.max(1) as u32;
            let (phys_w, phys_h) = self.surface_manager.current_size;
            let logical_w = (phys_w / scale).max(1);
            let logical_h = (phys_h / scale).max(1);
            let (screen_w, screen_h) = self.surface_manager.screen_size;
            let (pos_x, pos_y) = self
                .surface_manager
                .cursor_rect
                .compute_popup_position_constrained(logical_w, logical_h, screen_w, screen_h);
            layer_surf.set_size(logical_w, logical_h);
            layer_surf.set_margin(pos_y.max(0), 0, 0, pos_x.max(0));
            if let Some(ref surf) = self.popup_wl_surface {
                surf.commit();
            }
        }
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
                debug!("[微澜] 收到焦点激活事件：用户已进入文本输入框。");
                state.sync_external_state();
                if let Some(old_kb) = state.grabbed_keyboard.take() {
                    if old_kb.version() >= 3 {
                        old_kb.release();
                    }
                }
                if let Some(old_ctx) = state.active_context.take() {
                    old_ctx.destroy();
                }
                // 捕获硬件键盘输入
                let keyboard = id.grab_keyboard(qhandle, ());
                state.grabbed_keyboard = Some(keyboard);
                state.active_context = Some(id);
                state.mods_depressed = 0;
                state.phys_mods = 0;
                state.consumed_keys.clear();
                state.shift_sm.reset();
                state.punctuation_processor.reset();
                state.engine.reset();
            }
            MethodEvent::Deactivate { context } => {
                debug!("[微澜] 收到失焦事件：用户已离开文本输入框。");
                let is_current = state.active_context.as_ref() == Some(&context);
                if is_current {
                    if let Some(active_ctx) = state.active_context.take() {
                        active_ctx.destroy();
                    }
                    if let Some(old_kb) = state.grabbed_keyboard.take() {
                        if old_kb.version() >= 3 {
                            old_kb.release();
                        }
                    }
                    state.mods_depressed = 0;
                    state.phys_mods = 0;
                    state.consumed_keys.clear();
                    state.shift_sm.reset();
                    state.punctuation_processor.reset();
                    state.engine.reset();
                    state.hide_candidate_surface();
                    let _ = state.engine.maybe_compact_user_dict();
                }
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
        match event {
            ContextEvent::CommitState { serial } => {
                state.commit_serial = serial;
            }
            ContextEvent::Reset => {
                state.shift_sm.reset();
                state.punctuation_processor.reset();
                state.engine.reset();
                state.hide_candidate_surface();
            }
            _ => {}
        }
    }
}

mod dispatch;
mod keyboard;

#[cfg(test)]
mod session_v1_tests;
