pub(crate) mod item;
pub(crate) mod menu;

use crate::config::AppConfig;
use crate::system::ShutdownCoordinator;
use log::{debug, info, warn};
use menu::{DBusMenu, DBusMenuOptions};
use std::collections::HashMap;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
#[cfg(test)]
use waylan_core::InputMode;
use zbus::blocking::connection::Builder;
use zbus::zvariant::Value;

pub use item::StatusNotifierItem;

/// 托盘后台事件循环参数包
struct TrayLoopParams {
    is_chinese: Arc<AtomicBool>,
    is_caps_locked: Arc<AtomicBool>,
    traditional_chinese: Arc<AtomicBool>,
    full_width_punct: Arc<AtomicBool>,
    full_width_char: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    coordinator: ShutdownCoordinator,
    icon_update_tx: SyncSender<()>,
    icon_update_rx: Receiver<()>,
}

/// 系统托盘服务控制器
///
/// # 设计原理
/// - **实现初衷**：在现代 Linux Wayland 任务栏（Waybar/KDE/GNOME）呈现常驻输入法托盘，提供状态展示与快捷菜单。
/// - **核心优势**：通过 D-Bus StatusNotifierItem 与 DBusMenu 规范集成，支持右键菜单、平滑重启与即时中英切换。
/// - **代价与局限**：依赖 D-Bus 用户会话总线，若当前环境无 DBus 则自动优雅降级。
pub struct TrayService {
    is_chinese: Arc<AtomicBool>,
    is_caps_locked: Arc<AtomicBool>,
    traditional_chinese: Arc<AtomicBool>,
    full_width_punct: Arc<AtomicBool>,
    full_width_char: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    coordinator: ShutdownCoordinator,
    icon_update_tx: SyncSender<()>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl TrayService {
    /// 使用停机协调器启动后台系统托盘常驻服务
    pub fn spawn(coordinator: ShutdownCoordinator) -> Self {
        let config = AppConfig::load_or_create();
        let is_chinese = Arc::new(AtomicBool::new(config.init_chinese));
        let is_caps_locked = Arc::new(AtomicBool::new(false));
        let traditional_chinese = Arc::new(AtomicBool::new(config.traditional_chinese));
        let full_width_punct = Arc::new(AtomicBool::new(config.full_width_punct));
        let full_width_char = Arc::new(AtomicBool::new(config.full_width_char));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let (icon_update_tx, icon_update_rx) = mpsc::sync_channel::<()>(64);

        let params = TrayLoopParams {
            is_chinese: Arc::clone(&is_chinese),
            is_caps_locked: Arc::clone(&is_caps_locked),
            traditional_chinese: Arc::clone(&traditional_chinese),
            full_width_punct: Arc::clone(&full_width_punct),
            full_width_char: Arc::clone(&full_width_char),
            reload_requested: Arc::clone(&reload_requested),
            coordinator: coordinator.clone(),
            icon_update_tx: icon_update_tx.clone(),
            icon_update_rx,
        };

        let handle = thread::spawn(move || {
            Self::run_dbus_loop(params);
        });

        Self {
            is_chinese,
            is_caps_locked,
            traditional_chinese,
            full_width_punct,
            full_width_char,
            reload_requested,
            coordinator,
            icon_update_tx,
            join_handle: Some(handle),
        }
    }

    /// 获取中英文模式共享原子标志句柄
    pub fn mode_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_chinese)
    }

    /// 获取 CapsLock 大写锁定状态共享原子标志句柄
    pub fn caps_lock_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_caps_locked)
    }

    /// 获取简繁切换共享原子标志句柄
    pub fn traditional_chinese_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.traditional_chinese)
    }

    /// 获取全角/半角标点共享原子标志句柄
    pub fn full_width_punct_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.full_width_punct)
    }

    /// 获取全角/半角字符共享原子标志句柄
    pub fn full_width_char_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.full_width_char)
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
        self.is_chinese.load(Ordering::Acquire)
    }

    /// 更新当前输入模式并触发托盘图标变动通知（供单元测试验证使用）
    #[cfg(test)]
    pub fn update_mode(&self, mode: InputMode) {
        let is_zh = matches!(mode, InputMode::Chinese);
        let prev = self.is_chinese.swap(is_zh, Ordering::AcqRel);
        if prev != is_zh {
            let _ = self.icon_update_tx.try_send(());
        }
    }

    /// 初始化 StatusNotifierItem 与 DBusMenu 实例
    fn init_tray_objects(params: &TrayLoopParams) -> (StatusNotifierItem, DBusMenu) {
        let config = AppConfig::load_or_create();
        let loop_wakeup = params.coordinator.loop_wakeup();
        let sni = StatusNotifierItem::new(
            Arc::clone(&params.is_chinese),
            Arc::clone(&params.is_caps_locked),
            params.icon_update_tx.clone(),
            &config,
            loop_wakeup,
        );
        let menu = DBusMenu::new(DBusMenuOptions {
            is_chinese: Arc::clone(&params.is_chinese),
            is_caps_locked: Arc::clone(&params.is_caps_locked),
            traditional_chinese: params.traditional_chinese.clone(),
            full_width_punct: params.full_width_punct.clone(),
            full_width_char: params.full_width_char.clone(),
            reload_requested: params.reload_requested.clone(),
            coordinator: params.coordinator.clone(),
            settings_running: Arc::new(AtomicBool::new(false)),
            icon_update_tx: params.icon_update_tx.clone(),
        });
        (sni, menu)
    }

    /// 尝试连接 DBus 会话总线并注册托盘服务对象
    fn connect_dbus_session(
        sni: StatusNotifierItem,
        menu: DBusMenu,
        item_service: &str,
    ) -> Option<zbus::blocking::Connection> {
        let builder = match Builder::session() {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "[系统托盘 提示] 无法连接到 DBus 会话总线 ({})，跳过任务栏托盘注册。",
                    e
                );
                return None;
            }
        };

        match builder
            .name(item_service)
            .and_then(|b| b.serve_at("/StatusNotifierItem", sni))
            .and_then(|b| b.serve_at("/MenuBar", menu))
            .and_then(|b| b.build())
        {
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
    fn register_to_watcher(conn: &zbus::blocking::Connection, item_service: &str) {
        let has_watcher: bool = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &("org.kde.StatusNotifierWatcher",),
            )
            .and_then(|reply| reply.body().deserialize::<bool>())
            .unwrap_or(false);

        let mut registered = false;
        if has_watcher {
            for _ in 0..5 {
                registered = conn
                    .call_method(
                        Some("org.kde.StatusNotifierWatcher"),
                        "/StatusNotifierWatcher",
                        Some("org.kde.StatusNotifierWatcher"),
                        "RegisterStatusNotifierItem",
                        &(item_service,),
                    )
                    .is_ok()
                    || conn
                        .call_method(
                            Some("org.kde.StatusNotifierWatcher"),
                            "/StatusNotifierWatcher",
                            Some("org.kde.StatusNotifierWatcher"),
                            "RegisterStatusNotifierItem",
                            &("/StatusNotifierItem",),
                        )
                        .is_ok();
                if registered {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }

        if registered {
            info!("[系统托盘] 已成功向系统任务栏 (Waybar/KDE/GNOME) 注册托盘图标！");
        } else {
            info!("[系统托盘 提示] 当前环境暂未发现活跃 StatusNotifierWatcher，托盘服务保持后台就绪。");
        }
    }

    /// 向 DBus 发射图标、提示与菜单刷新信号
    fn emit_tray_signals(
        conn: &zbus::blocking::Connection,
        sni: &StatusNotifierItem,
        menu_revision: &mut u32,
    ) {
        let current_icon = sni.icon_name();

        if let Err(e) = conn.emit_signal(
            Option::<&str>::None,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "NewIcon",
            &(),
        ) {
            warn!("[系统托盘] 发射 NewIcon 图标刷新信号失败: {}", e);
        } else {
            debug!("[系统托盘] 成功向 D-Bus 总线发射 NewIcon 图标刷新信号");
        }

        let _ = conn.emit_signal(
            Option::<&str>::None,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "NewToolTip",
            &(),
        );

        let mut changed_props = HashMap::with_capacity(1);
        changed_props.insert("IconName", Value::from(current_icon));
        let invalidated_props: Vec<&str> = Vec::new();
        let _ = conn.emit_signal(
            Option::<&str>::None,
            "/StatusNotifierItem",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.kde.StatusNotifierItem",
                changed_props,
                invalidated_props,
            ),
        );

        *menu_revision = menu_revision.wrapping_add(1);
        if let Err(e) = conn.emit_signal(
            Option::<&str>::None,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "LayoutUpdated",
            &(*menu_revision, 0i32),
        ) {
            warn!("[系统托盘] 发射 LayoutUpdated 菜单刷新信号失败: {}", e);
        } else {
            debug!(
                "[系统托盘] 成功向 D-Bus 总线发射 LayoutUpdated 菜单刷新信号 (版本 {})",
                menu_revision
            );
        }
    }

    /// 保持 DBus 事件处理循环运行并在收到状态变更时广播信号
    fn run_tray_event_loop(
        params: TrayLoopParams,
        sni: StatusNotifierItem,
        conn: zbus::blocking::Connection,
    ) {
        let mut menu_revision: u32 = 1;
        let mut last_emitted_is_chinese = params.is_chinese.load(Ordering::Acquire);
        let mut last_emitted_is_caps = params.is_caps_locked.load(Ordering::Acquire);

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

            let current_is_chinese = params.is_chinese.load(Ordering::Acquire);
            let current_is_caps = params.is_caps_locked.load(Ordering::Acquire);

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
            return;
        };

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

#[cfg(test)]
#[path = "../tray_tests.rs"]
mod tests;
