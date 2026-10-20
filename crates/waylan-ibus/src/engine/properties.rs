//! IBus / GNOME 原生顶栏状态栏属性项响应与设置切换
//!
//! # 设计原理
//! - **实现初衷**：响应 GNOME / IBus 原生顶栏菜单点击事件（中英切换、简繁切换、全半角标点/字符切换、重载配置）。
//! - **核心优势**：与 Engine 核心按键分发解耦，状态变更后立即发射 UpdateProperty 信号通知桌面顶栏刷新。
//! - **代价与局限**：受 suppress_properties 配置控制，在独立托盘形态下静默属性上报。

use log::info;
use std::sync::atomic::Ordering;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use super::super::types::{
    build_full_width_char_property, build_full_width_punct_property, build_input_mode_property,
    build_traditional_property, KEY_EXIT, KEY_FULL_WIDTH_CHAR, KEY_FULL_WIDTH_PUNCT,
    KEY_INPUT_MODE, KEY_RELOAD_CONFIG, KEY_RESTART, KEY_SETUP, KEY_TRADITIONAL, KEY_VIEW_LOG,
};
use super::service::IbusEngineService;

impl IbusEngineService {
    /// 统一响应 GNOME / IBus 原生状态栏菜单项激活事件
    pub(crate) async fn handle_property_activate(&self, ctxt: &SignalContext<'_>, name: &str) {
        match name {
            KEY_INPUT_MODE => self.on_prop_input_mode(ctxt).await,
            KEY_TRADITIONAL => self.on_prop_traditional_chinese(ctxt).await,
            KEY_FULL_WIDTH_PUNCT => self.on_prop_full_width_punct(ctxt).await,
            KEY_FULL_WIDTH_CHAR => self.on_prop_full_width_char(ctxt).await,
            KEY_RELOAD_CONFIG => self.on_prop_reload_config(),
            KEY_SETUP => {
                if let Some(ref handler) = self.menu_handler {
                    handler.open_settings(&self.settings_running);
                }
            }
            KEY_VIEW_LOG => {
                if let Some(ref handler) = self.menu_handler {
                    handler.view_log();
                }
            }
            KEY_RESTART => {
                if let Some(ref handler) = self.menu_handler {
                    info!("[微澜 IBus] 原生状态栏菜单触发：排队重启输入法守护进程。");
                    handler.restart();
                }
            }
            KEY_EXIT => {
                if let Some(ref handler) = self.menu_handler {
                    info!("[微澜 IBus] 原生状态栏菜单触发：请求优雅停机与数据落盘。");
                    handler.quit();
                }
            }
            _ => {}
        }
    }

    /// 响应状态栏点击切换中英文输入模式
    async fn on_prop_input_mode(&self, ctxt: &SignalContext<'_>) {
        let (new_mode, is_caps) = {
            let mut guard = self.core.lock();
            let next = guard.runtime.toggle_input_mode();
            (next, guard.is_caps_locked())
        };
        if !self.suppress_properties {
            if let Ok(prop_val) = build_input_mode_property(new_mode, is_caps) {
                let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
            }
        }
    }

    /// 响应状态栏点击切换简繁模式
    pub(crate) async fn on_prop_traditional_chinese(&self, ctxt: &SignalContext<'_>) {
        let is_trad = {
            let mut guard = self.core.lock();
            let next = guard.runtime.toggle_traditional();
            info!(
                "[微澜 IBus] 原生状态栏切换简繁模式为: {}",
                if next { "繁体" } else { "简体" }
            );
            next
        };
        if !self.suppress_properties {
            if let Ok(prop_val) = build_traditional_property(is_trad) {
                let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
            }
        }
    }

    /// 响应状态栏点击切换标点全半角模式
    pub(crate) async fn on_prop_full_width_punct(&self, ctxt: &SignalContext<'_>) {
        let is_full = {
            let mut guard = self.core.lock();
            let next = guard.runtime.toggle_full_width_punct();
            info!(
                "[微澜 IBus] 原生状态栏切换标点模式为: {}",
                if next {
                    "全角 (中文标点)"
                } else {
                    "半角 (英文标点)"
                }
            );
            next
        };
        if !self.suppress_properties {
            if let Ok(prop_val) = build_full_width_punct_property(is_full) {
                let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
            }
        }
    }

    /// 响应状态栏点击切换字符全半角模式
    pub(crate) async fn on_prop_full_width_char(&self, ctxt: &SignalContext<'_>) {
        let is_full = {
            let mut guard = self.core.lock();
            let next = guard.runtime.toggle_full_width_char();
            info!(
                "[微澜 IBus] 原生状态栏切换字符模式为: {}",
                if next {
                    "全角 (英文字母)"
                } else {
                    "半角 (英文字母)"
                }
            );
            next
        };
        if !self.suppress_properties {
            if let Ok(prop_val) = build_full_width_char_property(is_full) {
                let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
            }
        }
    }

    /// 响应状态栏点击重新加载词库与配置
    pub(crate) fn on_prop_reload_config(&self) {
        if let Some(ref flag) = self.reload_requested {
            flag.store(true, Ordering::Release);
        }
        if let Some(ref wakeup) = self.loop_wakeup {
            wakeup.wake();
        }
        info!("[微澜 IBus] 收到原生状态栏菜单重载指令，已触发词库与配置热重载。");
    }
}
