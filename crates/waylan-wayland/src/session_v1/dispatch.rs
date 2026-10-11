use crate::protocol_v1::input_method_v1::zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1;
use crate::protocol_v1::input_method_v1::zwp_input_panel_v1::ZwpInputPanelV1;
use crate::session_v1::WaylanV1Session;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Event as LayerSurfaceEvent, ZwlrLayerSurfaceV1,
};

// 为 wl_seat 实现 Dispatch
impl Dispatch<WlSeat, ()> for WaylanV1Session {
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

// 为 wl_compositor 实现 Dispatch
impl Dispatch<WlCompositor, ()> for WaylanV1Session {
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

// 为 wl_shm 实现 Dispatch
impl Dispatch<WlShm, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        _event: <WlShm as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_surface 实现 Dispatch
impl Dispatch<WlSurface, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        _proxy: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_surface::Event::PreferredBufferScale { factor } = event
        {
            state.surface_manager.set_scale(factor);
        }
    }
}

// 为 wl_shm_pool 实现 Dispatch
impl Dispatch<WlShmPool, ()> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

// 为 wl_buffer 实现 Dispatch
impl Dispatch<WlBuffer, ()> for WaylanV1Session {
    fn event(
        state: &mut Self,
        proxy: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            state.surface_manager.on_buffer_released(proxy);
        }
    }
}

// 为 wl_registry 实现 Dispatch
impl Dispatch<WlRegistry, wayland_client::globals::GlobalListContents> for WaylanV1Session {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &wayland_client::globals::GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

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
        _state: &mut Self,
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
            LayerSurfaceEvent::Closed => {}
            _ => {}
        }
    }
}
