//! Wayland 输入法协议与合成器全局接口绑定
//!
//! # 设计原理
//! - **实现初衷**：将 Wayland V1 与 V2 会话初始化时对各种全局协议对象（Compositor、Seat、
//!   InputMethod、LayerShell、VirtualKeyboard 等）的绑定与能力探测逻辑与主事件循环解耦。
//! - **核心优势**：关注点分离，将复杂的协议协商与错误转换内聚于独立模块，降低单文件复杂度。
//! - **代价与局限**：涉及与会话内部上下文的借用交互，需通过 `session` 对应字段赋值。

use log::{info, warn};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_seat::WlSeat, wl_shm::WlShm};
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};

use crate::error::WaylandRunnerError;
use crate::protocol_v1::input_method_v1::zwp_input_method_v1::ZwpInputMethodV1;
use crate::protocol_v1::input_method_v1::zwp_input_panel_v1::ZwpInputPanelV1;
use crate::session_common::WaylandSessionLifecycle;
use crate::session_v1::WaylanV1Session;
use crate::session_v2::WaylanV2Session;

/// 初始化绑定 V1 输入法协议与相关全局接口（Compositor、Panel/LayerShell、Shm）
pub(super) fn bind_session_v1_globals(
    globals: &GlobalList,
    qh: &QueueHandle<WaylanV1Session>,
    session: &mut WaylanV1Session,
) -> Result<(), WaylandRunnerError> {
    let mut im_global = None;
    for g in globals.contents().clone_list() {
        if g.interface == "zwp_input_method_v1" {
            im_global = Some(g);
            break;
        }
    }

    if im_global.is_none() {
        return Err(WaylandRunnerError::UnsupportedProtocol {
            interface: "zwp_input_method_v1",
            reason: "合成器全局注册表中未声明该接口".to_string(),
        });
    }

    let _input_method: ZwpInputMethodV1 = globals.bind(qh, 1..=1, ())?;

    if let Ok(compositor) = globals.bind::<WlCompositor, _, _>(qh, 1..=6, ()) {
        let wl_surface = compositor.create_surface(qh, ());

        if let Ok(input_panel) = globals.bind::<ZwpInputPanelV1, _, _>(qh, 1..=1, ()) {
            let panel_surface = input_panel.get_input_panel_surface(&wl_surface, qh, ());
            panel_surface.set_overlay_panel();
            session.protocol.panel_surface = Some(panel_surface);
            info!("[微澜 Wayland] 成功绑定 zwp_input_panel_v1 并将候选框配置为光标悬浮面板(overlay_panel)。");
        } else if let Ok(layer_shell) = globals.bind::<ZwlrLayerShellV1, _, _>(qh, 1..=4, ()) {
            let layer_surface = layer_shell.get_layer_surface(
                &wl_surface,
                None,
                Layer::Overlay,
                "waylan-candidate-box".to_string(),
                qh,
                (),
            );
            layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer_surface.set_anchor(Anchor::Top | Anchor::Left);
            layer_surface.set_size(1, 1);
            wl_surface.commit();
            session.protocol.layer_surface = Some(layer_surface);
            info!(
                "[微澜 Wayland] 成功绑定 zwlr_layer_shell_v1 并将候选框配置为 Overlay 图层表面。"
            );
        } else {
            warn!("[微澜 Wayland] 合成器未提供 zwp_input_panel_v1 或 zwlr_layer_shell_v1，裸 wl_surface 可能无法正常显示。");
        }

        session.surface.popup_wl_surface = Some(wl_surface);
    }
    if let Ok(shm) = globals.bind::<WlShm, _, _>(qh, 1..=1, ()) {
        session.surface.shm = Some(shm);
    }
    session.qhandle = Some(qh.clone());
    session.init_inotify_watcher();
    info!("[微澜 Wayland] 成功绑定 zwp_input_method_v1 接口，进入输入法事件分发循环。");
    Ok(())
}

/// 初始化绑定 V2 现代输入法协议与虚拟键盘回退接口
pub(super) fn bind_session_v2_globals(
    globals: &GlobalList,
    qh: &QueueHandle<WaylanV2Session>,
    session: &mut WaylanV2Session,
) -> Result<(), WaylandRunnerError> {
    let seat: WlSeat =
        globals
            .bind(qh, 1..=7, ())
            .map_err(|e| WaylandRunnerError::MissingGlobal {
                interface: "wl_seat",
                reason: e.to_string(),
            })?;

    let im_manager: ZwpInputMethodManagerV2 =
        globals
            .bind(qh, 1..=1, ())
            .map_err(|e| WaylandRunnerError::UnsupportedProtocol {
                interface: "zwp_input_method_manager_v2",
                reason: e.to_string(),
            })?;
    let input_method = im_manager.get_input_method(&seat, qh, ());
    let pointer = seat.get_pointer(qh, ());
    session.surface.pointer = Some(pointer);
    info!("[微澜 Wayland] 成功向 wl_seat 注册鼠标指针(wl_pointer)，支持候选框点击与网格展开交互。");

    let compositor: WlCompositor =
        globals
            .bind(qh, 1..=6, ())
            .map_err(|e| WaylandRunnerError::MissingGlobal {
                interface: "wl_compositor",
                reason: e.to_string(),
            })?;
    let wl_surface = compositor.create_surface(qh, ());
    let popup_surface = input_method.get_input_popup_surface(&wl_surface, qh, ());
    session.surface.popup_wl_surface = Some(wl_surface);
    session.protocol.popup_surface = Some(popup_surface);

    let shm: WlShm =
        globals
            .bind(qh, 1..=1, ())
            .map_err(|e| WaylandRunnerError::MissingGlobal {
                interface: "wl_shm",
                reason: e.to_string(),
            })?;
    session.protocol.shm = Some(shm);
    session.qhandle = Some(qh.clone());
    session.set_input_method(input_method);

    match globals.bind::<ZwpVirtualKeyboardManagerV1, _, _>(qh, 1..=1, ()) {
        Ok(vk_mgr) => {
            let vk = vk_mgr.create_virtual_keyboard(&seat, qh, ());
            session.protocol.virtual_keyboard = Some(vk);
            info!("[微澜 Wayland] 成功绑定 zwp_virtual_keyboard_v1，支持虚拟键盘事件回退。");
        }
        Err(e) => {
            return Err(WaylandRunnerError::UnsupportedProtocol {
                interface: "zwp_virtual_keyboard_v1",
                reason: format!("合成器缺少扩展协议 ({})，为避免按键黑洞主动放弃 V2 会话", e),
            });
        }
    }

    session.init_inotify_watcher();
    info!("[微澜 Wayland] 成功绑定 zwp_input_method_v2 接口，进入输入法事件分发循环。");
    Ok(())
}
