use crate::surface::interaction::CursorRectUpdateParams;
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

use crate::session_common::WaylandSessionLifecycle;
use crate::session_v2::WaylanV2Session;

/// 处理输入法激活状态确认（Done 事务）
fn handle_v2_done(
    state: &mut WaylanV2Session,
    proxy: &ZwpInputMethodV2,
    qhandle: &QueueHandle<WaylanV2Session>,
) {
    let now_active = state.apply_done_state();
    if now_active {
        if state.protocol.keyboard_grab.is_none() {
            let grab = proxy.grab_keyboard(qhandle, ());
            state.protocol.keyboard_grab = Some(grab);
        }
    } else if let Some(grab) = state.protocol.keyboard_grab.take() {
        grab.release();
    }
}

/// 处理输入法 v2 协议不可用时的安全回退与资源释放
fn handle_v2_unavailable(state: &mut WaylanV2Session, proxy: &ZwpInputMethodV2) {
    info!("[Wayland v2] 输入法 v2 协议不可用，正在自动降级兼容模式。");
    state.focus.pending_active = false;
    state.focus.is_active = false;
    state.protocol.is_unavailable = true;
    if let Some(grab) = state.protocol.keyboard_grab.take() {
        grab.release();
    }
    proxy.destroy();
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
                state.focus.pending_is_password = matches!(
                    purpose,
                    WEnum::Value(ContentPurpose::Password | ContentPurpose::Pin)
                );
            }
            V2Event::Done => handle_v2_done(state, proxy, qhandle),
            V2Event::Unavailable => handle_v2_unavailable(state, proxy),
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
            state.surface.current_cursor = CursorRect::new(x, y, width, height);
            state
                .surface
                .surface_manager
                .update_cursor_rect(&CursorRectUpdateParams {
                    x,
                    y,
                    width,
                    height,
                });
        }
    }
}

// 为 zwp_input_method_keyboard_grab_v2 实现事件分发
/// 处理键盘抓取 keymap 映射配置同步
fn handle_grab_keymap(
    state: &mut WaylanV2Session,
    format: WEnum<wayland_client::protocol::wl_keyboard::KeymapFormat>,
    fd: &std::os::fd::OwnedFd,
    size: u32,
) {
    debug!(
        "[Wayland v2 键盘抓取] 收到键盘映射配置: format={:?}, 大小={} 字节",
        format, size
    );
    if let Ok(mut file) = fd.try_clone().map(File::from) {
        let read_len = (size as usize).min(1024 * 1024);
        let mut buf = vec![0u8; read_len];
        let read_ok = file.read_exact(&mut buf).is_ok();
        let _ = file.seek(SeekFrom::Start(0));
        if read_ok {
            let text = String::from_utf8_lossy(&buf);
            state
                .runtime
                .core
                .keymap_layout_mut()
                .update_from_xkb_text(text.trim_end_matches('\0'));
            debug!("[Wayland v2 键盘抓取] 成功从 XKB Keymap 动态同步键盘字符布局映射。");
        }
    }
    if let Some(ref vk) = state.protocol.virtual_keyboard {
        vk.keymap(u32::from(format), fd.as_fd(), size);
        state.protocol.has_virtual_keymap = true;
    }
}

/// 键盘抓取修饰键状态同步参数
///
/// # 设计原理
/// - **实现初衷**：XKB 修饰键事件携带"按下、锁存、锁定、布局组"四组位掩码，
///   平铺传参易与相邻的虚拟键盘同步调用发生顺序错位。
/// - **核心优势**：以参数对象聚合同源位掩码，调用点按名绑定语义。
/// - **代价与局限**：多引入一个结构体类型。
struct GrabModifiersParams {
    /// 当前按下的修饰键位掩码
    mods_depressed: u32,
    /// 已锁存待生效的修饰键位掩码
    mods_latched: u32,
    /// 已锁定的修饰键位掩码
    mods_locked: u32,
    /// 当前键盘布局组索引
    group: u32,
}

/// 处理键盘抓取修饰键状态同步与 CapsLock 通知
fn handle_grab_modifiers(state: &mut WaylanV2Session, params: GrabModifiersParams) {
    let GrabModifiersParams {
        mods_depressed,
        mods_latched,
        mods_locked,
        group,
    } = params;
    let prev_caps = state.runtime.core.is_caps_locked();
    state
        .runtime
        .core
        .set_modifiers(mods_depressed, mods_locked);
    state.keyboard.mods_latched = mods_latched;
    state.keyboard.mods_group = group;
    if state.protocol.has_virtual_keymap {
        if let Some(ref vk) = state.protocol.virtual_keyboard {
            vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
        }
    }
    let curr_caps = state.runtime.core.is_caps_locked();
    if prev_caps != curr_caps {
        state.runtime.update_caps_lock_state(curr_caps);
        debug!(
            "[Wayland v2] CapsLock 大写锁定状态变更: {}",
            if curr_caps {
                "开启大写"
            } else {
                "解除大写"
            }
        );
    }
    debug!(
        "[Wayland v2 键盘抓取] 收到修饰键变更: depressed={}, latched={}, locked={}, group={}",
        mods_depressed, mods_latched, mods_locked, group
    );
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
                handle_grab_keymap(state, format, &fd, size);
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
                handle_grab_modifiers(
                    state,
                    GrabModifiersParams {
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    },
                );
            }
            GrabEvent::RepeatInfo { rate, delay } => {
                state.keyboard.repeat_info = (rate, delay);
                debug!(
                    "[Wayland v2 键盘抓取] 收到按键连击设置: rate={}, delay={}",
                    rate, delay
                );
            }
            _ => {}
        }
    }
}

/// 处理鼠标点击选中特定索引的候选词
fn handle_pointer_select_candidate(state: &mut WaylanV2Session, idx: usize) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let select_res = if state.runtime.core.is_grid_expanded() {
        state
            .runtime
            .core
            .engine
            .select_candidate_from_all(idx, now)
    } else {
        state.runtime.core.engine.select_candidate(idx, now)
    };
    if let Some((commit_text, remaining_len)) = select_res {
        state.runtime.core.collapse_grid();
        state
            .runtime
            .core
            .punctuation_processor_mut()
            .record_commit(&commit_text);
        if remaining_len == 0 {
            state.commit_text(&commit_text);
        } else {
            let formatted = state.runtime.core.engine.formatted_preedit();
            state.commit_partial(&commit_text, &formatted);
        }
    }
}

/// 处理鼠标点击候选框事件（包含展开/折叠、选词上屏与翻页）
fn handle_pointer_click(state: &mut WaylanV2Session) {
    let (px, py) = state.surface.pointer_pos;
    let hit = state.surface.surface_manager.hit_test(px as f32, py as f32);
    let Some(target) = hit else {
        return;
    };

    match target {
        CandidateHitTarget::ToggleExpand => {
            state.runtime.core.toggle_grid_expansion();
            state.refresh_candidate_surface();
        }
        CandidateHitTarget::Select(idx) => handle_pointer_select_candidate(state, idx),
        CandidateHitTarget::NextPage => {
            state.runtime.core.next_page();
            state.refresh_candidate_surface();
        }
        CandidateHitTarget::PrevPage => {
            state.runtime.core.prev_page();
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
                if let Some(ref popup_surface) = state.surface.popup_wl_surface {
                    if &surface == popup_surface {
                        state.surface.pointer_in_surface = true;
                        state.surface.pointer_pos = (surface_x, surface_y);
                    }
                }
            }
            PointerEvent::Leave { surface, .. } => {
                if let Some(ref popup_surface) = state.surface.popup_wl_surface {
                    if &surface == popup_surface {
                        state.surface.pointer_in_surface = false;
                    }
                }
            }
            PointerEvent::Motion {
                surface_x,
                surface_y,
                ..
            } if state.surface.pointer_in_surface => {
                state.surface.pointer_pos = (surface_x, surface_y);
            }
            PointerEvent::Button {
                button: 0x110,
                state: WEnum::Value(WlButtonState::Pressed),
                ..
            } if state.surface.pointer_in_surface => {
                handle_pointer_click(state);
            }
            _ => {}
        }
    }
}
