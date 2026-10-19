//! IBus UI 信号发射与结果分发器
//!
//! # 设计原理
//! - **实现初衷**：将预编辑文本更新、候选词表刷新、按键上屏与标点光标补偿等 D-Bus 异步信号发送逻辑从接口定义中解耦。
//! - **核心优势**：在释放核心 Mutex 锁后进行纯异步信号发射，杜绝死锁与事件循环阻塞。

use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use waylan_core::input::{KEY_LEFT, KEY_RIGHT};

use super::super::keys::{IbusKeyOutcome, IBUS_KEYVAL_LEFT, IBUS_KEYVAL_RIGHT, IBUS_RELEASE_MASK};
use super::super::types::{
    build_ibus_lookup_table, build_ibus_preedit_text, build_ibus_text, build_input_mode_property,
    IBUS_ENGINE_PREEDIT_CLEAR,
};
use super::service::IbusEngineService;
use super::snapshot::UiSnapshot;

impl IbusEngineService {
    /// 在锁作用域之外根据快照向 `ibus-daemon` 发射预编辑与候选词表更新信号
    pub(crate) async fn emit_ui_snapshot(ctxt: &SignalContext<'_>, snap: &UiSnapshot) {
        if snap.preedit.is_empty() {
            let _ = Self::hide_preedit_text(ctxt).await;
        } else if let Ok(preedit_val) = build_ibus_preedit_text(&snap.preedit) {
            let char_len = snap.preedit.chars().count() as u32;
            let _ = Self::update_preedit_text(
                ctxt,
                Value::from(preedit_val),
                char_len,
                true,
                IBUS_ENGINE_PREEDIT_CLEAR,
            )
            .await;
        }

        if snap.candidates.is_empty() {
            let _ = Self::hide_lookup_table(ctxt).await;
        } else if let Ok(table_val) = build_ibus_lookup_table(
            &snap.candidates,
            snap.page_size,
            snap.selected_index,
            snap.orientation,
            snap.index_style,
        ) {
            let _ = Self::update_lookup_table(ctxt, Value::from(table_val), true).await;
        }
    }

    /// 在锁作用域之外统一派发 `IbusKeyOutcome` 包含的所有 D-Bus 信号
    pub(crate) async fn dispatch_outcome(
        &self,
        ctxt: &SignalContext<'_>,
        outcome: IbusKeyOutcome,
        snap: Option<UiSnapshot>,
        is_caps_locked: bool,
    ) {
        if let Some(ref text) = outcome.commit_text {
            if let Ok(text_val) = build_ibus_text(text) {
                let _ = Self::commit_text(ctxt, Value::from(text_val)).await;
            }
        }

        if outcome.cursor_offset != 0 {
            let (keyval, keycode) = if outcome.cursor_offset < 0 {
                (IBUS_KEYVAL_LEFT, KEY_LEFT)
            } else {
                (IBUS_KEYVAL_RIGHT, KEY_RIGHT)
            };
            for _ in 0..outcome.cursor_offset.unsigned_abs() {
                let _ = Self::forward_key_event(ctxt, keyval, keycode, 0).await;
                let _ = Self::forward_key_event(ctxt, keyval, keycode, IBUS_RELEASE_MASK).await;
            }
        }

        if let Some(ref s) = snap {
            Self::emit_ui_snapshot(ctxt, s).await;
        }

        if !self.suppress_properties {
            if let Some(new_mode) = outcome.mode_changed {
                if let Ok(prop_val) = build_input_mode_property(new_mode, is_caps_locked) {
                    let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
                }
            }
        }
    }

    /// 重置会话并向 `ibus-daemon` 发送隐藏预编辑与候选面板信号
    pub(crate) async fn reset_and_hide_ui(&self, ctxt: &SignalContext<'_>) {
        self.clear_password_state();
        let snap = {
            let mut guard = self.core.lock();
            guard.reset_context();
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
        let _ = Self::hide_preedit_text(ctxt).await;
        let _ = Self::hide_lookup_table(ctxt).await;
    }
}
