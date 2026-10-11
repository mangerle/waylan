use crate::surface::CursorRect;
use log::{debug, info, trace};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::time::{SystemTime, UNIX_EPOCH};
use waylan_ui::CandidateHitTarget;
use wayland_client::protocol::wl_keyboard::KeyState;
use wayland_client::protocol::wl_pointer::{
    ButtonState as WlButtonState, Event as PointerEvent, WlPointer,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ContentPurpose;
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
                debug!("[Wayland v2] 收到会话激活挂起事件（等待 Done 原子生效）。");
                state.stage_activate();
            }
            V2Event::Deactivate => {
                debug!("[Wayland v2] 收到会话失焦挂起事件（等待 Done 原子生效）。");
                state.stage_deactivate();
            }
            V2Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                trace!(
                    "[Wayland v2] 收到挂起上下文环境文本: 长度={}, 光标={}, 锚点={}",
                    text.len(),
                    cursor,
                    anchor
                );
                state.stage_surrounding_text(text, cursor, anchor);
            }
            V2Event::TextChangeCause { cause: _ } => {}
            V2Event::ContentType { hint: _, purpose } => {
                state.pending_is_password = matches!(
                    purpose,
                    WEnum::Value(ContentPurpose::Password | ContentPurpose::Pin)
                );
            }
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
            trace!(
                "[Wayland v2 光标矩形] 捕获焦点光标变化: x={}, y={}, w={}, h={}",
                x,
                y,
                width,
                height
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
        state.sync_atomic_state();
        match event {
            GrabEvent::Keymap { format, fd, size } => {
                debug!(
                    "[Wayland v2 键盘抓取] 收到键盘映射配置: format={:?}, 大小={} 字节",
                    format, size
                );
                // 尝试从 fd 读取 XKB keymap 文本并动态更新键盘字符映射以适配非 QWERTY 布局
                if let Ok(mut file) = fd.try_clone().map(File::from) {
                    let read_len = (size as usize).min(1024 * 1024);
                    let mut buf = vec![0u8; read_len];
                    let read_ok = file.read_exact(&mut buf).is_ok();
                    let _ = file.seek(SeekFrom::Start(0));
                    if read_ok {
                        let text = String::from_utf8_lossy(&buf);
                        state
                            .keymap_layout
                            .update_from_xkb_text(text.trim_end_matches('\0'));
                        debug!(
                            "[Wayland v2 键盘抓取] 成功从 XKB Keymap 动态同步键盘字符布局映射。"
                        );
                    }
                }
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
                debug!(
                    "[Wayland v2 键盘抓取] 收到修饰键变更: depressed={}, latched={}, locked={}, group={}",
                    mods_depressed, mods_latched, mods_locked, group
                );
            }
            GrabEvent::RepeatInfo { rate, delay } => {
                state.repeat_info = (rate, delay);
                debug!(
                    "[Wayland v2 键盘抓取] 收到按键连击设置: rate={}, delay={}",
                    rate, delay
                );
            }
            _ => {}
        }
    }
}

/// 处理鼠标点击候选框事件（包含展开/折叠、选词上屏与翻页）
fn handle_pointer_click(state: &mut WaylanV2Session) {
    let (px, py) = state.pointer_pos;
    let hit = state
        .surface_manager
        .renderer
        .hit_test(px as f32, py as f32);
    let Some(target) = hit else {
        return;
    };

    match target {
        CandidateHitTarget::ToggleExpand => {
            state.is_grid_expanded = !state.is_grid_expanded;
            if state.is_grid_expanded {
                state.grid_selected_index = state.engine.selected_index;
            } else {
                state.grid_selected_index = 0;
            }
            state.refresh_candidate_surface();
        }
        CandidateHitTarget::Select(idx) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let select_res = if state.is_grid_expanded {
                state.engine.select_candidate_from_all(idx, now)
            } else {
                state.engine.select_candidate(idx, now)
            };
            if let Some((commit_text, remaining_len)) = select_res {
                state.is_grid_expanded = false;
                state.grid_selected_index = 0;
                state.punctuation_processor.record_commit(&commit_text);
                if remaining_len == 0 {
                    state.commit_text(&commit_text);
                } else {
                    let formatted = state.engine.formatted_preedit();
                    state.commit_partial(&commit_text, &formatted);
                }
            }
        }
        CandidateHitTarget::NextPage => {
            state.engine.next_page();
            state.refresh_candidate_surface();
        }
        CandidateHitTarget::PrevPage => {
            state.engine.prev_page();
            state.refresh_candidate_surface();
        }
    }
}

// 为 wl_pointer 实现事件分发
impl Dispatch<WlPointer, ()> for WaylanV2Session {
    fn event(
        state: &mut Self,
        _proxy: &WlPointer,
        event: PointerEvent,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            PointerEvent::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(ref popup_surface) = state.popup_wl_surface {
                    if &surface == popup_surface {
                        state.pointer_in_surface = true;
                        state.pointer_pos = (surface_x, surface_y);
                    }
                }
            }
            PointerEvent::Leave { surface, .. } => {
                if let Some(ref popup_surface) = state.popup_wl_surface {
                    if &surface == popup_surface {
                        state.pointer_in_surface = false;
                    }
                }
            }
            PointerEvent::Motion {
                surface_x,
                surface_y,
                ..
            } if state.pointer_in_surface => {
                state.pointer_pos = (surface_x, surface_y);
            }
            PointerEvent::Button {
                button: 0x110,
                state: WEnum::Value(WlButtonState::Pressed),
                ..
            } if state.pointer_in_surface => {
                handle_pointer_click(state);
            }
            _ => {}
        }
    }
}
