pub(crate) mod item;
pub(crate) mod menu;

use crate::config::AppConfig;
use crate::system::{ShutdownCoordinator, SubprocessManager};
use item::{ResolvedTrayIcons, resolve_tray_icons};
use log::{debug, info, warn};
use menu::{DBusMenu, DBusMenuOptions};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;
#[cfg(test)]
use waylan_core::InputMode;
use waylan_core::state::SharedInputFlags;
use zbus::Variant;
use zbus::connection::{Builder, Connection};

pub use item::StatusNotifierItem;

/// 可跨线程克隆的托盘图标热重载同步句柄
#[derive(Clone)]
pub struct TraySyncHandle {
    icons: Arc<RwLock<ResolvedTrayIcons>>,
    icon_update_tx: SyncSender<()>,
}

impl TraySyncHandle {
    /// 依据最新配置更新托盘图标名称/主题路径，并触发一次 D-Bus `NewIcon` 信号刷新
    pub fn sync_config(&self, config: &AppConfig) {
        *self.icons.write() = resolve_tray_icons(config);
        let _ = self.icon_update_tx.try_send(());
    }
}

/// 托盘后台事件循环参数包
struct TrayLoopParams {
    icons: Arc<RwLock<ResolvedTrayIcons>>,
    shared_flags: SharedInputFlags,
    reload_requested: Arc<AtomicBool>,
    coordinator: ShutdownCoordinator,
    subprocess_manager: Arc<SubprocessManager>,
    settings_running: Arc<AtomicBool>,
    icon_update_tx: SyncSender<()>,
    icon_update_rx: Receiver<()>,
    dbus_ready: Arc<AtomicBool>,
    ready_tx: SyncSender<bool>,
}

/// 系统托盘服务控制器
///
/// # 设计原理
/// - **实现初衷**：在现代 Linux Wayland 任务栏（Waybar/KDE/GNOME）呈现常驻输入法托盘，提供状态展示与快捷菜单。
/// - **核心优势**：通过 D-Bus StatusNotifierItem 与 DBusMenu 规范集成，支持右键菜单、平滑重启与即时中英切换。
/// - **代价与局限**：依赖 D-Bus 用户会话总线，若当前环境无 DBus 则自动优雅降级并向调用方汇报真实就绪态。
pub struct TrayService {
    shared_flags: SharedInputFlags,
    reload_requested: Arc<AtomicBool>,
    coordinator: ShutdownCoordinator,
    icon_update_tx: SyncSender<()>,
    icons: Arc<RwLock<ResolvedTrayIcons>>,
    dbus_ready: Arc<AtomicBool>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl TrayService {
    /// 使用停机协调器启动后台系统托盘常驻服务（独立运行时/测试便捷入口）
    pub fn spawn(coordinator: ShutdownCoordinator) -> Self {
        let config = AppConfig::load_or_create();
        let subprocess_manager = Arc::new(SubprocessManager::new(coordinator.flag()));
        let settings_running = Arc::new(AtomicBool::new(false));
        Self::spawn_with_config(&config, coordinator, subprocess_manager, settings_running)
    }

    /// 使用共享配置、协调器与会话唯一子进程管理器启动后台系统托盘常驻服务
    pub fn spawn_with_config(
        config: &AppConfig,
        coordinator: ShutdownCoordinator,
        subprocess_manager: Arc<SubprocessManager>,
        settings_running: Arc<AtomicBool>,
    ) -> Self {
        let shared_flags = SharedInputFlags::new(
            config.pinyin.init_chinese,
            config.pinyin.traditional_chinese,
            config.pinyin.full_width_punct,
            config.pinyin.full_width_char,
        );
        let reload_requested = Arc::new(AtomicBool::new(false));
        let (icon_update_tx, icon_update_rx) = mpsc::sync_channel::<()>(64);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<bool>(1);
        let icons = Arc::new(RwLock::new(resolve_tray_icons(config)));
        let dbus_ready = Arc::new(AtomicBool::new(false));

        let params = TrayLoopParams {
            icons: Arc::clone(&icons),
            shared_flags: shared_flags.clone(),
            reload_requested: Arc::clone(&reload_requested),
            coordinator: coordinator.clone(),
            subprocess_manager,
            settings_running,
            icon_update_tx: icon_update_tx.clone(),
            icon_update_rx,
            dbus_ready: Arc::clone(&dbus_ready),
            ready_tx,
        };

        let join_handle = match thread::Builder::new()
            .name("waylan-tray".to_string())
            .spawn(move || {
                Self::run_dbus_loop(params);
            }) {
            Ok(handle) => {
                // 等待托盘 D-Bus 会话建连结果（短窗口 400ms），避免连不上 DBus 时仍误抑制 IBus 原生菜单
                let ready = ready_rx
                    .recv_timeout(Duration::from_millis(400))
                    .unwrap_or(false);
                dbus_ready.store(ready, Ordering::Release);
                Some(handle)
            }
            Err(e) => {
                warn!(
                    "[系统托盘 警告] 创建托盘后台线程失败，已降级为无托盘模式: {}",
                    e
                );
                None
            }
        };

        Self {
            shared_flags,
            reload_requested,
            coordinator,
            icon_update_tx,
            icons,
            dbus_ready,
            join_handle,
        }
    }

    /// 托盘 D-Bus 服务是否已成功连接总线并导出对象
    #[must_use]
    pub fn is_dbus_ready(&self) -> bool {
        self.dbus_ready.load(Ordering::Acquire)
    }

    /// 克隆可传递给宿主热重载桥接器的轻量托盘图标同步句柄
    #[must_use]
    pub fn sync_handle(&self) -> TraySyncHandle {
        TraySyncHandle {
            icons: Arc::clone(&self.icons),
            icon_update_tx: self.icon_update_tx.clone(),
        }
    }

    /// 获取输入法共享状态标志集合句柄
    pub fn shared_flags(&self) -> &SharedInputFlags {
        &self.shared_flags
    }

    /// 获取配置与词库重载请求共享原子标志句柄
    pub fn reload_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.reload_requested)
    }

    /// 获取托盘图标与菜单状态刷新通知发送端句柄
    pub fn icon_notifier(&self) -> SyncSender<()> {
        self.icon_update_tx.clone()
    }

    /// 查询是否已收到优雅退出请求（仅供测试断言使用）
    #[cfg(test)]
    pub fn is_exit_requested(&self) -> bool {
        self.coordinator.is_shutting_down()
    }

    /// 获取当前是否处于中文模式（供单元测试断言使用）
    #[cfg(test)]
    pub fn is_chinese(&self) -> bool {
        self.shared_flags.is_chinese()
    }

    /// 更新当前输入模式并触发托盘图标变动通知（供单元测试验证使用）
    #[cfg(test)]
    pub fn update_mode(&self, mode: InputMode) {
        let is_zh = matches!(mode, InputMode::Chinese);
        let prev = self.shared_flags.is_chinese();
        self.shared_flags.set_chinese(is_zh);
        if prev != is_zh {
            let _ = self.icon_update_tx.try_send(());
        }
    }

    /// 初始化 StatusNotifierItem 与 DBusMenu 实例
    fn init_tray_objects(params: &TrayLoopParams) -> (StatusNotifierItem, DBusMenu) {
        let loop_wakeup = params.coordinator.loop_wakeup();
        let sni = StatusNotifierItem::with_shared_icons(
            params.shared_flags.clone(),
            params.icon_update_tx.clone(),
            Arc::clone(&params.icons),
            loop_wakeup,
        );
        let menu = DBusMenu::new(DBusMenuOptions {
            shared_flags: params.shared_flags.clone(),
            reload_requested: params.reload_requested.clone(),
            coordinator: params.coordinator.clone(),
            settings_running: Arc::clone(&params.settings_running),
            icon_update_tx: params.icon_update_tx.clone(),
            subprocess_manager: Arc::clone(&params.subprocess_manager),
        });
        (sni, menu)
    }

    /// 尝试连接 DBus 会话总线并注册托盘服务对象
    fn connect_dbus_session(
        sni: StatusNotifierItem,
        menu: DBusMenu,
        item_service: &str,
    ) -> Option<Connection> {
        let conn_res = zbus::block_on(async {
            Builder::session()
                .name(item_service)
                .serve_at("/StatusNotifierItem", sni)
                .serve_at("/MenuBar", menu)
                .build()
                .await
        });

        match conn_res {
            Ok(conn) => {
                info!(
                    "[系统托盘] 成功注册 DBus 托盘与右键菜单服务对象: {} (路径: /StatusNotifierItem, /MenuBar)",
                    item_service
                );
                Some(conn)
            }
            Err(e) => {
                warn!(
                    "[系统托盘 提示] 初始化托盘 DBus 对象失败 ({})，托盘指示器将安全降级。",
                    e
                );
                None
            }
        }
    }

    /// 探测并向系统托盘监视器 (StatusNotifierWatcher) 注册自身
    fn register_to_watcher(conn: &Connection, item_service: &str) {
        let has_watcher: bool = zbus::block_on(async {
            conn.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &("org.kde.StatusNotifierWatcher",),
            )
            .await
            .and_then(|reply| reply.body().deserialize::<bool>())
        })
        .unwrap_or(false);

        let mut registered = false;
        if has_watcher {
            for _ in 0..5 {
                registered = zbus::block_on(async {
                    conn.call_method(
                        Some("org.kde.StatusNotifierWatcher"),
                        "/StatusNotifierWatcher",
                        Some("org.kde.StatusNotifierWatcher"),
                        "RegisterStatusNotifierItem",
                        &(item_service,),
                    )
                    .await
                    .is_ok()
                        || conn
                            .call_method(
                                Some("org.kde.StatusNotifierWatcher"),
                                "/StatusNotifierWatcher",
                                Some("org.kde.StatusNotifierWatcher"),
                                "RegisterStatusNotifierItem",
                                &("/StatusNotifierItem",),
                            )
                            .await
                            .is_ok()
                });
                if registered {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }

        if registered {
            info!("[系统托盘] 已成功向系统任务栏 (Waybar/KDE/GNOME) 注册托盘图标！");
        } else {
            info!(
                "[系统托盘 提示] 当前环境暂未发现活跃 StatusNotifierWatcher，托盘服务保持后台就绪。"
            );
        }
    }

    /// 向 DBus 批量发射图标、提示与菜单刷新信号（收敛在单次异步块内执行）
    fn emit_tray_signals(conn: &Connection, sni: &StatusNotifierItem, menu_revision: &mut u32) {
        let current_icon = sni.icon_name();
        *menu_revision = menu_revision.wrapping_add(1);
        let rev = *menu_revision;

        let mut changed_props = HashMap::with_capacity(1);
        changed_props.insert("IconName", Variant::from(current_icon));
        let invalidated_props: Vec<&str> = Vec::new();

        zbus::block_on(async {
            emit_logged_signal(
                conn,
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
                "NewIcon",
                &(),
                "NewIcon 图标刷新信号",
            )
            .await;
            let _ = conn
                .emit_signal(
                    Option::<&str>::None,
                    "/StatusNotifierItem",
                    "org.kde.StatusNotifierItem",
                    "NewToolTip",
                    &(),
                )
                .await;
            let _ = conn
                .emit_signal(
                    Option::<&str>::None,
                    "/StatusNotifierItem",
                    "org.freedesktop.DBus.Properties",
                    "PropertiesChanged",
                    &(
                        "org.kde.StatusNotifierItem",
                        changed_props,
                        invalidated_props,
                    ),
                )
                .await;
            emit_logged_signal(
                conn,
                "/MenuBar",
                "com.canonical.dbusmenu",
                "LayoutUpdated",
                &(rev, 0i32),
                "LayoutUpdated 菜单刷新信号",
            )
            .await;
        });
    }

    /// 保持 DBus 事件处理循环运行并在收到状态变更时广播信号
    fn run_tray_event_loop(params: TrayLoopParams, sni: StatusNotifierItem, conn: Connection) {
        let mut menu_revision: u32 = 1;
        let mut last_emitted_is_chinese = params.shared_flags.is_chinese();
        let mut last_emitted_is_caps = params.shared_flags.is_caps_locked();

        loop {
            if params.coordinator.is_shutting_down() {
                break;
            }

            let received = params
                .icon_update_rx
                .recv_timeout(Duration::from_millis(500));
            if params.coordinator.is_shutting_down() {
                break;
            }

            while params.icon_update_rx.try_recv().is_ok() {}

            let current_is_chinese = params.shared_flags.is_chinese();
            let current_is_caps = params.shared_flags.is_caps_locked();

            let should_emit = received.is_ok()
                || current_is_chinese != last_emitted_is_chinese
                || current_is_caps != last_emitted_is_caps;

            if should_emit {
                last_emitted_is_chinese = current_is_chinese;
                last_emitted_is_caps = current_is_caps;
                Self::emit_tray_signals(&conn, &sni, &mut menu_revision);
            }
        }
    }

    /// 在独立线程中初始化 DBus 连接与注册 StatusNotifierItem 及 DBusMenu
    fn run_dbus_loop(params: TrayLoopParams) {
        let (sni, menu) = Self::init_tray_objects(&params);
        let pid = process::id();
        let item_service = format!("org.kde.StatusNotifierItem-{}-1", pid);

        let Some(conn) = Self::connect_dbus_session(sni.clone(), menu, &item_service) else {
            params.dbus_ready.store(false, Ordering::Release);
            let _ = params.ready_tx.try_send(false);
            return;
        };

        params.dbus_ready.store(true, Ordering::Release);
        let _ = params.ready_tx.try_send(true);
        Self::register_to_watcher(&conn, &item_service);
        Self::run_tray_event_loop(params, sni, conn);
    }
}

impl Drop for TrayService {
    fn drop(&mut self) {
        self.coordinator.request_shutdown();
        // 使用非阻塞 try_send 避免通道已满时阻塞死锁；通道已满说明已有待唤醒信号
        let _ = self.icon_update_tx.try_send(());
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// 发射单条托盘 D-Bus 信号并记录统一日志
async fn emit_logged_signal<B: serde::ser::Serialize + zbus::zvariant::Type>(
    conn: &Connection,
    path: &str,
    interface: &str,
    signal_name: &str,
    body: &B,
    label: &str,
) {
    if let Err(e) = conn
        .emit_signal(Option::<&str>::None, path, interface, signal_name, body)
        .await
    {
        warn!("[系统托盘] 发射 {} 失败: {}", label, e);
    } else {
        debug!("[系统托盘] 成功向 D-Bus 总线发射 {}", label);
    }
}

#[cfg(test)]
#[path = "../tray_tests.rs"]
mod tests;
