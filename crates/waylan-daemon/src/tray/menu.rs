use crate::config::AppConfig;
use crate::logger::log_file_path;
use log::info;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::thread;
use waylan_wayland::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

/// DBusMenu 布局返回元组类型定义
pub type DbusMenuLayout = (
    u32,
    (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>),
);

/// 构造单条 DBusMenu 菜单项结构
pub(crate) fn make_menu_item(id: i32, label: &str) -> Value<'static> {
    let mut props = HashMap::with_capacity(1);
    props.insert("label".to_string(), Value::from(label.to_string()));
    Value::from((id, props, Vec::<Value>::new()))
}

/// 异步等待并收割外部子进程退出状态，防止产生僵尸进程
pub(crate) fn reap_child_process(mut child: Child, running_guard: Option<Arc<AtomicBool>>) {
    thread::spawn(move || {
        let _ = child.wait();
        if let Some(guard) = running_guard {
            guard.store(false, Ordering::Release);
        }
    });
}

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    pub(crate) is_chinese: Arc<AtomicBool>,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) exit_requested: Arc<AtomicBool>,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
    pub(crate) loop_wakeup: Option<LoopWakeup>,
}

impl DBusMenu {
    pub(crate) fn new(
        is_chinese: Arc<AtomicBool>,
        reload_requested: Arc<AtomicBool>,
        exit_requested: Arc<AtomicBool>,
        settings_running: Arc<AtomicBool>,
        icon_update_tx: SyncSender<()>,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        Self {
            is_chinese,
            reload_requested,
            exit_requested,
            settings_running,
            icon_update_tx,
            loop_wakeup,
        }
    }

    /// 唤起偏好设置图形界面进程
    fn launch_settings(&self) {
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

        for var in [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CURRENT_DESKTOP",
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd1.env(var, &val);
                cmd2.env(var, &val);
            }
        }

        let spawned = cmd1.spawn().or_else(|_| cmd2.spawn());
        match spawned {
            Ok(child) => {
                reap_child_process(child, Some(Arc::clone(&self.settings_running)));
            }
            Err(_) => {
                self.settings_running.store(false, Ordering::Release);
                self.launch_config_file_fallback();
            }
        }
    }

    /// 备选回退方案：调用默认文本编辑器打开配置文件
    fn launch_config_file_fallback(&self) {
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

    /// 打开系统运行日志文件
    fn launch_view_log(&self) {
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
    pub(crate) fn get_layout(
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

        let log_path = log_file_path();
        let log_label = format!("查看运行日志 ({})", log_path.display());

        let menu_items = vec![
            make_menu_item(1, mode_text),
            make_menu_item(2, "重新加载词库与配置"),
            make_menu_item(3, "偏好设置..."),
            make_menu_item(4, &log_label),
            make_menu_item(5, "退出微澜输入法"),
        ];

        (1, (0, HashMap::new(), menu_items))
    }

    /// 用户在右键菜单中点击特定项触发事件
    pub(crate) fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }

        match id {
            1 => {
                let current = self.is_chinese.load(Ordering::Acquire);
                let next = !current;
                self.is_chinese.store(next, Ordering::Release);
                let _ = self.icon_update_tx.try_send(());
                if let Some(ref wakeup) = self.loop_wakeup {
                    wakeup.wake();
                }
                info!(
                    "[系统托盘] 右键菜单切换输入模式为: {}",
                    if next { "中文" } else { "英文" }
                );
            }
            2 => {
                self.reload_requested.store(true, Ordering::Release);
                if let Some(ref wakeup) = self.loop_wakeup {
                    wakeup.wake();
                }
                info!("[系统托盘] 右键菜单触发：已发出重新加载词库与配置信号...");
            }
            3 => {
                self.launch_settings();
            }
            4 => {
                self.launch_view_log();
            }
            5 => {
                info!("[系统托盘] 收到退出菜单指令，正在触发优雅停机与数据落盘流程...");
                self.exit_requested.store(true, Ordering::Release);
                let _ = self.icon_update_tx.try_send(());
                if let Some(ref wakeup) = self.loop_wakeup {
                    wakeup.wake();
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_menu_layout_contains_dynamic_log_path() {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let reload = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);

        let menu = DBusMenu::new(is_chinese, reload, exit, settings, tx, None);
        let (_rev, (_root_id, _props, items)) = menu.get_layout(0, -1, Vec::new());
        assert_eq!(items.len(), 5);

        let log_path = log_file_path();
        let expected_label = format!("查看运行日志 ({})", log_path.display());

        // 验证菜单项包含真实的日志文件路径
        let item4 = &items[3];
        let serialized = format!("{:?}", item4);
        assert!(
            serialized.contains(&expected_label),
            "菜单项中应包含实际日志绝对路径"
        );
    }
}
