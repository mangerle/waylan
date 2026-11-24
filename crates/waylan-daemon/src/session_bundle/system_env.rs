//! 系统环境能力探测与看门狗自重启转发

use log::{info, warn};
use waylan_wayland::{WaylandCapabilities, probe_wayland_environment};

pub use crate::system::trigger_graceful_restart_and_exit;

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
