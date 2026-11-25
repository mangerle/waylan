//! 系统守护进程外部子进程生命周期治理与防僵尸进程模块
//!
//! # 设计原理
//! - **实现初衷**：统一纳管从系统守护进程、托盘菜单或 IBus 交互派生的外部子进程，避免裸线程阻塞等待与脱缰孤儿进程。
//! - **核心优势**：通过单一定时守护线程以非阻塞方式 (`try_wait`) 统一轮询回收退出子进程，在主进程退出或 Drop 时优雅清理未终止子进程 (`kill` 与 `wait`)，杜绝 PCB 残留与线程泄漏。
//! - **代价与局限**：后台运行一个休眠 200ms 的轻量管理线程，开销极小。

use crate::config::AppConfig;
use crate::logger::log_file_path;
use log::info;
use std::env;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use parking_lot::Mutex;

/// 构建微澜偏好设置中心外部进程命令实例并注入关键会话环境变量
pub fn build_settings_command() -> Command {
    let current_exe = env::current_exe().ok();
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
    inject_desktop_env(&mut cmd);
    cmd
}

/// 外部子进程退场治理策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExitPolicy {
    /// 守护进程退出时终止并收割该附属子进程（发送 SIGTERM/SIGKILL 并等待退出）
    KillOnExit,
    /// 用户自主打开的独立交互应用（如偏好设置中心、编辑器），守护进程重启/退出时不强杀，仅做末次非阻塞收割后脱管
    DetachOnExit,
}

/// 被纳管的外部子进程条目
pub(crate) struct TrackedChild {
    pub(crate) child: Child,
    pub(crate) running_guard: Option<Arc<AtomicBool>>,
    pub(crate) name: &'static str,
    pub(crate) exit_policy: ChildExitPolicy,
}

impl TrackedChild {
    /// 标记该子进程已不再运行，清除运行态指示标志
    fn mark_finished(&self) {
        if let Some(ref guard) = self.running_guard {
            guard.store(false, Ordering::Release);
        }
    }
}

/// 收割已退出或状态查询失败的子进程条目，及时回收 PCB 杜绝僵尸进程堆积
fn reap_exited_children(children: &Mutex<Vec<TrackedChild>>) {
    let mut lock = children.lock();
    lock.retain_mut(|tracked| match tracked.child.try_wait() {
        Ok(Some(status)) => {
            log::debug!(
                "[进程治理] 外部子进程 [{}] 已正常退出: {:?}",
                tracked.name,
                status
            );
            tracked.mark_finished();
            false
        }
        Ok(None) => true,
        Err(e) => {
            log::warn!(
                "[进程治理] 查询外部子进程 [{}] 状态失败: {}",
                tracked.name,
                e
            );
            tracked.mark_finished();
            false
        }
    });
}

/// 收到退出信号后按退场策略清理子进程：强管控子进程收割退出，用户独立交互应用保留存活防误杀未保存工作
fn terminate_all_children(children: &Mutex<Vec<TrackedChild>>) {
    let mut pending = std::mem::take(&mut *children.lock());
    for tracked in pending.iter_mut() {
        match tracked.exit_policy {
            ChildExitPolicy::KillOnExit => {
                info!(
                    "[进程治理] 正在终止被纳管的外部子进程 [{}]...",
                    tracked.name
                );
                let _ = tracked.child.kill();
                let _ = tracked.child.wait();
                tracked.mark_finished();
            }
            ChildExitPolicy::DetachOnExit => {
                if let Ok(Some(_)) = tracked.child.try_wait() {
                    tracked.mark_finished();
                } else {
                    info!(
                        "[进程治理] 守护进程即将退场，已将外部交互进程 [{}] 转交桌面会话继续存活",
                        tracked.name
                    );
                }
            }
        }
    }
}

/// 启动后台收割线程，持续轮询回收已退出的外部子进程并在停机时强制终止存活进程
fn spawn_reaper_thread(
    children: Arc<Mutex<Vec<TrackedChild>>>,
    shutdown: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    match thread::Builder::new()
        .name("waylan-subprocess-reaper".to_string())
        .spawn(move || {
            while !shutdown.load(Ordering::Acquire) && !exit_requested.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(200));
                reap_exited_children(&children);
            }

            // 收到退出信号后，主动清理所有存活子进程并收割状态
            terminate_all_children(&children);
        }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            log::warn!(
                "[进程治理 警告] 创建子进程收割线程失败，外部子进程将无法自动回收: {}",
                e
            );
            None
        }
    }
}

/// 外部子进程生命周期管理器
pub struct SubprocessManager {
    children: Arc<Mutex<Vec<TrackedChild>>>,
    shutdown: Arc<AtomicBool>,
    reaper_handle: Option<JoinHandle<()>>,
}

impl SubprocessManager {
    /// 创建外部子进程管理器并启动后台收割线程
    pub fn new(exit_requested: Arc<AtomicBool>) -> Self {
        let children = Arc::new(Mutex::new(Vec::<TrackedChild>::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let reaper_handle =
            spawn_reaper_thread(Arc::clone(&children), Arc::clone(&shutdown), exit_requested);

        Self {
            children,
            shutdown,
            reaper_handle,
        }
    }

    /// 注册纳管新启动的外部子进程（默认按 `KillOnExit` 退场回收）
    pub fn track(&self, child: Child, running_guard: Option<Arc<AtomicBool>>, name: &'static str) {
        self.track_with_policy(child, running_guard, name, ChildExitPolicy::KillOnExit);
    }

    /// 按显式退场策略注册纳管外部子进程
    pub fn track_with_policy(
        &self,
        child: Child,
        running_guard: Option<Arc<AtomicBool>>,
        name: &'static str,
        exit_policy: ChildExitPolicy,
    ) {
        let mut lock = self.children.lock();
        lock.push(TrackedChild {
            child,
            running_guard,
            name,
            exit_policy,
        });
    }

    /// 唤起偏好设置图形界面进程
    pub fn launch_settings(&self, settings_running: &Arc<AtomicBool>) {
        if settings_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            info!("[进程治理] 偏好设置窗口已在运行中，忽略重复唤起请求。");
            return;
        }
        info!("[进程治理] 唤起微澜偏好设置界面...");
        let mut cmd = build_settings_command();

        match cmd.spawn() {
            Ok(child) => {
                self.track_with_policy(
                    child,
                    Some(Arc::clone(settings_running)),
                    "偏好设置界面",
                    ChildExitPolicy::DetachOnExit,
                );
            }
            Err(e) => {
                log::warn!(
                    "[进程治理] 无法启动图形偏好设置二进制 waylan-settings ({})，回退使用文本编辑器打开配置文件。",
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
            spawn_file_viewer(&path, self, "配置文件文本编辑器");
        }
    }

    /// 打开系统运行日志文件
    pub(crate) fn launch_view_log(&self) {
        let log_path = log_file_path();
        info!("[系统托盘] 查看运行日志: {:?}", log_path);
        spawn_file_viewer(&log_path, self, "运行日志查看器");
    }
}

/// 向待派生的外部进程命令注入 Linux 桌面关键会话环境变量
fn inject_desktop_env(cmd: &mut Command) {
    for var in [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_CURRENT_DESKTOP",
        "DESKTOP_SESSION",
    ] {
        if let Ok(val) = env::var(var) {
            cmd.env(var, &val);
        }
    }
}

/// 尝试派生注入了桌面环境与独立进程组的文件查看器子进程
fn try_spawn_viewer(mut cmd: Command, tracker: &SubprocessManager, desc: &'static str) -> bool {
    inject_desktop_env(&mut cmd);
    #[cfg(unix)]
    cmd.process_group(0);
    if let Ok(child) = cmd.spawn() {
        tracker.track_with_policy(child, None, desc, ChildExitPolicy::DetachOnExit);
        true
    } else {
        false
    }
}

/// 注入桌面环境关键变量并多级回退唤起外部文本编辑器或查看器
pub(crate) fn spawn_file_viewer(path: &Path, tracker: &SubprocessManager, desc: &'static str) {
    let editors = [
        "gnome-text-editor",
        "gedit",
        "kate",
        "mousepad",
        "xed",
        "featherpad",
    ];
    for editor in editors {
        let mut cmd = Command::new(editor);
        cmd.arg(path);
        if try_spawn_viewer(cmd, tracker, desc) {
            return;
        }
    }

    let mut gio_cmd = Command::new("gio");
    gio_cmd.arg("open").arg(path);
    if try_spawn_viewer(gio_cmd, tracker, desc) {
        return;
    }

    let xdg_bin = ["/usr/bin/xdg-open", "/bin/xdg-open"]
        .iter()
        .find(|p| Path::new(p).exists())
        .copied()
        .unwrap_or("xdg-open");
    let mut xdg_cmd = Command::new(xdg_bin);
    xdg_cmd.arg(path);
    let _ = try_spawn_viewer(xdg_cmd, tracker, desc);
}

impl Drop for SubprocessManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.reaper_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_subprocess_manager_lifecycle() {
        let exit_requested = Arc::new(AtomicBool::new(false));
        let manager = SubprocessManager::new(Arc::clone(&exit_requested));
        let running = Arc::new(AtomicBool::new(true));

        let child = Command::new("true").spawn().expect("执行 true 失败");
        manager.track(child, Some(Arc::clone(&running)), "测试短进程");

        // 等待轮询周期收割子进程
        thread::sleep(Duration::from_millis(500));
        assert!(
            !running.load(Ordering::Acquire),
            "子进程退出后 running_guard 应被自动收割置为 false"
        );
    }
}
