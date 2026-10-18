//! V2 协议键盘输入分发与状态机管理模块

mod repeat;

use super::{WaylanV2Frontend, WaylanV2Session};

pub(crate) use repeat::{drive_key_repeat, next_poll_timeout_ms};

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub(crate) fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    // 1. 全权委托统一运行时 ImeRuntime 处理（通过组合子前端上下文实现安全不相交借用）
    let is_password = session.focus.is_password_context;
    let surrounding = session.focus.surrounding_text.as_ref();
    let mut frontend = WaylanV2Frontend::new(
        &session.protocol,
        &mut session.surface,
        session.qhandle.as_ref(),
    );

    let outcome = session.runtime.process_key_event(
        &mut frontend,
        waylan_engine::ProcessKeyEventParams {
            key,
            is_press,
            is_password_or_locked: is_password,
            surrounding_text: surrounding,
            inline_preedit: true,
        },
    );

    // 2. 按键长按连击跟踪处理
    if !is_press {
        if session
            .keyboard
            .repeat_state
            .is_some_and(|(rep_key, _)| rep_key == key)
        {
            session.keyboard.repeat_state = None;
        }
    } else {
        repeat::update_key_repeat(session, key, outcome.0);
    }

    outcome
}
