use crate::config::AppConfig;
use crate::logger::log_file_path;
use log::info;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
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

/// 被纳管的外部子进程条目
struct TrackedChild {
    child: Child,
    running_guard: Option<Arc<AtomicBool>>,
    name: &'static str,
}

/// 外部子进程生命周期管理器
///
/// # 设计原理
/// - **实现初衷**：统一纳管从托盘菜单或辅助工具派生的外部子进程，避免裸线程阻塞等待与脱缰孤儿进程。
/// - **核心优势**：通过单一定时守护线程以非阻塞方式 (`try_wait`) 统一轮询回收退出子进程，在主进程退出或 Drop 时优雅清理未终止子进程 (`kill` 与 `wait`)，杜绝 PCB 残留与线程泄漏。
/// - **代价与局限**：后台运行一个休眠 500ms 的轻量管理线程，开销极小。
pub(crate) struct SubprocessManager {
    children: Arc<Mutex<Vec<TrackedChild>>>,
    shutdown: Arc<AtomicBool>,
    reaper_handle: Option<thread::JoinHandle<()>>,
}

impl SubprocessManager {
    pub(crate) fn new(exit_requested: Arc<AtomicBool>) -> Self {
        let children = Arc::new(Mutex::new(Vec::<TrackedChild>::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let children_clone = Arc::clone(&children);
        let shutdown_clone = Arc::clone(&shutdown);

        let reaper_handle = thread::spawn(move || {
            while !shutdown_clone.load(Ordering::Acquire) && !exit_requested.load(Ordering::Acquire)
            {
                thread::sleep(std::time::Duration::from_millis(200));
                if let Ok(mut lock) = children_clone.lock() {
                    lock.retain_mut(|tracked| match tracked.child.try_wait() {
                        Ok(Some(status)) => {
                            log::debug!(
                                "[系统托盘] 外部子进程 [{}] 已正常退出: {:?}",
                                tracked.name,
                                status
                            );
                            if let Some(ref guard) = tracked.running_guard {
                                guard.store(false, Ordering::Release);
                            }
                            false
                        }
                        Ok(None) => true,
                        Err(e) => {
                            log::warn!(
                                "[系统托盘] 查询外部子进程 [{}] 状态失败: {}",
                                tracked.name,
                                e
                            );
                            if let Some(ref guard) = tracked.running_guard {
                                guard.store(false, Ordering::Release);
                            }
                            false
                        }
                    });
                }
            }

            // 收到退出信号后，主动清理所有存活子进程并收割状态
            if let Ok(mut lock) = children_clone.lock() {
                for tracked in lock.iter_mut() {
                    info!(
                        "[系统托盘] 正在终止被纳管的外部子进程 [{}]...",
                        tracked.name
                    );
                    let _ = tracked.child.kill();
                    let _ = tracked.child.wait();
                    if let Some(ref guard) = tracked.running_guard {
                        guard.store(false, Ordering::Release);
                    }
                }
                lock.clear();
            }
        });

        Self {
            children,
            shutdown,
            reaper_handle: Some(reaper_handle),
        }
    }

    /// 注册纳管新启动的外部子进程
    pub(crate) fn track(
        &self,
        child: Child,
        running_guard: Option<Arc<AtomicBool>>,
        name: &'static str,
    ) {
        if let Ok(mut lock) = self.children.lock() {
            lock.push(TrackedChild {
                child,
                running_guard,
                name,
            });
        }
    }

    /// 唤起偏好设置图形界面进程
    pub(crate) fn launch_settings(&self, settings_running: &Arc<AtomicBool>) {
        if settings_running
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

        let mut cmd = if let Some(ref path) = sibling_settings {
            Command::new(path)
        } else {
            Command::new("waylan-settings")
        };
        #[cfg(unix)]
        cmd.process_group(0);

        for var in [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CURRENT_DESKTOP",
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, &val);
            }
        }

        match cmd.spawn() {
            Ok(child) => {
                self.track(child, Some(Arc::clone(settings_running)), "偏好设置界面");
            }
            Err(e) => {
                log::warn!(
                    "[系统托盘] 无法启动图形偏好设置二进制 waylan-settings ({})，回退使用文本编辑器打开配置文件。",
                    e
                );
                settings_running.store(false, Ordering::Release);
                self.launch_config_file_fallback();
            }
        }
    }

    /// 备选回退方案：调用默认文本编辑器打开配置文件
    pub(crate) fn launch_config_file_fallback(&self) {
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
                self.track(child, None, "配置文件文本编辑器");
            }
        }
    }

    /// 打开系统运行日志文件
    pub(crate) fn launch_view_log(&self) {
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
            self.track(child, None, "运行日志查看器");
        }
    }
}

impl Drop for SubprocessManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.reaper_handle.take() {
            let _ = handle.join();
        }
    }
}

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    pub(crate) is_chinese: Arc<AtomicBool>,
    pub(crate) is_caps_locked: Arc<AtomicBool>,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) exit_requested: Arc<AtomicBool>,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
    pub(crate) loop_wakeup: Option<LoopWakeup>,
    pub(crate) subprocess_manager: Arc<SubprocessManager>,
}

impl DBusMenu {
    pub(crate) fn new(
        is_chinese: Arc<AtomicBool>,
        is_caps_locked: Arc<AtomicBool>,
        reload_requested: Arc<AtomicBool>,
        exit_requested: Arc<AtomicBool>,
        settings_running: Arc<AtomicBool>,
        icon_update_tx: SyncSender<()>,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        let subprocess_manager = Arc::new(SubprocessManager::new(Arc::clone(&exit_requested)));
        Self {
            is_chinese,
            is_caps_locked,
            reload_requested,
            exit_requested,
            settings_running,
            icon_update_tx,
            loop_wakeup,
            subprocess_manager,
        }
    }

    fn launch_settings(&self) {
        self.subprocess_manager
            .launch_settings(&self.settings_running);
    }

    fn launch_view_log(&self) {
        self.subprocess_manager.launch_view_log();
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
        let mode_text = if self.is_caps_locked.load(Ordering::Acquire) {
            "当前输入: 英文大写"
        } else if self.is_chinese.load(Ordering::Acquire) {
            "当前输入: 中文拼音"
        } else {
            "当前输入: 英文模式"
        };
        let menu_items = vec![
            make_menu_item(1, mode_text),
            make_menu_item(2, "重载词库与配置"),
            make_menu_item(3, "偏好设置"),
            make_menu_item(4, "查看运行日志"),
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
        let is_caps = Arc::new(AtomicBool::new(false));
        let reload = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);

        let menu = DBusMenu::new(
            is_chinese.clone(),
            is_caps.clone(),
            reload,
            exit,
            settings,
            tx,
            None,
        );
        let (_rev, (_root_id, _props, items)) = menu.get_layout(0, -1, Vec::new());
        assert_eq!(items.len(), 5);

        // 验证默认中文状态文本
        let item1_str = format!("{:?}", items[0]);
        assert!(item1_str.contains("当前输入: 中文拼音"));

        // 开启大写锁定，验证菜单第1项文本正确变为英文大写
        is_caps.store(true, Ordering::Release);
        let (_rev, (_root_id, _props, items_caps)) = menu.get_layout(0, -1, Vec::new());
        let item1_caps_str = format!("{:?}", items_caps[0]);
        assert!(item1_caps_str.contains("当前输入: 英文大写"));

        let expected_label = "查看运行日志";

        // 验证菜单项包含真实的日志文件路径
        let item4 = &items[3];
        let serialized = format!("{:?}", item4);
        assert!(
            serialized.contains(expected_label),
            "菜单项中应包含实际日志绝对路径"
        );
    }

    #[test]
    fn test_subprocess_manager_lifecycle() {
        let exit_requested = Arc::new(AtomicBool::new(false));
        let manager = SubprocessManager::new(Arc::clone(&exit_requested));
        let running = Arc::new(AtomicBool::new(true));

        let child = Command::new("true").spawn().expect("执行 true 失败");
        manager.track(child, Some(Arc::clone(&running)), "测试短进程");

        // 等待轮询周期收割子进程
        thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            !running.load(Ordering::Acquire),
            "子进程退出后 running_guard 应被自动收割置为 false"
        );
    }
}
