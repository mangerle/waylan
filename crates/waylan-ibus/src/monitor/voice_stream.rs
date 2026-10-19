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
    build_ibus_lookup_table, build_ibus_preedit_text, build_ibus_text, IBUS_ENGINE_PREEDIT_CLEAR,
    IBUS_ORIENTATION_HORIZONTAL,
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
    if let Ok(table_val) = build_ibus_lookup_table(
        &voice_snap.candidates,
        voice_snap.page_size,
        voice_snap.selected_index,
        voice_snap.orientation,
        voice_snap.index_style,
    ) {
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
    if let Ok(empty_table) = build_ibus_lookup_table(
        &[],
        1,
        0,
        IBUS_ORIENTATION_HORIZONTAL,
        CandidateIndexStyle::None,
    ) {
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

/// 自动断句文字上屏提交
fn commit_voice_text(conn: &Connection, target_engine_path: &str, commit_text: &str) {
    if commit_text.is_empty() {
        return;
    }
    info!(
        "[微澜 IBus 语音] 自动断句文字上屏 (目标路径: {}): \"{}\"",
        target_engine_path, commit_text
    );
    emit_voice_stream_exit(conn, target_engine_path);
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

/// 处理单轮语音事件轮询、流式更新与过渡退出
pub(crate) fn handle_voice_stream_cycle(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    target_engine_path: &str,
    is_voice_active: bool,
    last_voice_active: &mut bool,
) {
    if is_voice_active {
        let (voice_snap, voice_commit) = {
            let mut guard = shared_core.lock();
            let commit = guard.poll_voice_events();
            let snap = UiSnapshot::capture(&guard);
            (snap, commit)
        };
        emit_voice_stream_update(conn, target_engine_path, &voice_snap);
        if let Some(ref commit_text) = voice_commit {
            commit_voice_text(conn, target_engine_path, commit_text);
        }
    } else if *last_voice_active {
        emit_voice_stream_exit(conn, target_engine_path);
    }
    *last_voice_active = is_voice_active;
}
