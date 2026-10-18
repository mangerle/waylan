//! V2 协议键盘输入分发与状态机管理模块

mod repeat;

use super::WaylanV2Session;

pub(crate) use repeat::{drive_key_repeat, next_poll_timeout_ms};

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub(crate) fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    // 1. 全权委托统一运行时 ImeRuntime 处理（包含语音分发、核心决策与前端 UI 刷新）
    let is_password = session.is_password_context;
    let surrounding = session.surrounding_text.clone();
    let (runtime, mut frontend) = session.split_runtime_and_frontend();

    let outcome = runtime.process_key_event(
        &mut frontend,
        key,
        is_press,
        is_password,
        surrounding.as_ref(),
        true, // inline_preedit
    );

    // 3. 按键长按连击跟踪处理
    if !is_press {
        if session
            .repeat_state
            .is_some_and(|(rep_key, _)| rep_key == key)
        {
            session.repeat_state = None;
        }
    } else {
        repeat::update_key_repeat(session, key, outcome.0);
    }

    outcome
}
