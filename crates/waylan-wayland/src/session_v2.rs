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

/// 微澜面向现代 Wayland 合成器 (KDE Plasma 6 / GNOME 46+ / Hyprland) 的 V2 会话状态
pub struct WaylanV2Session {
    /// 核心拼音算法引擎
    pub engine: AlgoEngine,
    /// 中英文状态机
    pub shift_state: ShiftToggleStateMachine,
    /// 智能标点符号处理器
    pub punctuation_processor: PunctuationProcessor,
    /// 候选框透明悬浮 Surface 管理器
    pub surface_manager: CandidateSurfaceManager,
    /// 当前是否处于激活会话
    pub is_active: bool,
    /// 硬件键盘捕获句柄
    pub keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    /// 悬浮候选弹出层 Surface
    pub popup_surface: Option<ZwpInputPopupSurfaceV2>,
    /// 当前跟踪的光标坐标
    pub current_cursor: CursorRect,
}

impl WaylanV2Session {
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_state: ShiftToggleStateMachine::new(),
            punctuation_processor: PunctuationProcessor::new(),
            surface_manager: CandidateSurfaceManager::new(),
            is_active: false,
            keyboard_grab: None,
            popup_surface: None,
            current_cursor: CursorRect::default(),
        }
    }
}

// 为 zwp_input_method_v2 实现事件分发
impl Dispatch<ZwpInputMethodV2, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodV2,
        event: V2Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            V2Event::Activate => {
                info!("[Wayland v2] 收到会话激活事件：输入焦点已建立。");
                state.is_active = true;
                state.engine.reset();
            }
            V2Event::Deactivate => {
                info!("[Wayland v2] 收到会话失焦事件：输入焦点已离开。");
                state.is_active = false;
                state.engine.reset();
                state.surface_manager.on_input_inactive();
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
                // 批次状态同步已完成
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
        _state: &mut Self,
        _proxy: &ZwpInputMethodKeyboardGrabV2,
        _event: <ZwpInputMethodKeyboardGrabV2 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}
