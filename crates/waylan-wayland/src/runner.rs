use std::io::ErrorKind;
use std::sync::atomic::Ordering;

use log::{info, warn};
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::BorrowedFd;
use rustix::io::Errno;
use wayland_client::backend::{ReadEventsGuard, WaylandError};
use wayland_client::globals::{registry_queue_init, GlobalList};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_seat::WlSeat, wl_shm::WlShm};
use wayland_client::{Connection, QueueHandle};
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
use crate::session_v1::WaylanV1Session;
use crate::session_v2::WaylanV2Session;

/// 初始化绑定 V1 输入法协议与相关全局接口（Compositor、Panel/LayerShell、Shm）
fn bind_session_v1_globals(
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
            session.panel_surface = Some(panel_surface);
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
            session.layer_surface = Some(layer_surface);
            info!(
                "[微澜 Wayland] 成功绑定 zwlr_layer_shell_v1 并将候选框配置为 Overlay 图层表面。"
            );
        } else {
            warn!("[微澜 Wayland] 合成器未提供 zwp_input_panel_v1 或 zwlr_layer_shell_v1，裸 wl_surface 可能无法正常显示。");
        }

        session.popup_wl_surface = Some(wl_surface);
    }
    if let Ok(shm) = globals.bind::<WlShm, _, _>(qh, 1..=1, ()) {
        session.shm = Some(shm);
    }
    session.qhandle = Some(qh.clone());
    session.init_inotify_watcher();
    info!("[微澜 Wayland] 成功绑定 zwp_input_method_v1 接口，进入输入法事件分发循环。");
    Ok(())
}

/// 事件循环 poll 多路复用分发结果
enum PollEventOutcome {
    /// 超时到达（poll 返回 0）
    TimedOut,
    /// 监测到就绪事件（唤醒或 inotify 描述符变化）
    WokenUp {
        wakeup_triggered: bool,
        inotify_triggered: bool,
    },
    /// 被系统中断信号唤醒
    Interrupted,
}

/// 执行事件循环多路复用 poll 与 Wayland 消息读取
fn poll_and_read_events(
    guard: ReadEventsGuard,
    timeout_ms: i32,
    inotify_fd: Option<BorrowedFd<'_>>,
    wakeup_fd: Option<BorrowedFd<'_>>,
) -> Result<PollEventOutcome, WaylandRunnerError> {
    let wayland_fd = guard.connection_fd();
    let mut fds = [
        PollFd::new(&wayland_fd, PollFlags::IN | PollFlags::ERR),
        PollFd::new(&wayland_fd, PollFlags::empty()),
        PollFd::new(&wayland_fd, PollFlags::empty()),
    ];
    let mut fd_count = 1;

    let inotify_idx = inotify_fd.as_ref().map(|fd| {
        let idx = fd_count;
        fds[idx] = PollFd::new(fd, PollFlags::IN | PollFlags::ERR);
        fd_count += 1;
        idx
    });

    let wakeup_idx = wakeup_fd.as_ref().map(|fd| {
        let idx = fd_count;
        fds[idx] = PollFd::new(fd, PollFlags::IN | PollFlags::ERR);
        fd_count += 1;
        idx
    });

    match poll(&mut fds[..fd_count], timeout_ms) {
        Ok(0) => {
            drop(guard);
            Ok(PollEventOutcome::TimedOut)
        }
        Ok(_) => {
            let wakeup_triggered = wakeup_idx
                .map(|idx| {
                    fds[idx]
                        .revents()
                        .intersects(PollFlags::IN | PollFlags::ERR)
                })
                .unwrap_or(false);
            let inotify_triggered = inotify_idx
                .map(|idx| {
                    fds[idx]
                        .revents()
                        .intersects(PollFlags::IN | PollFlags::ERR)
                })
                .unwrap_or(false);

            if fds[0].revents().intersects(PollFlags::IN | PollFlags::ERR) {
                if let Err(WaylandError::Io(io_err)) = guard.read() {
                    if io_err.kind() != ErrorKind::WouldBlock {
                        return Err(io_err.into());
                    }
                }
            } else {
                drop(guard);
            }

            Ok(PollEventOutcome::WokenUp {
                wakeup_triggered,
                inotify_triggered,
            })
        }
        Err(Errno::INTR) => Ok(PollEventOutcome::Interrupted),
        Err(e) => Err(e.into()),
    }
}

/// 启动并运行 Wayland V1 输入法事件循环（使用已初始化的会话实例）
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必要全局协议或事件队列派发异常时返回对应的细粒度错误。
pub fn run_session_v1_loop(mut session: WaylanV1Session) -> Result<(), WaylandRunnerError> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    bind_session_v1_globals(&globals, &qh, &mut session)?;

    loop {
        if session
            .exit_requested
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(false)
        {
            info!("[微澜 Wayland] 收到退出请求信号，正在安全退出 V1 事件循环。");
            return Ok(());
        }

        event_queue.dispatch_pending(&mut session)?;
        event_queue.flush()?;

        let default_poll_timeout = if session.inotify_watcher.is_some() {
            -1
        } else {
            2000
        };
        let timeout_ms = session.next_poll_timeout_ms(default_poll_timeout);
        let inotify_fd = session.inotify_watcher.as_ref().map(|w| w.as_fd());
        let wakeup_fd = session.loop_wakeup.as_ref().map(|w| w.as_fd());

        if let Some(guard) = event_queue.prepare_read() {
            match poll_and_read_events(guard, timeout_ms, inotify_fd, wakeup_fd)? {
                PollEventOutcome::TimedOut => {
                    if session.inotify_watcher.is_none() {
                        session.sync_external_state();
                    }
                }
                PollEventOutcome::WokenUp {
                    wakeup_triggered,
                    inotify_triggered,
                } => {
                    if wakeup_triggered {
                        if let Some(ref wakeup) = session.loop_wakeup {
                            wakeup.drain();
                        }
                        session.sync_external_state();
                    } else if inotify_triggered {
                        session.sync_external_state();
                    }
                }
                PollEventOutcome::Interrupted => {}
            }
        }
    }
}

/// 初始化绑定 V2 现代输入法协议与虚拟键盘回退接口
fn bind_session_v2_globals(
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
    session.pointer = Some(pointer);
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
    session.popup_wl_surface = Some(wl_surface);
    session.popup_surface = Some(popup_surface);

    let shm: WlShm =
        globals
            .bind(qh, 1..=1, ())
            .map_err(|e| WaylandRunnerError::MissingGlobal {
                interface: "wl_shm",
                reason: e.to_string(),
            })?;
    session.shm = Some(shm);
    session.qhandle = Some(qh.clone());
    session.set_input_method(input_method);

    match globals.bind::<ZwpVirtualKeyboardManagerV1, _, _>(qh, 1..=1, ()) {
        Ok(vk_mgr) => {
            let vk = vk_mgr.create_virtual_keyboard(&seat, qh, ());
            session.virtual_keyboard = Some(vk);
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

/// 启动并运行现代 Wayland V2 输入法事件循环（若协议不可用或发生错误，返回错误同时移交 session 实例以便降级）
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必需全局协议或事件队列异常时返回错误及当前会话实例。
pub fn run_session_v2_loop_fallible(
    mut session: WaylanV2Session,
) -> Result<(), (WaylandRunnerError, Box<WaylanV2Session>)> {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let (globals, mut event_queue) = match registry_queue_init(&conn) {
        Ok(res) => res,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let qh = event_queue.handle();

    if let Err(e) = bind_session_v2_globals(&globals, &qh, &mut session) {
        return Err((e, Box::new(session)));
    }

    loop {
        if session
            .exit_requested
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(false)
        {
            info!("[微澜 Wayland] 收到退出请求信号，正在安全退出 V2 事件循环。");
            return Ok(());
        }

        if let Err(e) = event_queue.dispatch_pending(&mut session) {
            return Err((e.into(), Box::new(session)));
        }

        if let Err(e) = event_queue.flush() {
            return Err((e.into(), Box::new(session)));
        }

        let default_poll_timeout = if session.inotify_watcher.is_some() {
            -1
        } else {
            2000
        };
        let timeout_ms = session.next_poll_timeout_ms(default_poll_timeout);
        let inotify_fd = session.inotify_watcher.as_ref().map(|w| w.as_fd());
        let wakeup_fd = session.loop_wakeup.as_ref().map(|w| w.as_fd());

        if let Some(guard) = event_queue.prepare_read() {
            let outcome = match poll_and_read_events(guard, timeout_ms, inotify_fd, wakeup_fd) {
                Ok(res) => res,
                Err(e) => return Err((e, Box::new(session))),
            };
            match outcome {
                PollEventOutcome::TimedOut => {
                    session.drive_key_repeat();
                    if session.inotify_watcher.is_none() {
                        session.sync_external_state();
                    }
                }
                PollEventOutcome::WokenUp {
                    wakeup_triggered,
                    inotify_triggered,
                } => {
                    if wakeup_triggered {
                        if let Some(ref wakeup) = session.loop_wakeup {
                            wakeup.drain();
                        }
                        session.sync_external_state();
                    } else if inotify_triggered {
                        session.sync_external_state();
                    }
                }
                PollEventOutcome::Interrupted => {}
            }
        }
    }
}

/// 启动并运行现代 Wayland V2 输入法事件循环（适用于 KDE Plasma 6 / GNOME / Hyprland / Sway）
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必需全局协议或事件队列异常时返回对应的细粒度错误。
pub fn run_session_v2_loop(session: WaylanV2Session) -> Result<(), WaylandRunnerError> {
    run_session_v2_loop_fallible(session).map_err(|(e, _)| e)
}
