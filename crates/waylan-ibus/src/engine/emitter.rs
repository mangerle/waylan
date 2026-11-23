//! IBus UI 信号发射与结果分发器
//!
//! # 设计原理
//! - **实现初衷**：将预编辑文本更新、候选词表刷新、按键上屏与标点光标补偿等 D-Bus 异步信号发送逻辑从接口定义中解耦。
//! - **核心优势**：在释放核心 Mutex 锁后进行纯异步信号发射，杜绝死锁与事件循环阻塞。

use zbus::Variant;
use zbus::connection::Connection;
use zbus::object_server::SignalEmitter;

use waylan_core::input::{KEY_LEFT, KEY_RIGHT};

use super::super::keys::{IBUS_KEYVAL_LEFT, IBUS_KEYVAL_RIGHT, IBUS_RELEASE_MASK, IbusKeyOutcome};
use super::super::types::{
    IBUS_ENGINE_PREEDIT_CLEAR, IbusLookupTableParams, build_ibus_lookup_table,
    build_ibus_preedit_text, build_ibus_text, build_input_mode_property,
};
use super::service::IbusEngineService;
use super::snapshot::UiSnapshot;

/// 按键结果派发参数
///
/// # 设计原理
/// - **实现初衷**：结果派发需同时携带"信号上下文、按键处理结果、界面快照与大写锁定状态"，
///   平铺传参易造成快照与锁定状态顺序颠倒，导致输入模式属性与候选框状态不同步。
/// - **核心优势**：以参数对象聚合派发上下文，调用点按名传参即可完成语义绑定。
/// - **代价与局限**：多引入一个结构体类型。
pub(crate) struct OutcomeDispatchParams<'a, 'b> {
    /// D-Bus 信号发射上下文
    pub ctxt: &'a SignalEmitter<'b>,
    /// 按键处理结果
    pub outcome: IbusKeyOutcome,
    /// 界面状态快照（按键未触发界面刷新时为 None）
    pub snap: Option<UiSnapshot>,
    /// 当前大写锁定状态
    pub is_caps_locked: bool,
}

impl IbusEngineService {
    /// 在锁作用域之外根据快照向 `ibus-daemon` 发射预编辑与候选词表更新信号
    pub(crate) async fn emit_ui_snapshot(ctxt: &SignalEmitter<'_>, snap: &UiSnapshot) {
        if snap.preedit.is_empty() {
            let _ = Self::hide_preedit_text(ctxt).await;
        } else if let Ok(preedit_val) = build_ibus_preedit_text(&snap.preedit) {
            let char_len = snap.preedit.chars().count() as u32;
            let _ = Self::update_preedit_text(
                ctxt,
                Variant::from(preedit_val),
                char_len,
                true,
                IBUS_ENGINE_PREEDIT_CLEAR,
            )
            .await;
        }

        if snap.candidates.is_empty() {
            let _ = Self::hide_lookup_table(ctxt).await;
        } else if let Ok(table_val) = build_ibus_lookup_table(&IbusLookupTableParams {
            candidates: &snap.candidates,
            page_size: snap.page_size,
            selected_index: snap.selected_index,
            orientation: snap.orientation,
            index_style: snap.index_style,
        }) {
            let _ = Self::update_lookup_table(ctxt, Variant::from(table_val), true).await;
        }
    }

    /// 在锁作用域之外统一派发 `IbusKeyOutcome` 包含的所有 D-Bus 信号
    pub(crate) async fn dispatch_outcome(&self, params: OutcomeDispatchParams<'_, '_>) {
        let OutcomeDispatchParams {
            ctxt,
            outcome,
            snap,
            is_caps_locked,
        } = params;
        if let Some(ref text) = outcome.commit_text
            && let Ok(text_val) = build_ibus_text(text)
        {
            let _ = Self::commit_text(ctxt, Variant::from(text_val)).await;
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

        if !self.suppress_properties
            && let Some(new_mode) = outcome.mode_changed
            && let Ok(prop_val) = build_input_mode_property(new_mode, is_caps_locked)
        {
            let _ = Self::update_property(ctxt, Variant::from(prop_val)).await;
        }
    }

    /// 重置会话并向 `ibus-daemon` 发送隐藏预编辑与候选面板信号
    pub(crate) async fn reset_and_hide_ui(&self, ctxt: &SignalEmitter<'_>) {
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

/// IBus 引擎 D-Bus 接口名
const IBUS_ENGINE_INTERFACE: &str = "org.freedesktop.IBus.Engine";

/// 在锁作用域之外，通过阻塞式 D-Bus 连接向指定引擎路径发射文本上屏信号
pub(crate) fn emit_commit_text_blocking(conn: &Connection, target_engine_path: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(text_val) = build_ibus_text(text) {
        let _ = zbus::block_on(async {
            conn.emit_signal(
                None::<&str>,
                target_engine_path,
                IBUS_ENGINE_INTERFACE,
                "CommitText",
                &(Variant::from(text_val),),
            )
            .await
        });
    }
}

/// 在锁作用域之外，通过阻塞式 D-Bus 连接发射预编辑与候选词表快照信号
///
/// 与 [`IbusEngineService::emit_ui_snapshot`] 共享同一套 `UiSnapshot` 语义与载荷构造，
/// 供后台监控线程（如语音流式刷新）复用。
pub(crate) fn emit_ui_snapshot_blocking(
    conn: &Connection,
    target_engine_path: &str,
    snap: &UiSnapshot,
) {
    if let Ok(preedit_val) = build_ibus_preedit_text(&snap.preedit) {
        let char_len = snap.preedit.chars().count() as u32;
        let visible = !snap.preedit.is_empty();
        let _ = zbus::block_on(async {
            conn.emit_signal(
                None::<&str>,
                target_engine_path,
                IBUS_ENGINE_INTERFACE,
                "UpdatePreeditText",
                &(
                    Variant::from(preedit_val),
                    char_len,
                    visible,
                    IBUS_ENGINE_PREEDIT_CLEAR,
                ),
            )
            .await
        });
    }

    let table_visible = !snap.candidates.is_empty();
    if let Ok(table_val) = build_ibus_lookup_table(&IbusLookupTableParams {
        candidates: &snap.candidates,
        page_size: snap.page_size,
        selected_index: snap.selected_index,
        orientation: snap.orientation,
        index_style: snap.index_style,
    }) {
        let _ = zbus::block_on(async {
            conn.emit_signal(
                None::<&str>,
                target_engine_path,
                IBUS_ENGINE_INTERFACE,
                "UpdateLookupTable",
                &(Variant::from(table_val), table_visible),
            )
            .await
        });
    }
}
