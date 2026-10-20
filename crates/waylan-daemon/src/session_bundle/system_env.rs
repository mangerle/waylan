//! 系统环境能力探测与看门狗自重启管理

use std::env;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use log::{info, warn};
use waylan_sys::LoopWakeup;
use waylan_wayland::{probe_wayland_environment, WaylandCapabilities};

/// 检查 Wayland 合成器协议能力
pub fn check_wayland_capabilities() -> WaylandCapabilities {
    info!("[系统环境] 正在检查当前 Wayland 会话与合成器能力...");
    match probe_wayland_environment() {
        Ok(caps) => {
            info!(
                "[系统环境] 成功连接到 Wayland 会话：wl_compositor={}, wl_seat={}, zwp_input_method_v1={}, zwp_input_method_manager_v2={}, zwlr_layer_shell_v1={}",
                caps.has_compositor,
                caps.has_seat,
                caps.has_input_method_v1,
                caps.has_input_method_v2,
                caps.has_layer_shell_v1
            );
            caps
        }
        Err(e) => {
            warn!(
                "[系统环境 警告] 无法直接连接默认 Wayland 会话 ({})，请确保 WAYLAND_DISPLAY 环境变量已设置。",
                e
            );
            WaylandCapabilities::default()
        }
    }
}

/// 派生延迟等待当前进程退出的新实例并向旧实例发出优雅停机信号
pub fn trigger_graceful_restart_and_exit(
    exit_requested: &AtomicBool,
    loop_wakeup: Option<&LoopWakeup>,
) {
    let exe = match env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::error!(
                "[系统环境 错误] 无法获取当前程序可执行路径，取消自重启: {}",
                e
            );
            return;
        }
    };

    let pid = process::id();
    let args = env::args().skip(1).collect::<Vec<_>>();

    // 采用固定脚本并通过位置参数 ($1, $2, "$@") 传参，由内核保证参数原子性，100% 杜绝 Shell 注入
    let shell_script = "trap '' HUP; pid=\"$1\"; exe=\"$2\"; shift 2; \
        for _ in $(seq 1 50); do if ! kill -0 \"$pid\" 2>/dev/null; then break; fi; sleep 0.1; done; \
        if kill -0 \"$pid\" 2>/dev/null; then exit 1; fi; \
        sleep 0.15; exec \"$exe\" \"$@\"";

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(shell_script);
    cmd.arg("waylan-restart-helper");
    cmd.arg(pid.to_string());
    cmd.arg(&exe);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);

    match cmd.spawn() {
        Ok(_) => {
            log::info!("[系统环境] 已成功派生自重启看门狗子进程，正在向旧实例发送优雅停机信号...");
            exit_requested.store(true, Ordering::Release);
            if let Some(wakeup) = loop_wakeup {
                wakeup.wake();
            }
        }
        Err(e) => {
            log::error!(
                "[系统环境 错误] 派生自重启看门狗子进程失败: {}，取消退出以保持当前服务存活",
                e
            );
        }
    }
}
