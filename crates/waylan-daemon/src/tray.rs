use crate::config::AppConfig;
use crate::logger::log_file_path;
use log::{debug, info, warn};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
#[cfg(test)]
use waylan_core::InputMode;
use zbus::blocking::connection::Builder;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::{OwnedObjectPath, Value};

/// 系统托盘状态项实现 (StatusNotifierItem)
/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

pub struct StatusNotifierItem {
    /// 当前是否处于中文模式
    is_chinese: Arc<AtomicBool>,
    /// 图标与状态刷新通知发送端
    icon_update_tx: SyncSender<()>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    /// 类别
    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".to_string()
    }

    /// 唯一标识
    #[zbus(property)]
    fn id(&self) -> String {
        "waylan".to_string()
    }

    /// 显示标题
    #[zbus(property)]
    fn title(&self) -> String {
        "微澜输入法".to_string()
    }

    /// 状态（Active 表示常驻激活）
    #[zbus(property)]
    fn status(&self) -> String {
        "Active".to_string()
    }

    /// 图标名称（根据中英文状态动态切换）
    #[zbus(property)]
    pub(crate) fn icon_name(&self) -> String {
        if self.is_chinese.load(Ordering::Relaxed) {
            "input-keyboard".to_string()
        } else {
            "input-latin".to_string()
        }
    }

    /// 提示文本结构
    #[zbus(property)]
    fn tool_tip(&self) -> ToolTipData {
        let mode_str = if self.is_chinese.load(Ordering::Relaxed) {
            "中文拼音"
        } else {
            "英文模式"
        };
        (
            "waylan".to_string(),
            Vec::new(),
            "微澜输入法".to_string(),
            format!("当前工作状态: {}", mode_str),
        )
    }

    /// 上下文菜单 DBus 路径 (指向 /MenuBar)
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap_or_else(|e| {
            warn!(
                "[系统托盘] 解析 /MenuBar 对象路径失败: {}，使用根路径兜底",
                e
            );
            OwnedObjectPath::default()
        })
    }

    /// 用户左键点击托盘图标：触发中英文快速切换
    pub(crate) fn activate(&self, _x: i32, _y: i32) {
        let current = self.is_chinese.load(Ordering::Relaxed);
        let next = !current;
        self.is_chinese.store(next, Ordering::Relaxed);
        let _ = self.icon_update_tx.try_send(());
        info!(
            "[系统托盘] 用户点击托盘图标，切换输入模式为: {}",
            if next { "中文" } else { "英文" }
        );
    }

    /// 用户右键点击托盘图标：触发上下文菜单响应
    fn context_menu(&self, x: i32, y: i32) {
        info!("[系统托盘] 用户右键点击托盘图标，坐标: ({}, {})", x, y);
    }

    /// 图标更新通知信号（用于告知系统任务栏刷新图标呈现）
    #[zbus(signal)]
    async fn new_icon(signal_ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    /// 悬停提示更新通知信号
    #[zbus(signal)]
    async fn new_tool_tip(signal_ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

/// DBusMenu 布局返回元组类型定义
pub type DbusMenuLayout = (
    u32,
    (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>),
);

/// 构造单条 DBusMenu 菜单项结构
fn make_menu_item(id: i32, label: &str) -> Value<'static> {
    let mut props = HashMap::with_capacity(1);
    props.insert("label".to_string(), Value::from(label.to_string()));
    Value::from((id, props, Vec::<Value>::new()))
}

/// 异步等待并收割外部子进程退出状态，防止产生僵尸进程（<defunct>）
fn reap_child_process(mut child: Child, running_guard: Option<Arc<AtomicBool>>) {
    thread::spawn(move || {
        let _ = child.wait();
        if let Some(guard) = running_guard {
            guard.store(false, Ordering::Release);
        }
    });
}

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    is_chinese: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
    settings_running: Arc<AtomicBool>,
    icon_update_tx: SyncSender<()>,
}

#[interface(name = "com.canonical.dbusmenu")]
impl DBusMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "normal".to_string()
    }

    /// 获取托盘右键菜单布局
    fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> DbusMenuLayout {
        let mode_text = if self.is_chinese.load(Ordering::Acquire) {
            "当前输入: 中文拼音 (点击切换)"
        } else {
            "当前输入: 英文模式 (点击切换)"
        };

        let menu_items = vec![
            make_menu_item(1, mode_text),
            make_menu_item(2, "重新加载词库与配置"),
            make_menu_item(3, "偏好设置..."),
            make_menu_item(4, "查看运行日志 (~/.local/state/waylan/waylan.log)"),
            make_menu_item(5, "退出微澜输入法"),
        ];

        (1, (0, HashMap::new(), menu_items))
    }

    /// 用户在右键菜单中点击特定项触发事件
    fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }

        match id {
            1 => {
                let current = self.is_chinese.load(Ordering::Acquire);
                let next = !current;
                self.is_chinese.store(next, Ordering::Release);
                let _ = self.icon_update_tx.try_send(());
                info!(
                    "[系统托盘] 右键菜单切换输入模式为: {}",
                    if next { "中文" } else { "英文" }
                );
            }
            2 => {
                self.reload_requested.store(true, Ordering::Release);
                info!("[系统托盘] 右键菜单触发：已发出重新加载词库与配置信号...");
            }
            3 => {
                if self
                    .settings_running
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    info!("[系统托盘] 偏好设置窗口已在运行中，忽略重复唤起请求。");
                    return;
                }
                info!("[系统托盘] 唤起微澜偏好设置界面...");
                let current_exe = std::env::current_exe().ok();
                let sibling_settings = current_exe
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|d| d.join("waylan-settings"))
                    .filter(|p| p.is_file());

                let mut cmd1 = if let Some(ref path) = sibling_settings {
                    Command::new(path)
                } else {
                    Command::new("waylan-settings")
                };
                #[cfg(unix)]
                cmd1.process_group(0);

                let mut cmd2 = if let Some(ref path) = current_exe {
                    Command::new(path)
                } else {
                    Command::new("waylan")
                };
                cmd2.arg("settings");
                #[cfg(unix)]
                cmd2.process_group(0);

                let spawned = cmd1.spawn().or_else(|_| cmd2.spawn());

                match spawned {
                    Ok(child) => {
                        reap_child_process(child, Some(Arc::clone(&self.settings_running)));
                    }
                    Err(_) => {
                        self.settings_running.store(false, Ordering::Release);
                        if let Some(path) = AppConfig::config_path() {
                            let xdg_bin = ["/usr/bin/xdg-open", "/bin/xdg-open"]
                                .iter()
                                .find(|p| std::path::Path::new(p).exists())
                                .copied()
                                .unwrap_or("xdg-open");
                            let mut xdg_cmd = Command::new(xdg_bin);
                            xdg_cmd.arg(path);
                            #[cfg(unix)]
                            xdg_cmd.process_group(0);
                            if let Ok(child) = xdg_cmd.spawn() {
                                reap_child_process(child, None);
                            }
                        }
                    }
                }
            }
            4 => {
                let log_path = log_file_path();
                info!("[系统托盘] 查看运行日志: {:?}", log_path);
                let xdg_bin = ["/usr/bin/xdg-open", "/bin/xdg-open"]
                    .iter()
                    .find(|p| std::path::Path::new(p).exists())
                    .copied()
                    .unwrap_or("xdg-open");
                let mut log_cmd = Command::new(xdg_bin);
                log_cmd.arg(log_path);
                #[cfg(unix)]
                log_cmd.process_group(0);
                if let Ok(child) = log_cmd.spawn() {
                    reap_child_process(child, None);
                }
            }
            5 => {
                info!("[系统托盘] 收到退出菜单指令，正在触发优雅停机与数据落盘流程...");
                self.exit_requested.store(true, Ordering::Release);
                let _ = self.icon_update_tx.try_send(());
            }
            _ => {}
        }
    }

    /// 菜单布局更新通知信号
    #[zbus(signal)]
    async fn layout_updated(
        signal_ctxt: &SignalContext<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// 系统托盘服务控制器
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
        let sni = StatusNotifierItem {
            is_chinese: Arc::clone(&is_chinese),
            icon_update_tx: icon_update_tx.clone(),
        };
        let menu = DBusMenu {
            is_chinese,
            reload_requested,
            exit_requested: Arc::clone(&exit_requested),
            settings_running: Arc::new(AtomicBool::new(false)),
            icon_update_tx,
        };

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
        let _ = self.icon_update_tx.try_send(());
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
#[path = "tray_tests.rs"]
mod tests;
