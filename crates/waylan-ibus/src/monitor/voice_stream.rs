//! IBus 语音流式识别波形刷新与自动断句上屏
//!
//! # 设计原理
//! - **实现初衷**：将语音引擎实时吐出的动态音量波形与临时转写文本，映射为 IBus LookupTable 与 PreeditText。
//! - **核心优势**：非侵入式 D-Bus 信号广播，利用 IBus 标准接口展现语音胶囊悬浮窗与流式预编辑。
//! - **代价与局限**：高频流式推送依赖每 60ms 轮询窗口发射 D-Bus 信号。

use std::sync::Arc;

use log::info;
use parking_lot::Mutex;
use waylan_core::CandidateIndexStyle;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::super::engine::{UiSnapshot, WAYLAN_ENGINE_OBJECT_PATH};
use super::super::keys::IbusSessionCore;
use super::super::types::{
    build_ibus_lookup_table, build_ibus_preedit_text, build_ibus_text, IbusLookupTableParams,
    IBUS_ENGINE_PREEDIT_CLEAR, IBUS_ORIENTATION_HORIZONTAL,
};

/// 获取当前语音活跃状态与目标 IBus 引擎对象路径
pub(crate) fn query_voice_active_and_path(
    shared_core: &Arc<Mutex<IbusSessionCore>>,
) -> (bool, String) {
    let guard = shared_core.lock();
    let active = guard.is_voice_active();
    let path = guard
        .active_engine_path
        .clone()
        .unwrap_or_else(|| WAYLAN_ENGINE_OBJECT_PATH.to_string());
    (active, path)
}

/// 发送语音波形候选框与实时预编辑文本更新
fn emit_voice_stream_update(conn: &Connection, target_engine_path: &str, voice_snap: &UiSnapshot) {
    let table_visible = !voice_snap.candidates.is_empty();
    if let Ok(table_val) = build_ibus_lookup_table(&IbusLookupTableParams {
        candidates: &voice_snap.candidates,
        page_size: voice_snap.page_size,
        selected_index: voice_snap.selected_index,
        orientation: voice_snap.orientation,
        index_style: voice_snap.index_style,
    }) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(table_val), table_visible),
        );
    }

    if let Ok(preedit_val) = build_ibus_preedit_text(&voice_snap.preedit) {
        let char_len = voice_snap.preedit.chars().count() as u32;
        let visible = !voice_snap.preedit.is_empty();
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(preedit_val),
                char_len,
                visible,
                IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 发射清空候选框与预编辑的信号
fn emit_voice_stream_exit(conn: &Connection, target_engine_path: &str) {
    if let Ok(empty_table) = build_ibus_lookup_table(&IbusLookupTableParams {
        candidates: &[],
        page_size: 1,
        selected_index: 0,
        orientation: IBUS_ORIENTATION_HORIZONTAL,
        index_style: CandidateIndexStyle::None,
    }) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(empty_table), false),
        );
    }
    if let Ok(empty_preedit) = build_ibus_preedit_text("") {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(empty_preedit),
                0u32,
                false,
                IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 自动断句或最终结算文字上屏提交（保留语音状态浮条以衔接「识别完成」淡出）
fn commit_voice_text(conn: &Connection, target_engine_path: &str, commit_text: &str) {
    if commit_text.is_empty() {
        return;
    }
    info!(
        "[微澜 IBus 语音] 自动断句文字上屏 (目标路径: {}): \"{}\"",
        target_engine_path, commit_text
    );
    if let Ok(empty_preedit) = build_ibus_preedit_text("") {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(empty_preedit),
                0u32,
                false,
                IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
    if let Ok(text_val) = build_ibus_text(commit_text) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "CommitText",
            &(Value::from(text_val),),
        );
    }
}

/// 语音流式轮询单周期处理参数包
///
/// # 设计原理
/// - **实现初衷**：单轮语音轮询需同时依赖总线连接、共享会话核心、目标引擎路径与跨轮次延续的语音激活标记，
///   平铺参数会让调用点难以辨识可变槽位，故收敛为专用的参数结构体。
/// - **核心优势**：可变状态以 `&mut` 槽位形式显式承载，跨轮次状态的归属一目了然，杜绝位置错配。
/// - **代价与局限**：调用点需显式构造结构体字面量，代码量略增但语义更清晰。
pub(crate) struct VoiceStreamCycleParams<'a> {
    /// 到 `ibus-daemon` 的阻塞连接
    pub(crate) conn: &'a Connection,
    /// 共享的输入法会话核心
    pub(crate) shared_core: &'a Arc<Mutex<IbusSessionCore>>,
    /// 目标 IBus 引擎对象路径
    pub(crate) target_engine_path: &'a str,
    /// 本轮观测到的语音激活状态
    pub(crate) is_voice_active: bool,
    /// 上一轮观测到的语音激活状态（跨轮次延续的可变槽位）
    pub(crate) last_voice_active: &'a mut bool,
}

/// 处理单轮语音事件轮询、流式更新与过渡退出
pub(crate) fn handle_voice_stream_cycle(params: &mut VoiceStreamCycleParams<'_>) {
    if params.is_voice_active {
        let (voice_snap, voice_commit) = {
            let mut guard = params.shared_core.lock();
            let commit = guard.poll_voice_events();
            let snap = UiSnapshot::capture(&guard);
            (snap, commit)
        };
        if let Some(ref commit_text) = voice_commit {
            commit_voice_text(params.conn, params.target_engine_path, commit_text);
        }
        emit_voice_stream_update(params.conn, params.target_engine_path, &voice_snap);
    } else if *params.last_voice_active {
        emit_voice_stream_exit(params.conn, params.target_engine_path);
    }
    *params.last_voice_active = params.is_voice_active;
}
