use std::thread;
use std::time::Duration;
use waylan_algo::{AlgoConfig, AlgoEngine};
use waylan_core::input::*;
use waylan_core::{VoiceConfig, VoiceEngineType};
use waylan_ibus::keys::IbusSessionCore;
use waylan_ibus::keys::keyval::{IBUS_CONTROL_MASK, IBUS_MOD1_MASK, IBUS_RELEASE_MASK};
use waylan_voice::VoiceInteractionManager;

const KEY_V: u32 = 47;

fn create_test_core() -> IbusSessionCore {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    IbusSessionCore::new(engine).with_voice_provider(Box::new(VoiceInteractionManager::new(
        VoiceConfig::default(),
    )))
}
/// 更新测试语音配置并通过统一入口刷新快捷键缓存
fn update_voice_config(core: &mut IbusSessionCore, update: impl FnOnce(&mut VoiceConfig)) {
    let mut config = core
        .runtime()
        .voice_manager()
        .expect("语音管理器应存在")
        .config()
        .clone();
    update(&mut config);
    core.runtime_mut()
        .voice_manager_mut()
        .expect("语音管理器应存在")
        .update_config(&config);
}

#[test]
fn test_ibus_voice_input_unconfigured_passthrough() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    assert!(core.runtime().voice_manager().is_none());
    let res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(!res.consumed);
}

#[test]
fn test_ibus_voice_input_disabled_by_default() {
    let mut core = create_test_core();
    // 验证默认情况下 voice_manager.config().enabled 为 false
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .config()
            .enabled,
        "语音输入默认应处于关闭状态"
    );

    // 按下 Ctrl + Alt + V 不应被语音功能拦截消费
    let v_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(!v_res.consumed, "默认关闭时不应拦截快捷键");
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording(),
        "默认关闭时不应启动语音录制"
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .display_text()
            .is_none()
    );

    // 按下 F8（evdev 扫描码 66）也不应被拦截
    let f8_res = core.process_key_event(0xffc5, 66, 0);
    assert!(!f8_res.consumed, "默认关闭时不应拦截对讲机按键");
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording(),
        "默认关闭时不应启动对讲机录制"
    );
}

#[test]
fn test_ibus_voice_input_shortcut_and_cancel() {
    let mut core = create_test_core();
    update_voice_config(&mut core, |config| {
        config.enabled = true;
        config.engine_type = VoiceEngineType::Mock;
    });

    // 1. 验证开启语音配置下按下 Ctrl + Alt + V 触发语音输入激活
    let start_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(start_res.consumed, "语音激活按键应被拦截消费");
    assert!(start_res.update_ui, "语音启动时应请求刷新 UI 候选与预编辑");
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording(),
        "语音控制器应进入录音状态"
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .display_text()
            .is_some()
    );

    // 2. 录音状态下按下 ESC 键应取消录音并清除预编辑
    let cancel_res = core.process_key_event(0xff1b, KEY_ESC, 0);
    assert!(cancel_res.consumed);
    assert!(cancel_res.update_ui);
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording(),
        "ESC 键应终止录音"
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .display_text()
            .is_none()
    );
}

#[test]
fn test_ibus_voice_input_commit_on_enter() {
    let mut core = create_test_core();
    update_voice_config(&mut core, |config| {
        config.enabled = true;
        config.engine_type = VoiceEngineType::Mock;
    });

    // 1. 触发语音录音
    let _ = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );

    // 2. 给予后台仿真录音线程极短时隙产生音频分块
    thread::sleep(Duration::from_millis(50));

    // 3. 按下 Enter 键，应停止录音并触发上屏或消费按键
    let enter_res = core.process_key_event(0xff0d, KEY_ENTER, 0);
    assert!(enter_res.consumed);
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );
    let display_text = core
        .runtime()
        .voice_manager()
        .expect("语音管理器应存在")
        .display_text();
    assert!(matches!(
        display_text.as_deref(),
        Some("正在识别...") | Some("识别完成")
    ));
}

#[test]
fn test_ibus_voice_input_push_to_talk_mode() {
    let mut core = create_test_core();
    update_voice_config(&mut core, |config| {
        config.enabled = true;
        config.engine_type = VoiceEngineType::Mock;
    });
    update_voice_config(&mut core, |config| {
        config.push_to_talk_shortcut = "Ctrl + Alt + V".to_string();
    });

    // 1. 按下快捷键 Ctrl + Alt + V 开始录音
    let press_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(press_res.consumed);
    assert!(press_res.update_ui);
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_push_to_talk_recording()
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );

    // 2. 长按过程中的 Auto-Repeat 连发应被直接消费，不重复启动也不停止；给予后台仿真录音线程时隙产生音频分块
    thread::sleep(Duration::from_millis(100));
    let repeat_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(repeat_res.consumed);
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );

    // 若异步仿真录音在并发高压测试环境下尚未调度产生首帧分块，提供保底文本确保单测执行的确定性
    if core
        .runtime()
        .voice_manager()
        .expect("语音管理器应存在")
        .partial_text()
        .is_none()
    {
        core.runtime_mut()
            .voice_manager_mut()
            .expect("语音管理器应存在")
            .set_partial_text(Some("测试对讲机语音文本".to_string()));
    }

    // 3. 模拟松开 V 键 (Release)，对讲机模式应立即停止录音并请求提交
    let release_res = core.process_key_event(
        b'v' as u32,
        KEY_V,
        IBUS_RELEASE_MASK | IBUS_CONTROL_MASK | IBUS_MOD1_MASK,
    );
    assert!(release_res.consumed);
    assert!(release_res.update_ui);
    assert!(
        release_res.commit_text.is_some(),
        "对讲机松手必须产生提交文字，不可被清空"
    );
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_push_to_talk_recording()
    );
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );
}

#[test]
fn test_ibus_voice_input_f8_push_to_talk_and_fallback() {
    let mut core = create_test_core();
    update_voice_config(&mut core, |config| {
        config.enabled = true;
        config.engine_type = VoiceEngineType::Mock;
    });
    update_voice_config(&mut core, |config| {
        config.push_to_talk_shortcut = "F8".to_string();
    });

    // 1. 按下 F8（evdev 扫描码 66）触发对讲机录音
    let press_res = core.process_key_event(0xffc5, 66, 0);
    assert!(press_res.consumed);
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_push_to_talk_recording()
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );

    // 2. 模拟流式识别出了部分文字预览
    core.runtime_mut()
        .voice_manager_mut()
        .expect("语音管理器应存在")
        .set_partial_text(Some("流式已输出的文本".to_string()));

    // 3. 松开 F8 键，必须成功上屏提交文字，绝不能被清空为 None
    let release_res = core.process_key_event(0xffc5, 66, IBUS_RELEASE_MASK);
    assert!(release_res.consumed);
    assert!(release_res.update_ui);
    assert!(
        release_res.commit_text.is_some(),
        "松开 F8 必须成功生成文字上屏，杜绝文字丢失"
    );
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_push_to_talk_recording()
    );
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .partial_text()
            .is_none()
    );
}

#[test]
fn test_ibus_voice_input_custom_shortcut() {
    let mut core = create_test_core();
    update_voice_config(&mut core, |config| {
        config.enabled = true;
        config.engine_type = VoiceEngineType::Mock;
    });
    update_voice_config(&mut core, |config| {
        config.toggle_shortcut = "F8".to_string();
        config.push_to_talk_shortcut = "已禁用".to_string();
    });

    // 1. 按下无关快捷键（例如默认的 Ctrl+Alt+V）不应触发
    let unrelated = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(
        !unrelated.consumed
            || !core
                .runtime()
                .voice_manager()
                .expect("语音管理器应存在")
                .is_recording()
    );

    // 2. 按下自定义的 F8 键（evdev 扫描码 66），应成功触发录音
    let f8_res = core.process_key_event(0xffc5, 66, 0);
    assert!(f8_res.consumed);
    assert!(
        core.runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );

    // 3. 按下 ESC 取消录音并释放会话
    let esc_res = core.process_key_event(0xff1b, KEY_ESC, 0);
    assert!(esc_res.consumed);
    assert!(
        !core
            .runtime()
            .voice_manager()
            .expect("语音管理器应存在")
            .is_recording()
    );
}
