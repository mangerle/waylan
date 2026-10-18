//! V2 协议键盘输入分发与状态机管理模块

mod repeat;

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use log::debug;
use waylan_algo::{extract_surrounding_prefix, KeyProcessContext, KeyProcessResult};
use waylan_core::input::*;
use waylan_core::{CandidateBoxState, InputMode};
use waylan_voice::VoiceKeyAction;

use super::WaylanV2Session;

pub(crate) use repeat::{drive_key_repeat, next_poll_timeout_ms};

/// 根据当前 V2 会话算法引擎与输入模式构建候选框渲染状态
pub(crate) fn build_candidate_box_state(session: &WaylanV2Session) -> CandidateBoxState {
    crate::session_common::build_common_candidate_box_state(
        &session.core.engine,
        crate::session_common::CandidateBoxBuildOptions {
            mode: session.core.mode,
            is_active: session.is_active,
            is_grid_expanded: session.is_grid_expanded,
            grid_selected_index: session.grid_selected_index,
            inline_preedit: true,
            voice_manager: session.voice_manager.as_ref(),
        },
    )
}

/// 处理单个硬件按键事件并返回 (是否被输入法消费, 本次直接上屏文本)
pub(crate) fn process_key_event(
    session: &mut WaylanV2Session,
    key: u32,
    is_press: bool,
) -> (bool, Option<String>) {
    // 1. 统一语音按键状态机优先分发（与 IBus 体验对齐）
    if let Some(ref mut vm) = session.voice_manager {
        match vm.dispatch_voice_key(
            key,
            is_press,
            session.core.mods_depressed(),
            &mut session.core.consumed_keys,
        ) {
            VoiceKeyAction::Consumed => {
                session.refresh_candidate_surface();
                return (true, None);
            }
            VoiceKeyAction::Commit(text) => {
                session.commit_text(&text);
                session.core.punctuation_processor.record_commit(&text);
                session.refresh_candidate_surface();
                return (true, Some(text));
            }
            VoiceKeyAction::PassThrough => {}
        }
    }

    // 2. 外部快捷键拦截器优先捕获
    if let Some(mut filter) = session.key_filter.take() {
        let filter_outcome = filter(key, is_press, session.core.mods_depressed());
        session.key_filter = Some(filter);
        match filter_outcome {
            crate::session_common::KeyFilterResult::Consumed => {
                if is_press {
                    session.core.consumed_keys.record_press(key, true);
                } else {
                    session.core.consumed_keys.take_release(key);
                }
                return (true, None);
            }
            crate::session_common::KeyFilterResult::Commit(text) => {
                session.commit_text(&text);
                session.core.punctuation_processor.record_commit(&text);
                session.refresh_candidate_surface();
                if is_press {
                    session.core.consumed_keys.record_press(key, true);
                } else {
                    session.core.consumed_keys.take_release(key);
                }
                return (true, Some(text));
            }
            crate::session_common::KeyFilterResult::PassThrough => {}
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 同步前置网格状态（若外部通过 session 显式设置）
    session.core.is_grid_expanded = session.is_grid_expanded;
    session.core.grid_selected_index = session.grid_selected_index;

    // 3. 调用统一核心会话决策流水线（包含组词、中英切换、快捷键与大网格导航）
    let context_prefix = extract_surrounding_prefix(session.surrounding_text.as_ref(), 64);
    let result = session.core.process_key(
        KeyProcessContext {
            keycode: key,
            is_press,
            is_password_or_locked: session.is_password_context,
            surrounding_prefix: context_prefix,
        },
        now,
    );

    // 4. 统一驱动 Wayland 界面呈现与文本上屏
    let outcome = match result {
        KeyProcessResult::PassThrough => (false, None),
        KeyProcessResult::Handled { update_ui } => {
            if update_ui {
                let preedit = session.core.engine.formatted_preedit();
                session.update_preedit(&preedit);
            }
            (true, None)
        }
        KeyProcessResult::ExpandGrid => {
            session.refresh_candidate_surface();
            (true, None)
        }
        KeyProcessResult::Commit(text) => {
            session.commit_text(&text);
            (true, Some(text))
        }
        KeyProcessResult::CommitAndPassThrough(text) => {
            session.commit_text(&text);
            session.refresh_candidate_surface();
            (false, Some(text))
        }
        KeyProcessResult::CommitPunctuation {
            text,
            cursor_offset,
        } => {
            session.commit_text(&text);
            let dir_key = if cursor_offset < 0 {
                KEY_LEFT
            } else {
                KEY_RIGHT
            };
            for _ in 0..cursor_offset.unsigned_abs() {
                session.forward_key_to_seat(0, dir_key, true);
                session.forward_key_to_seat(0, dir_key, false);
            }
            (true, Some(text))
        }
        KeyProcessResult::CommitPartial {
            committed,
            remaining_preedit,
        } => {
            session.commit_partial(&committed, &remaining_preedit);
            (true, Some(committed))
        }
        KeyProcessResult::Reset => {
            session.cancel_input();
            (true, None)
        }
        KeyProcessResult::ShortcutToggled(_toggle) => {
            if let Some(ref notify_tx) = session.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            session.refresh_candidate_surface();
            (true, None)
        }
        KeyProcessResult::ModeChanged {
            new_mode,
            committed,
            was_consumed,
        } => {
            let is_zh = matches!(new_mode, InputMode::Chinese | InputMode::VMode);
            if let Some(ref flags) = session.shared_flags {
                flags.set_chinese(is_zh);
            } else if let Some(ref zh_flag) = session.shared_is_chinese {
                zh_flag.store(is_zh, Ordering::Release);
            }
            if let Some(ref notify_tx) = session.mode_notify_tx {
                let _ = notify_tx.try_send(());
            }
            let mode_str = match new_mode {
                InputMode::Chinese => "中文",
                InputMode::English => "英文",
                InputMode::VMode => "V模式",
            };
            debug!("[Wayland v2] 切换输入模式为：{}", mode_str);

            if let Some(ref raw) = committed {
                session.commit_text(raw);
            }
            (was_consumed, committed)
        }
    };

    // 同步核心会话决策后的网格展开与索引状态
    session.is_grid_expanded = session.core.is_grid_expanded;
    session.grid_selected_index = session.core.grid_selected_index;

    // 5. 按键重复跟踪处理
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
