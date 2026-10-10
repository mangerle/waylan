use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use waylan_core::InputMode;
use zbus::blocking::connection::Builder;
use zbus::interface;

/// 系统托盘状态项实现 (StatusNotifierItem)
/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

pub struct StatusNotifierItem {
    /// 当前是否处于中文模式
    is_chinese: Arc<AtomicBool>,
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
    fn icon_name(&self) -> String {
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
    fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
        zbus::zvariant::OwnedObjectPath::try_from("/MenuBar").unwrap()
    }

    /// 用户左键点击托盘图标：触发中英文快速切换
    fn activate(&self, _x: i32, _y: i32) {
        let current = self.is_chinese.load(Ordering::Relaxed);
        let next = !current;
        self.is_chinese.store(next, Ordering::Relaxed);
        info!(
            "[系统托盘] 用户点击托盘图标，切换输入模式为: {}",
            if next { "中文" } else { "英文" }
        );
    }

    /// 用户右键点击托盘图标：触发上下文菜单响应
    fn context_menu(&self, x: i32, y: i32) {
        info!("[系统托盘] 用户右键点击托盘图标，坐标: ({}, {})", x, y);
    }
}

/// DBusMenu 布局返回元组类型定义
pub type DbusMenuLayout = (
    u32,
    (
        i32,
        std::collections::HashMap<String, zbus::zvariant::Value<'static>>,
        Vec<zbus::zvariant::Value<'static>>,
    ),
);

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    is_chinese: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
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
        let mode_text = if self.is_chinese.load(Ordering::Relaxed) {
            "当前输入: 中文拼音 (点击切换)"
        } else {
            "当前输入: 英文模式 (点击切换)"
        };

        // 构造子菜单项
        let mut menu_items = Vec::new();

        // 1. 模式切换
        let mut item1 = std::collections::HashMap::new();
        item1.insert("label".to_string(), zbus::zvariant::Value::from(mode_text));
        menu_items.push(zbus::zvariant::Value::from((1, item1, Vec::<zbus::zvariant::Value>::new())));

        // 2. 重新加载词库与配置
        let mut item2 = std::collections::HashMap::new();
        item2.insert("label".to_string(), zbus::zvariant::Value::from("重新加载词库与配置"));
        menu_items.push(zbus::zvariant::Value::from((2, item2, Vec::<zbus::zvariant::Value>::new())));

        // 3. 偏好设置
        let mut item3 = std::collections::HashMap::new();
        item3.insert("label".to_string(), zbus::zvariant::Value::from("偏好设置..."));
        menu_items.push(zbus::zvariant::Value::from((3, item3, Vec::<zbus::zvariant::Value>::new())));

        // 4. 查看运行日志
        let mut item4 = std::collections::HashMap::new();
        item4.insert("label".to_string(), zbus::zvariant::Value::from("查看运行日志 (~/.local/state/waylan/waylan.log)"));
        menu_items.push(zbus::zvariant::Value::from((4, item4, Vec::<zbus::zvariant::Value>::new())));

        // 5. 退出输入法
        let mut item5 = std::collections::HashMap::new();
        item5.insert("label".to_string(), zbus::zvariant::Value::from("退出微澜输入法"));
        menu_items.push(zbus::zvariant::Value::from((5, item5, Vec::<zbus::zvariant::Value>::new())));

        let root_props = std::collections::HashMap::new();
        (1, (0, root_props, menu_items))
    }

    /// 用户在右键菜单中点击特定项触发事件
    fn event(&self, id: i32, event_id: String, _data: zbus::zvariant::Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }

        match id {
            1 => {
                let current = self.is_chinese.load(Ordering::Relaxed);
                let next = !current;
                self.is_chinese.store(next, Ordering::Relaxed);
                info!(
                    "[系统托盘] 右键菜单切换输入模式为: {}",
                    if next { "中文" } else { "英文" }
                );
            }
            2 => {
                self.reload_requested.store(true, Ordering::Relaxed);
                info!("[系统托盘] 右键菜单触发：已发出重新加载词库与配置信号...");
            }
            3 => {
                info!("[系统托盘] 唤起微澜偏好设置界面...");
                let spawned = std::process::Command::new("waylan-settings")
                    .spawn()
                    .or_else(|_| {
                        std::process::Command::new("waylan")
                            .arg("settings")
                            .spawn()
                    });

                if spawned.is_err() {
                    if let Some(path) = crate::config::AppConfig::config_path() {
                        let _ = std::process::Command::new("xdg-open")
                            .arg(path)
                            .spawn();
                    }
                }
            }
            4 => {
                let log_path = crate::logger::log_file_path();
                info!("[系统托盘] 查看运行日志: {:?}", log_path);
                let _ = std::process::Command::new("xdg-open")
                    .arg(log_path)
                    .spawn();
            }
            5 => {
                info!("[系统托盘] 正在退出微澜输入法守护进程...");
                std::process::exit(0);
            }
            _ => {}
        }
    }
}

/// 系统托盘服务控制器
#[allow(dead_code)]
pub struct TrayService {
    is_chinese: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl TrayService {
    /// 启动后台系统托盘常驻服务
    pub fn spawn() -> Self {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&is_chinese);
        let reload_clone = Arc::clone(&reload_requested);

        thread::spawn(move || {
            Self::run_dbus_loop(flag_clone, reload_clone);
        });

        Self {
            is_chinese,
            reload_requested,
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

    /// 获取当前是否处于中文模式
    pub fn is_chinese(&self) -> bool {
        self.is_chinese.load(Ordering::Relaxed)
    }

    /// 更新当前输入模式并触发托盘图标变动
    pub fn update_mode(&self, mode: InputMode) {
        let is_zh = matches!(mode, InputMode::Chinese);
        self.is_chinese.store(is_zh, Ordering::Relaxed);
    }

    /// 在独立线程中初始化 DBus 连接与注册 StatusNotifierItem 及 DBusMenu
    fn run_dbus_loop(is_chinese: Arc<AtomicBool>, reload_requested: Arc<AtomicBool>) {
        let sni = StatusNotifierItem {
            is_chinese: Arc::clone(&is_chinese),
        };
        let menu = DBusMenu {
            is_chinese,
            reload_requested,
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

        // 保持 DBus 事件处理循环运行
        loop {
            thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tray_mode_toggle() {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let tray = TrayService {
            is_chinese: Arc::clone(&is_chinese),
            reload_requested,
        };

        assert!(tray.is_chinese());
        tray.update_mode(InputMode::English);
        assert!(!tray.is_chinese());
        tray.update_mode(InputMode::Chinese);
        assert!(tray.is_chinese());
    }

    #[test]
    fn test_tray_menu_layout_and_event() {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let menu = DBusMenu {
            is_chinese: Arc::clone(&is_chinese),
            reload_requested: Arc::clone(&reload_requested),
        };

        let (_rev, (_parent_id, _props, items)) = menu.get_layout(0, -1, vec![]);
        assert_eq!(items.len(), 5);

        // 模拟点击菜单第1项（模式切换）
        menu.event(1, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
        assert!(!is_chinese.load(Ordering::Relaxed));

        // 再次点击切回中文
        menu.event(1, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
        assert!(is_chinese.load(Ordering::Relaxed));

        // 模拟点击菜单第2项（重新加载词库与配置）
        assert!(!reload_requested.load(Ordering::Relaxed));
        menu.event(2, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
        assert!(reload_requested.load(Ordering::Relaxed));
    }
}
