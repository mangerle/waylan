use crate::impl_common_wayland_dispatch;
use crate::protocol_v1::input_method_v1::zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1;
use crate::protocol_v1::input_method_v1::zwp_input_panel_v1::ZwpInputPanelV1;
use crate::session_v1::WaylanV1Session;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Event as LayerSurfaceEvent, ZwlrLayerSurfaceV1,
};

// 展开 7 组通用的基础 Wayland Dispatch trait (wl_seat, wl_compositor, wl_shm, wl_surface, wl_shm_pool, wl_buffer, wl_registry)
impl_common_wayland_dispatch!(WaylanV1Session);

// 为 zwp_input_panel_v1 实现 Dispatch
impl Dispatch<ZwpInputPanelV1, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputPanelV1,
        _event: <ZwpInputPanelV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 zwp_input_panel_surface_v1 实现 Dispatch
impl Dispatch<ZwpInputPanelSurfaceV1, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputPanelSurfaceV1,
        _event: <ZwpInputPanelSurfaceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 zwlr_layer_shell_v1 实现 Dispatch
impl Dispatch<ZwlrLayerShellV1, ()> for WaylanV1Session {
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

// 为 zwlr_layer_surface_v1 实现 Dispatch
impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            LayerSurfaceEvent::Configure { serial, .. } => {
                proxy.ack_configure(serial);
            }
            LayerSurfaceEvent::Closed => {
                log::info!("[V1会话] 图层表面 (LayerSurface) 已被合成器关闭，重置表面句柄。");
                state.layer_surface = None;
            }
            _ => {}
        }
    }
}
