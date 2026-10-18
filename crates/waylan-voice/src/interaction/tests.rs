use super::*;
use std::sync::mpsc::sync_channel;
use waylan_core::input::*;

#[test]
fn test_voice_interaction_manager_initial_state() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    assert!(!mgr.is_active());
    assert!(!mgr.is_recording());
    assert!(!mgr.is_finalizing());
    assert!(mgr.poll_events().is_none());

    mgr.clear_state();
    assert!(!mgr.is_active());
}

#[test]
fn test_voice_interaction_manager_poll_events() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    let (tx, rx) = sync_channel(16);
    mgr.state = VoiceSessionState::ToggleRecording { rx };

    // 1. 测试波形与文案更新
    tx.send(VoiceEvent::WaveformUpdated {
        volume: 0.5,
        waveform: "||||".to_string(),
    })
    .unwrap();
    assert!(mgr.poll_events().is_none());
    assert_eq!(mgr.waveform.as_deref(), Some("||||"));
    assert_eq!(mgr.display_text.as_deref(), Some("正在聆听中..."));

    // 2. 测试流式预览文字
    tx.send(VoiceEvent::PartialText("测试预览".to_string()))
        .unwrap();
    assert!(mgr.poll_events().is_none());
    assert_eq!(mgr.partial_text.as_deref(), Some("测试预览"));

    // 3. 测试自动分句提交
    tx.send(VoiceEvent::SegmentCommit("分句一".to_string()))
        .unwrap();
    assert_eq!(mgr.poll_events().as_deref(), Some("分句一"));
    assert!(mgr.partial_text.is_none());

    // 4. 测试最终完成结算
    tx.send(VoiceEvent::FinalCommit("最终识别文本。".to_string()))
        .unwrap();
    assert_eq!(mgr.poll_events().as_deref(), Some("最终识别文本。"));
    assert!(!mgr.is_active());
    assert!(matches!(mgr.state, VoiceSessionState::Idle));
}

#[test]
fn test_voice_interaction_manager_finalize_and_fallback() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    let (tx, rx) = sync_channel(16);
    mgr.state = VoiceSessionState::Finalizing {
        rx,
        start_time: Instant::now(),
        fallback_committed: false,
    };
    mgr.partial_text = Some("保底预览文字".to_string());

    // 当最终结果为空时，自动启用流式预览作为保底上屏
    tx.send(VoiceEvent::FinalCommit("   ".to_string())).unwrap();
    assert_eq!(mgr.poll_events().as_deref(), Some("保底预览文字"));
    assert!(!mgr.is_active());
    assert!(!mgr.is_finalizing());
}

#[test]
fn test_voice_interaction_manager_finalize_timeout_fallback() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    let (_tx, rx) = sync_channel(16);
    mgr.state = VoiceSessionState::Finalizing {
        rx,
        // 模拟 3 秒前进入结算（超过 2 秒超时限制）
        start_time: Instant::now() - Duration::from_secs(3),
        fallback_committed: false,
    };
    mgr.partial_text = Some("超时保底文本".to_string());

    assert_eq!(mgr.poll_events().as_deref(), Some("超时保底文本"));
    assert!(!mgr.is_active());
    assert!(!mgr.is_finalizing());
}

#[test]
fn test_voice_interaction_manager_key_interception_during_finalizing() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    let (_tx, rx) = sync_channel(16);
    mgr.config.enabled = true;
    mgr.state = VoiceSessionState::Finalizing {
        rx,
        start_time: Instant::now(),
        fallback_committed: false,
    };

    // 1. 转录结算期间按普通键应被静默拦截
    let action = mgr.handle_key_event(KEY_SPACE, true, 0);
    assert_eq!(action, VoiceKeyAction::Consumed);

    // 2. 转录结算期间按 Esc 键应取消并清理状态
    let action = mgr.handle_key_event(KEY_ESC, true, 0);
    assert_eq!(action, VoiceKeyAction::Consumed);
    assert!(!mgr.is_finalizing());
    assert!(!mgr.is_active());
}

#[test]
fn test_dispatch_voice_key_tracks_consumed_keys() {
    let mut mgr = VoiceInteractionManager::new(VoiceConfig::default());
    let (_tx, rx) = sync_channel(16);
    mgr.config.enabled = true;
    mgr.state = VoiceSessionState::Finalizing {
        rx,
        start_time: Instant::now(),
        fallback_committed: false,
    };
    let mut tracker = ConsumedKeyTracker::new();

    // 结算期间按下空格键被消费，tracker 应记账为按下
    let action = mgr.dispatch_voice_key(KEY_SPACE, true, 0, &mut tracker);
    assert_eq!(action, VoiceKeyAction::Consumed);
    assert!(tracker.is_consumed(KEY_SPACE));

    // 松开空格键被消费，tracker 应释放该键
    let action_rel = mgr.dispatch_voice_key(KEY_SPACE, false, 0, &mut tracker);
    assert_eq!(action_rel, VoiceKeyAction::Consumed);
    assert!(!tracker.is_consumed(KEY_SPACE));
}
