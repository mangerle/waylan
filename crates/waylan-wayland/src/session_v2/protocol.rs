use crate::surface::CursorRect;
use log::info;
use std::os::fd::AsFd;
use wayland_client::protocol::wl_keyboard::KeyState;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_keyboard_grab_v2::{
    Event as GrabEvent, ZwpInputMethodKeyboardGrabV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    Event as V2Event, ZwpInputMethodV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::{
    Event as PopupEvent, ZwpInputPopupSurfaceV2,
};

use crate::session_v2::WaylanV2Session;

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
                info!("[Wayland v2] 收到会话激活挂起事件（等待 Done 原子生效）。");
                state.stage_activate();
            }
            V2Event::Deactivate => {
                info!("[Wayland v2] 收到会话失焦挂起事件（等待 Done 原子生效）。");
                state.stage_deactivate();
            }
            V2Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                info!(
                    "[Wayland v2] 收到挂起上下文环境文本: 长度={}, 光标={}, 锚点={}",
                    text.len(),
                    cursor,
                    anchor
                );
                state.stage_surrounding_text(text, cursor, anchor);
            }
            V2Event::TextChangeCause { cause: _ } => {}
            V2Event::ContentType {
                hint: _,
                purpose: _,
            } => {}
            V2Event::Done => {
                let now_active = state.apply_done_state();
                if now_active {
                    if state.keyboard_grab.is_none() {
                        let grab = proxy.grab_keyboard(qhandle, ());
                        state.keyboard_grab = Some(grab);
                    }
                } else if let Some(grab) = state.keyboard_grab.take() {
                    grab.release();
                }
            }
            V2Event::Unavailable => {
                info!("[Wayland v2] 输入法 v2 协议不可用，正在自动降级兼容模式。");
                state.pending_active = false;
                state.is_active = false;
                state.is_unavailable = true;
                if let Some(grab) = state.keyboard_grab.take() {
                    grab.release();
                }
                proxy.destroy();
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
        if let PopupEvent::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            info!(
                "[Wayland v2 光标矩形] 捕获焦点光标变化: x={}, y={}, w={}, h={}",
                x, y, width, height
            );
            state.current_cursor = CursorRect::new(x, y, width, height);
            state
                .surface_manager
                .update_cursor_rect(x, y, width, height);
        }
    }
}

// 为 zwp_input_method_keyboard_grab_v2 实现事件分发
impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodKeyboardGrabV2,
        event: GrabEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        state.sync_external_state();
        match event {
            GrabEvent::Keymap { format, fd, size } => {
                info!(
                    "[Wayland v2 键盘抓取] 收到键盘映射配置: format={:?}, 大小={} 字节",
                    format, size
                );
                if let Some(ref vk) = state.virtual_keyboard {
                    vk.keymap(u32::from(format), fd.as_fd(), size);
                    state.has_virtual_keymap = true;
                }
            }
            GrabEvent::Key {
                serial: _,
                time,
                key,
                state: key_state,
            } => {
                let is_press = matches!(key_state, WEnum::Value(KeyState::Pressed));
                let (consumed, _) = state.process_key_event(key, is_press);
                if !consumed {
                    state.forward_key_to_seat(time, key, is_press);
                }
            }
            GrabEvent::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                state.mods_depressed = mods_depressed;
                state.mods_latched = mods_latched;
                state.mods_locked = mods_locked;
                state.mods_group = group;
                if state.has_virtual_keymap {
                    if let Some(ref vk) = state.virtual_keyboard {
                        vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                    }
                }
                info!(
                    "[Wayland v2 键盘抓取] 收到修饰键变更: depressed={}, latched={}, locked={}, group={}",
                    mods_depressed, mods_latched, mods_locked, group
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
