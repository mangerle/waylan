//! IBus 会话总线地址发现与套接字可达性校验
//!
//! # 设计原理
//! - **实现初衷**：在非 `--ibus` 直接启动模式下，自动发现宿主环境中由 ibus-daemon 写入的会话总线地址文件。
//! - **核心优势**：支持环境变量与 XDG 目录双重探测，优先匹配当前 Wayland 显示环境，并在连接前进行套接字可达性预检。
//! - **代价与局限**：依赖文件系统读取 ~/.config/ibus/bus 目录下的总线配置文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use waylan_core::config::env_var_non_empty;

/// 探测当前用户会话中的活跃 IBus D-Bus 总线地址
pub fn discover_ibus_address() -> Option<String> {
    if let Some(addr) = env_var_non_empty("IBUS_ADDRESS")
        && is_ibus_socket_reachable(&addr)
    {
        return Some(addr);
    }

    let bus_dir = resolve_ibus_bus_dir()?;
    let entries = fs::read_dir(bus_dir).ok()?;
    let prefer_wayland = env_var_non_empty("WAYLAND_DISPLAY").is_some();

    let mut candidates: Vec<(bool, SystemTime, String)> = Vec::with_capacity(4);

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let is_wayland_file = prefer_wayland && file_name.contains("wayland");
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if let Some(addr) = parse_valid_bus_file(&path) {
            candidates.push((is_wayland_file, mtime, addr));
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    candidates.into_iter().next().map(|(_, _, addr)| addr)
}

/// 解析 `~/.config/ibus/bus` 目录路径
fn resolve_ibus_bus_dir() -> Option<PathBuf> {
    if let Some(cfg_home) = env_var_non_empty("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(cfg_home).join("ibus").join("bus");
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let home = env_var_non_empty("HOME")?;
    let dir = PathBuf::from(home).join(".config").join("ibus").join("bus");
    dir.is_dir().then_some(dir)
}

/// 解析单个 IBus 总线文件并校验守护进程存活与套接字可达性
fn parse_valid_bus_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let mut address = None;
    let mut pid: Option<u32> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some(addr) = trimmed.strip_prefix("IBUS_ADDRESS=") {
            address = Some(addr.trim().to_string());
        } else if let Some(p_str) = trimmed.strip_prefix("IBUS_DAEMON_PID=") {
            pid = p_str.trim().parse::<u32>().ok();
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(daemon_pid) = pid
        && !Path::new(&format!("/proc/{}", daemon_pid)).exists()
    {
        return None;
    }

    let addr = address?;
    is_ibus_socket_reachable(&addr).then_some(addr)
}

/// 检查 `unix:path=...` 形式的 D-Bus 地址对应的本地套接字文件是否存在
pub(crate) fn is_ibus_socket_reachable(address: &str) -> bool {
    for part in address.split(',') {
        if let Some(sock_path) = part.strip_prefix("unix:path=") {
            return Path::new(sock_path).exists();
        }
        if part.starts_with("unix:abstract=") || part.starts_with("tcp:") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ibus_socket_reachable() {
        assert!(is_ibus_socket_reachable(
            "unix:abstract=/tmp/dbus-test,guid=12345"
        ));
        assert!(!is_ibus_socket_reachable(
            "unix:path=/nonexistent/waylan_ibus_test_socket,guid=12345"
        ));
    }
}
