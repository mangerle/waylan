//! IBus / GNOME 原生顶栏状态栏属性项响应与设置切换
//!
//! # 设计原理
//! - **实现初衷**：响应 GNOME / IBus 原生顶栏菜单点击事件（中英切换、简繁切换、全半角标点/字符切换、重载配置）。
//! - **核心优势**：与 Engine 核心按键分发解耦，状态变更后立即发射 UpdateProperty 信号通知桌面顶栏刷新。
//! - **代价与局限**：受 suppress_properties 配置控制，在独立托盘形态下静默属性上报。

use log::info;
use std::sync::atomic::Ordering;
use waylan_core::InputMode;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use super::super::types::{
    build_full_width_char_property, build_full_width_punct_property, build_input_mode_property,
    build_traditional_property,
};
use super::service::IbusEngineService;

impl IbusEngineService {
    /// 响应状态栏点击切换中英文输入模式
    pub(crate) async fn on_prop_input_mode(&self, ctxt: &SignalContext<'_>) {
        let (new_mode, is_caps) = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            let next = match guard.core.mode {
                InputMode::Chinese | InputMode::VMode => InputMode::English,
                InputMode::English => InputMode::Chinese,
            };
            guard.core.mode = next;
            if let Some(ref flags) = guard.shared_flags {
                flags.set_chinese(matches!(next, InputMode::Chinese));
            }
            if let Some(ref tx) = guard.mode_notify_tx {
                let _ = tx.try_send(());
            }
            (next, guard.is_caps_locked)
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
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            let next = !guard.core.engine.config.traditional_chinese;
            guard.core.engine.config.traditional_chinese = next;
            if let Some(ref flags) = guard.shared_flags {
                flags.set_traditional(next);
            }
            if let Some(ref tx) = guard.mode_notify_tx {
                let _ = tx.try_send(());
            }
            info!(
                "[微澜 IBus] 原生状态栏切换简繁模式为: {}",
                if next { "繁体" } else { "简体" }
            );
            next
        };
        if let Ok(prop_val) = build_traditional_property(is_trad) {
            let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
        }
    }

    /// 响应状态栏点击切换标点全半角模式
    pub(crate) async fn on_prop_full_width_punct(&self, ctxt: &SignalContext<'_>) {
        let is_full = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            let next = !guard.core.punctuation_processor.full_width_punct;
            guard.core.punctuation_processor.full_width_punct = next;
            if let Some(ref flags) = guard.shared_flags {
                flags.set_full_width_punct(next);
            }
            if let Some(ref tx) = guard.mode_notify_tx {
                let _ = tx.try_send(());
            }
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
        if let Ok(prop_val) = build_full_width_punct_property(is_full) {
            let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
        }
    }

    /// 响应状态栏点击切换字符全半角模式
    pub(crate) async fn on_prop_full_width_char(&self, ctxt: &SignalContext<'_>) {
        let is_full = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            let current = guard
                .shared_flags
                .as_ref()
                .map(|f| f.is_full_width_char())
                .unwrap_or(guard.core.full_width_char);
            let next = !current;
            guard.core.full_width_char = next;
            if let Some(ref flags) = guard.shared_flags {
                flags.set_full_width_char(next);
            }
            if let Some(ref tx) = guard.mode_notify_tx {
                let _ = tx.try_send(());
            }
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
        if let Ok(prop_val) = build_full_width_char_property(is_full) {
            let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
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
