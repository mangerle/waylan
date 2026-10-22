//! IBus 输入上下文窗口感知与焦点生命周期模块
//!
//! # 设计原理
//! - **实现初衷**：在 Wayland 合成器（如 GNOME Mutter）下，普通客户端可能被 XWayland 或 IBus 代理，
//!   单纯依赖 IBus 内部传递的 `object_path` 或 `client` 可能无法精确分辨每个独立窗口及其实际进程（如各类终端模拟器）。
//!   本模块采用“双轨协同”架构：第一轨通过 IBus 协议层的 `FocusInId` 获取独立上下文对象路径；
//!   第二轨从 GNOME Shell 扩展在内存文件系统（tmpfs）中发布的元数据快速提取合成器原生顶层焦点窗口标识与应用类别。
//! - **核心优势**：纳秒级纯内存读取（tmpfs），非阻塞且零额外 D-Bus 调用开销；完美识别各类终端应用并实现独立的输入状态记忆。
//! - **代价与局限**：GNOME 扩展轨仅在 GNOME Shell 桌面环境且扩展已启用时生效，非 GNOME 环境平滑降级至 IBus 协议轨。

use log::debug;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use super::super::types::{build_ibus_prop_list, IbusPropListParams};
use super::service::IbusEngineService;
use super::snapshot::UiSnapshot;

/// GNOME Shell 扩展导出的活跃焦点窗口元数据
#[derive(Debug, Deserialize)]
struct GnomeActiveWindowInfo {
    #[serde(default)]
    win_id: String,
    #[serde(default)]
    wm_class: String,
}

/// 从 JSON 字符串解析顶层焦点窗口标识与应用类别
fn parse_active_window_from_str(content: &str) -> Option<(String, String)> {
    let info: GnomeActiveWindowInfo = serde_json::from_str(content).ok()?;
    if info.win_id.is_empty() && info.wm_class.is_empty() {
        None
    } else {
        Some((info.win_id, info.wm_class))
    }
}

/// 从内存 tmpfs 读取 GNOME Shell 扩展实时同步的当前顶层焦点窗口信息
///
/// 返回 `(win_id, wm_class)`，若文件不存在或读取解析失败则返回 `None`。
pub(crate) fn read_gnome_active_window() -> Option<(String, String)> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let uid = rustix::process::getuid().as_raw();
        format!("/run/user/{}", uid)
    });
    let file_path = PathBuf::from(runtime_dir).join("waylan_active_window.json");
    let content = std::fs::read_to_string(file_path).ok()?;
    parse_active_window_from_str(&content)
}

impl IbusEngineService {
    /// 统一焦点进入处理实现
    pub(crate) async fn handle_focus_in_internal(
        &self,
        ctxt: &SignalContext<'_>,
        object_path: Option<&str>,
        client: Option<&str>,
    ) {
        let (window_id, app_name) = if let Some((win_id, wm_class)) = read_gnome_active_window() {
            let final_id = if !win_id.is_empty() {
                format!("gnome_win_{}", win_id)
            } else if let Some(p) = object_path.filter(|s| !s.is_empty()) {
                p.to_string()
            } else {
                ctxt.path().to_string()
            };
            (final_id, Some(wm_class))
        } else {
            let final_id = object_path
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ctxt.path().as_str())
                .to_string();
            let app = client
                .filter(|s| !s.is_empty() && *s != "fake" && *s != "xim")
                .map(|s| s.to_string());
            (final_id, app)
        };

        debug!(
            "[微澜 IBus] 输入上下文获得焦点: 窗口标识={}, 应用类别={:?}",
            window_id, app_name
        );

        let (mode, is_caps, trad, punct, char_full, snap) = {
            let mut guard = self.core.lock();
            guard.active_engine_path = Some(window_id.clone());
            // 切换窗口上下文独立记忆（自动感知终端类应用并按需初始化英文）
            guard.runtime.focus_window(&window_id, app_name.as_deref());
            // 焦点进入新输入上下文时同步当前 Engine 实例记录的密码框状态，
            // 既杜绝旧普通窗口残留密码标记，又防止因先触发 SetContentType 后触发 FocusIn 导致密码标记被误冲刷
            guard.is_password_context = self.is_password.load(Ordering::Acquire);
            // 自愈保护：若当前获焦的并非密码框，而内部仍记录为锁屏状态，自动解除锁屏直通
            self.lock_healer.heal_on_focus(
                guard.is_password_context,
                &mut guard.is_screen_locked,
                self.loop_wakeup.as_ref(),
            );
            // 获焦状态校验：若系统当前实际处于锁屏状态，严格锁定为锁屏直通模式
            if self.lock_healer.is_screen_locked() {
                guard.is_screen_locked = true;
            }
            let is_protected = guard.is_password_context || guard.is_screen_locked;
            guard.runtime.set_protected_context(is_protected);
            let _ = guard.sync_shared_mode();
            let trad = guard.runtime.core.engine.config.traditional_chinese;
            let punct = guard.runtime.core.is_full_width_punct();
            let char_full = guard.is_full_width_char();
            (
                guard.runtime.effective_input_mode(),
                guard.is_caps_locked(),
                trad,
                punct,
                char_full,
                UiSnapshot::capture(&guard),
            )
        };
        if !self.suppress_properties {
            if let Ok(prop_list) = build_ibus_prop_list(&IbusPropListParams {
                mode,
                is_caps_locked: is_caps,
                traditional_chinese: trad,
                full_width_punct: punct,
                full_width_char: char_full,
            }) {
                let _ = Self::register_properties(ctxt, Value::from(prop_list)).await;
            }
        }
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_active_window_success() {
        let json = r#"{"win_id":"42","wm_class":"org.gnome.Terminal","title":"终端"}"#;
        let res = parse_active_window_from_str(json);
        assert_eq!(
            res,
            Some(("42".to_string(), "org.gnome.Terminal".to_string()))
        );
    }

    #[test]
    fn test_parse_active_window_empty_fields() {
        let json = r#"{"win_id":"","wm_class":"","title":""}"#;
        assert_eq!(parse_active_window_from_str(json), None);
    }

    #[test]
    fn test_parse_active_window_invalid_json() {
        let json = r#"not a valid json"#;
        assert_eq!(parse_active_window_from_str(json), None);
    }
}
