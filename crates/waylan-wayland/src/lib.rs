pub mod protocol_v1;
pub mod session_v1;
pub mod session_v2;
pub mod surface;

use protocol_v1::input_method_v1::zwp_input_panel_v1::ZwpInputPanelV1;
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};

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
    /// 合成器广播的全部全局接口名称列表
    pub all_globals: Vec<String>,
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
        _event: <WlCompositor as wayland_client::Proxy>::Event,
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
        _event: <WlSeat as wayland_client::Proxy>::Event,
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
        _event: <ZwpInputMethodManagerV2 as wayland_client::Proxy>::Event,
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
        _event: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
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
    > for WaylandProbeState
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

/// 探测当前运行环境中的 Wayland 合成器协议能力
pub fn probe_wayland_environment() -> Result<WaylandCapabilities, Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = wayland_client::globals::registry_queue_init(&conn)?;
    let _qh = event_queue.handle();

    let mut state = WaylandProbeState::default();

    // 检查并记录所有广播的全局接口
    for global in globals.contents().clone_list() {
        state
            .capabilities
            .all_globals
            .push(format!("{} (版本 {})", global.interface, global.version));

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

/// 启动并运行 Wayland V1 输入法事件循环（使用已初始化的会话实例）
pub fn run_session_v1_loop(
    mut session: session_v1::WaylanV1Session,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = wayland_client::globals::registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let mut im_global = None;
    for g in globals.contents().clone_list() {
        if g.interface == "zwp_input_method_v1" {
            im_global = Some(g);
            break;
        }
    }

    let Some(_global_info) = im_global else {
        return Err("当前 Wayland 合成器未暴露 zwp_input_method_v1 接口".into());
    };

    let _input_method: protocol_v1::input_method_v1::zwp_input_method_v1::ZwpInputMethodV1 =
        globals.bind(&qh, 1..=1, ())?;

    if let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()) {
        let wl_surface = compositor.create_surface(&qh, ());

        // 优先尝试绑定原生的 zwp_input_panel_v1 输入法浮动面板协议赋予 overlay_panel 角色
        if let Ok(input_panel) = globals.bind::<ZwpInputPanelV1, _, _>(&qh, 1..=1, ()) {
            let panel_surface = input_panel.get_input_panel_surface(&wl_surface, &qh, ());
            panel_surface.set_overlay_panel();
            session.panel_surface = Some(panel_surface);
            log::info!("[微澜 Wayland] 成功绑定 zwp_input_panel_v1 并将候选框配置为光标悬浮面板(overlay_panel)。");
        } else if let Ok(layer_shell) = globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ()) {
            let layer_surface = layer_shell.get_layer_surface(
                &wl_surface,
                None,
                Layer::Overlay,
                "waylan-candidate-box".to_string(),
                &qh,
                (),
            );
            layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer_surface.set_anchor(Anchor::Top | Anchor::Left);
            wl_surface.commit();
            session.layer_surface = Some(layer_surface);
            log::info!(
                "[微澜 Wayland] 成功绑定 zwlr_layer_shell_v1 并将候选框配置为 Overlay 图层表面。"
            );
        } else {
            log::warn!("[微澜 Wayland] 合成器未提供 zwp_input_panel_v1 或 zwlr_layer_shell_v1，裸 wl_surface 可能无法正常显示。");
        }

        session.popup_wl_surface = Some(wl_surface);
    }
    if let Ok(shm) = globals.bind::<wayland_client::protocol::wl_shm::WlShm, _, _>(&qh, 1..=1, ()) {
        session.shm = Some(shm);
    }
    session.qhandle = Some(qh.clone());

    log::info!("[微澜 Wayland] 成功绑定 zwp_input_method_v1 接口，进入输入法事件分发循环。");

    loop {
        event_queue.blocking_dispatch(&mut session)?;
    }
}

/// 启动并运行现代 Wayland V2 输入法事件循环（若协议不可用或发生错误，返回错误同时移交 session 实例以便降级）
pub fn run_session_v2_loop_fallible(
    mut session: session_v2::WaylanV2Session,
) -> Result<(), (Box<dyn std::error::Error>, Box<session_v2::WaylanV2Session>)> {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let (globals, mut event_queue) = match wayland_client::globals::registry_queue_init(&conn) {
        Ok(res) => res,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let qh = event_queue.handle();

    let seat: WlSeat = match globals.bind(&qh, 1..=7, ()) {
        Ok(s) => s,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let im_manager: ZwpInputMethodManagerV2 = match globals.bind(&qh, 1..=1, ()) {
        Ok(m) => m,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let input_method = im_manager.get_input_method(&seat, &qh, ());

    if let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()) {
        let wl_surface = compositor.create_surface(&qh, ());
        let popup_surface = input_method.get_input_popup_surface(&wl_surface, &qh, ());
        session.popup_wl_surface = Some(wl_surface);
        session.popup_surface = Some(popup_surface);
    }
    if let Ok(shm) = globals.bind::<wayland_client::protocol::wl_shm::WlShm, _, _>(&qh, 1..=1, ()) {
        session.shm = Some(shm);
    }
    session.qhandle = Some(qh.clone());
    session.set_input_method(input_method);

    if let Ok(vk_mgr) = globals.bind::<wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, _, _>(&qh, 1..=1, ()) {
        let vk = vk_mgr.create_virtual_keyboard(&seat, &qh, ());
        session.virtual_keyboard = Some(vk);
        log::info!("[微澜 Wayland] 成功绑定 zwp_virtual_keyboard_v1，支持虚拟键盘事件回退。");
    }

    log::info!(
        "[微澜 Wayland] 成功绑定 zwp_input_method_manager_v2、wl_compositor 与 wl_shm，进入 V2 输入法事件分发循环。"
    );

    loop {
        if let Err(e) = event_queue.blocking_dispatch(&mut session) {
            return Err((e.into(), Box::new(session)));
        }
        if session.is_unavailable {
            log::warn!("[微澜 Wayland] zwp_input_method_v2 收到 Unavailable 事件，跳出 V2 事件循环以准备降级。");
            return Err((
                "zwp_input_method_v2 协议已被合成器宣告不可用 (Unavailable)".into(),
                Box::new(session),
            ));
        }
    }
}

/// 启动并运行现代 Wayland V2 输入法事件循环（适用于 KDE Plasma 6 / GNOME / Hyprland / Sway）
pub fn run_session_v2_loop(
    session: session_v2::WaylanV2Session,
) -> Result<(), Box<dyn std::error::Error>> {
    run_session_v2_loop_fallible(session).map_err(|(e, _)| e)
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
