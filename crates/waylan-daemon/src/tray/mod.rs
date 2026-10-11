pub(crate) mod item;
pub(crate) mod menu;

use crate::config::AppConfig;
use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
#[cfg(test)]
use waylan_core::InputMode;
use zbus::blocking::connection::Builder;

pub use item::StatusNotifierItem;
pub use menu::DBusMenu;

/// 系统托盘服务控制器
///
/// # 设计原理
/// - **实现初衷**：在现代 Linux Wayland 任务栏（Waybar/KDE/GNOME）呈现常驻输入法托盘，提供状态展示与快捷菜单。
/// - **核心优势**：通过 D-Bus StatusNotifierItem 与 DBusMenu 规范集成，支持右键菜单、平滑重启与即时中英切换。
/// - **代价与局限**：依赖 D-Bus 用户会话总线，若当前环境无 DBus 则自动优雅降级。
pub struct TrayService {
    is_chinese: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
    icon_update_tx: SyncSender<()>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl TrayService {
    /// 使用全局退出标志启动后台系统托盘常驻服务（实现与进程信号 SIGINT/SIGTERM 共享同一退出源）
    pub fn spawn(exit_requested: Arc<AtomicBool>) -> Self {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let (icon_update_tx, icon_update_rx) = mpsc::sync_channel::<()>(8);

        let flag_clone = Arc::clone(&is_chinese);
        let reload_clone = Arc::clone(&reload_requested);
        let exit_clone = Arc::clone(&exit_requested);
        let tx_clone = icon_update_tx.clone();

        let handle = thread::spawn(move || {
            Self::run_dbus_loop(
                flag_clone,
                reload_clone,
                exit_clone,
                tx_clone,
                icon_update_rx,
            );
        });

        Self {
            is_chinese,
            reload_requested,
            exit_requested,
            icon_update_tx,
            join_handle: Some(handle),
        }
    }

    /// 获取中英文模式共享原子标志句柄
    pub fn mode_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_chinese)
    }

    /// 获取配置与词库重载请求共享原子标志句柄
    pub fn reload_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.reload_requested)
    }

    /// 获取优雅退出请求共享原子标志句柄
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exit_requested)
    }

    /// 获取托盘图标与菜单状态刷新通知发送端句柄
    pub fn icon_notifier(&self) -> SyncSender<()> {
        self.icon_update_tx.clone()
    }

    /// 查询是否已收到优雅退出请求
    pub fn is_exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
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

    /// 在独立线程中初始化 DBus 连接与注册 StatusNotifierItem 及 DBusMenu
    fn run_dbus_loop(
        is_chinese: Arc<AtomicBool>,
        reload_requested: Arc<AtomicBool>,
        exit_requested: Arc<AtomicBool>,
        icon_update_tx: SyncSender<()>,
        icon_update_rx: Receiver<()>,
    ) {
        let config = AppConfig::load_or_create();
        let sni = StatusNotifierItem::new(Arc::clone(&is_chinese), icon_update_tx.clone(), &config);
        let menu = DBusMenu::new(
            is_chinese,
            reload_requested,
            Arc::clone(&exit_requested),
            Arc::new(AtomicBool::new(false)),
            icon_update_tx,
        );

        // 尝试连接用户 DBus 会话总线
        let conn = match Builder::session() {
            Ok(builder) => match builder
                .name("org.kde.StatusNotifierItem.waylan")
                .and_then(|b| b.serve_at("/StatusNotifierItem", sni))
                .and_then(|b| b.serve_at("/MenuBar", menu))
                .and_then(|b| b.build())
            {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "[系统托盘 提示] 初始化托盘 DBus 对象失败 ({})，托盘指示器将安全降级。",
                        e
                    );
                    return;
                }
            },
            Err(e) => {
                warn!(
                    "[系统托盘 提示] 无法连接到 DBus 会话总线 ({})，跳过任务栏托盘注册。",
                    e
                );
                return;
            }
        };

        info!("[系统托盘] 成功注册 DBus 托盘与右键菜单服务对象: /StatusNotifierItem, /MenuBar");

        // 尝试向系统托盘监视器注册自身
        if let Err(e) = conn.call_method(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            Some("org.kde.StatusNotifierWatcher"),
            "RegisterStatusNotifierItem",
            &("/StatusNotifierItem",),
        ) {
            info!(
                "[系统托盘 提示] 当前环境未运行 StatusNotifierWatcher ({})，托盘服务保持后台就绪。",
                e
            );
        } else {
            info!("[系统托盘] 已成功向系统任务栏 (Waybar/KDE/GNOME) 注册托盘图标！");
        }

        // 保持 DBus 事件处理循环运行，并在收到模式切换通知时发射 NewIcon、NewToolTip 与 LayoutUpdated 信号
        let mut menu_revision: u32 = 1;
        while let Ok(()) = icon_update_rx.recv() {
            if exit_requested.load(Ordering::Acquire) {
                break;
            }
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

            menu_revision = menu_revision.wrapping_add(1);
            if let Err(e) = conn.emit_signal(
                Option::<&str>::None,
                "/MenuBar",
                "com.canonical.dbusmenu",
                "LayoutUpdated",
                &(menu_revision, 0i32),
            ) {
                warn!("[系统托盘] 发射 LayoutUpdated 菜单刷新信号失败: {}", e);
            } else {
                debug!(
                    "[系统托盘] 成功向 D-Bus 总线发射 LayoutUpdated 菜单刷新信号 (版本 {})",
                    menu_revision
                );
            }
        }
    }
}

impl Drop for TrayService {
    fn drop(&mut self) {
        self.exit_requested.store(true, Ordering::Release);
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
