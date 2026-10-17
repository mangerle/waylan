//! Wayland 协议事件通用派发宏
//!
//! # 设计原理
//! - **实现初衷**：为输入法会话（V1 与 V2）自动实现基础通用协议（wl_seat, wl_compositor,
//!   wl_shm, wl_surface, wl_shm_pool, wl_buffer, wl_registry）的事件 Dispatch 样板代码。
//! - **核心优势**：消除上百行机械重复的 trait 实现，收敛缓冲区缩放响应与内存池释放回调。

/// 为会话实现 7 组通用的基础 Wayland Dispatch trait（消除 V1/V2 重复样板代码）
#[macro_export]
macro_rules! impl_common_wayland_dispatch {
    ($session_type:ty) => {
        impl wayland_client::Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()>
            for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_seat::WlSeat,
                _event: <wayland_client::protocol::wl_seat::WlSeat as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_compositor::WlCompositor,
                (),
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_compositor::WlCompositor,
                _event: <wayland_client::protocol::wl_compositor::WlCompositor as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl wayland_client::Dispatch<wayland_client::protocol::wl_shm::WlShm, ()>
            for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_shm::WlShm,
                _event: <wayland_client::protocol::wl_shm::WlShm as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_surface::WlSurface,
                (),
            > for $session_type
        {
            fn event(
                state: &mut Self,
                _proxy: &wayland_client::protocol::wl_surface::WlSurface,
                event: <wayland_client::protocol::wl_surface::WlSurface as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
                if let wayland_client::protocol::wl_surface::Event::PreferredBufferScale { factor } =
                    event
                {
                    state.surface_manager.set_scale(factor);
                }
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_shm_pool::WlShmPool,
                (),
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_shm_pool::WlShmPool,
                _event: <wayland_client::protocol::wl_shm_pool::WlShmPool as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_buffer::WlBuffer,
                (),
            > for $session_type
        {
            fn event(
                state: &mut Self,
                proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
                event: <wayland_client::protocol::wl_buffer::WlBuffer as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
                if matches!(
                    event,
                    wayland_client::protocol::wl_buffer::Event::Release
                ) {
                    state.surface_manager.on_buffer_released(proxy);
                }
            }
        }

        impl
            wayland_client::Dispatch<
                wayland_client::protocol::wl_registry::WlRegistry,
                wayland_client::globals::GlobalListContents,
            > for $session_type
        {
            fn event(
                _state: &mut Self,
                _proxy: &wayland_client::protocol::wl_registry::WlRegistry,
                _event: <wayland_client::protocol::wl_registry::WlRegistry as wayland_client::Proxy>::Event,
                _data: &wayland_client::globals::GlobalListContents,
                _conn: &wayland_client::Connection,
                _qhandle: &wayland_client::QueueHandle<Self>,
            ) {
            }
        }
    };
}
