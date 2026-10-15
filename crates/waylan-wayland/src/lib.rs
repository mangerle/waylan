pub mod error;
pub mod fs_watcher;
pub mod input_constants;
pub mod input_core;
pub mod keymap;
pub mod loop_wakeup;
pub mod protocol_v1;
pub mod runner;
pub mod session_common;
pub mod session_v1;
pub mod session_v2;
pub mod surface;

pub use error::WaylandRunnerError;
pub use runner::*;
pub use session_common::*;

pub use fs_watcher::InotifyWatcher;
pub use input_core::{
    keycode_to_ascii_digit_char, keycode_to_char, keycode_to_digit, keycode_to_punctuation,
    keycode_to_punctuation_with_shift, ModifierState, PageKeyConfig, KEY_APOSTROPHE, KEY_BACKSPACE,
    KEY_COMMA, KEY_DELETE, KEY_DOT, KEY_DOWN, KEY_ENTER, KEY_EQUAL, KEY_ESC, KEY_LEFT, KEY_LEFTALT,
    KEY_LEFTBRACE, KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_MINUS, KEY_PAGEDOWN, KEY_PAGEUP, KEY_RIGHT,
    KEY_RIGHTALT, KEY_RIGHTBRACE, KEY_RIGHTCTRL, KEY_RIGHTSHIFT, KEY_SEMICOLON, KEY_SLASH,
    KEY_SPACE, KEY_TAB, KEY_UP, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER,
};
pub use loop_wakeup::LoopWakeup;

use std::error::Error;
use std::fmt;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_seat::WlSeat,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

pub use session_v1::WaylanV1Session;
pub use session_v2::WaylanV2Session;

/// 合成器全局接口信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandGlobal {
    /// 接口名称 (如 "wl_compositor")
    pub interface: String,
    /// 接口版本号
    pub version: u32,
}

impl fmt::Display for WaylandGlobal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (版本 {})", self.interface, self.version)
    }
}

/// Wayland 合成器支持的特权能力汇总
#[derive(Debug, Default, Clone)]
pub struct WaylandCapabilities {
    /// 是否支持输入法协议 v1 (zwp_input_method_v1)
    pub has_input_method_v1: bool,
    /// 是否支持特权输入法管理器协议 (zwp_input_method_manager_v2)
    pub has_input_method_v2: bool,
    /// 是否支持无焦点置顶窗口协议 (zwlr_layer_shell_v1)
    pub has_layer_shell_v1: bool,
    /// 是否具有标准合成器接口 (wl_compositor)
    pub has_compositor: bool,
    /// 是否具有席位输入接口 (wl_seat)
    pub has_seat: bool,
    /// 合成器广播的全部全局接口信息列表
    pub all_globals: Vec<WaylandGlobal>,
}

/// Wayland 状态管理器
#[derive(Debug, Default)]
pub struct WaylandProbeState {
    pub capabilities: WaylandCapabilities,
}

// 为 wl_compositor 实现 Dispatch
impl Dispatch<WlCompositor, ()> for WaylandProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_seat 实现 Dispatch
impl Dispatch<WlSeat, ()> for WaylandProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        _event: <WlSeat as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 zwp_input_method_manager_v2 实现 Dispatch
impl Dispatch<ZwpInputMethodManagerV2, ()> for WaylandProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputMethodManagerV2,
        _event: <ZwpInputMethodManagerV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 zwlr_layer_shell_v1 实现 Dispatch
impl Dispatch<ZwlrLayerShellV1, ()> for WaylandProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_registry 实现 Dispatch
impl Dispatch<WlRegistry, GlobalListContents> for WaylandProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

/// 探测当前运行环境中的 Wayland 合成器协议能力
pub fn probe_wayland_environment() -> Result<WaylandCapabilities, Box<dyn Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let _qh = event_queue.handle();

    let mut state = WaylandProbeState::default();

    // 检查并记录所有广播的全局接口
    for global in globals.contents().clone_list() {
        state.capabilities.all_globals.push(WaylandGlobal {
            interface: global.interface.clone(),
            version: global.version,
        });

        if global.interface == "zwp_input_method_v1" {
            state.capabilities.has_input_method_v1 = true;
        }
        if global.interface == "zwp_input_method_manager_v2" {
            state.capabilities.has_input_method_v2 = true;
        }
        if global.interface == "zwlr_layer_shell_v1" {
            state.capabilities.has_layer_shell_v1 = true;
        }
        if global.interface == "wl_compositor" {
            state.capabilities.has_compositor = true;
        }
        if global.interface == "wl_seat" {
            state.capabilities.has_seat = true;
        }
    }

    // 派发一次事件队列，确认连接通信顺畅
    event_queue.roundtrip(&mut state)?;

    Ok(state.capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_default() {
        let caps = WaylandCapabilities::default();
        assert!(!caps.has_input_method_v2);
        assert!(!caps.has_layer_shell_v1);
    }
}
