//! IBus 语音流式识别波形刷新与自动断句上屏
//!
//! # 设计原理
//! - **实现初衷**：将语音引擎实时吐出的动态音量波形与临时转写文本，通过统一 [`ImeRuntime::poll_voice_cycle`]
//!   驱动并借助 `emitter` 模块的共享 D-Bus 信号发射函数呈现给 `ibus-daemon`。
//! - **核心优势**：彻底消除与 `emitter.rs` 的重复信号载荷构造逻辑，并与 Wayland 端共用同一语音流状态机。
//! - **代价与局限**：高频流式推送依赖每 60ms 轮询窗口发射 D-Bus 信号。

use std::sync::Arc;

use log::info;
use parking_lot::Mutex;
use zbus::connection::Connection;

use super::super::engine::emitter::{emit_commit_text_blocking, emit_ui_snapshot_blocking};
use super::super::engine::{UiSnapshot, WAYLAN_ENGINE_OBJECT_PATH};
use super::super::keys::IbusSessionCore;

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
        let mut outcome = {
            let mut guard = params.shared_core.lock();
            guard.poll_voice_cycle()
        };
        if let Some(ref commit_text) = outcome.commit_text {
            info!(
                "[微澜 IBus 语音] 自动断句文字上屏 (目标路径: {}): \"{}\"",
                params.target_engine_path, commit_text
            );
            emit_commit_text_blocking(params.conn, params.target_engine_path, commit_text);
        }
        if let Some(ref snap) = outcome.ui_snapshot.take() {
            emit_ui_snapshot_blocking(params.conn, params.target_engine_path, snap);
        }
    } else if *params.last_voice_active {
        emit_ui_snapshot_blocking(params.conn, params.target_engine_path, &UiSnapshot::empty());
    }
    *params.last_voice_active = params.is_voice_active;
}
