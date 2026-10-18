//! V2 协议会话按键长按连击 (Key Repeat) 驱动与等待超时控制

use std::time::{Duration, Instant};

use super::super::WaylanV2Session;
use super::process_key_event;

/// 根据本次按键消费状态与连击设置更新连击计时器
pub(super) fn update_key_repeat(session: &mut WaylanV2Session, key: u32, consumed: bool) {
    let (rate, delay) = session.keyboard.repeat_info;
    if consumed && rate > 0 && delay > 0 && !session.runtime.core.engine.raw_input().is_empty() {
        let next_fire = Instant::now() + Duration::from_millis(delay.max(50) as u64);
        session.keyboard.repeat_state = Some((key, next_fire));
    } else {
        session.keyboard.repeat_state = None;
    }
}

/// 计算事件循环下一次 poll 的等待超时毫秒数（若存在活跃按键连击，按剩余到期时间缩短超时）
pub(crate) fn next_poll_timeout_ms(session: &WaylanV2Session, default_ms: i32) -> i32 {
    let Some((_, next_fire)) = session.keyboard.repeat_state else {
        return default_ms;
    };
    let now = Instant::now();
    if next_fire <= now {
        return 0;
    }
    let remain_ms = next_fire.duration_since(now).as_millis() as i32;
    if default_ms < 0 {
        remain_ms.max(1)
    } else {
        remain_ms.clamp(1, default_ms)
    }
}

/// 检查并触发已到期的长按按键连击事件
pub(crate) fn drive_key_repeat(session: &mut WaylanV2Session) {
    let Some((key, next_fire)) = session.keyboard.repeat_state else {
        return;
    };
    let (rate, _) = session.keyboard.repeat_info;
    if rate <= 0 || !session.focus.is_active || session.runtime.core.engine.raw_input().is_empty() {
        session.keyboard.repeat_state = None;
        return;
    };

    let now = Instant::now();
    if now >= next_fire {
        let (consumed, _) = process_key_event(session, key, true);
        if consumed && !session.runtime.core.engine.raw_input().is_empty() {
            let interval_ms = (1000 / rate.max(1)).max(10) as u64;
            session.keyboard.repeat_state = Some((key, now + Duration::from_millis(interval_ms)));
        } else {
            session.keyboard.repeat_state = None;
        }
    }
}
