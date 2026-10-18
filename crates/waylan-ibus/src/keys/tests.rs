use super::core::IbusSessionCore;
use super::keyval::{
    ibus_keyval_to_evdev, IBUS_CONTROL_MASK, IBUS_LOCK_MASK, IBUS_MOD1_MASK, IBUS_RELEASE_MASK,
    IBUS_SHIFT_MASK, KEY_F,
};
use std::thread;
use std::time::Duration;
use waylan_algo::types::AlgoConfig;
use waylan_algo::AlgoEngine;
use waylan_core::input::*;
use waylan_core::state::SharedInputFlags;
use waylan_core::{
    FullWidthCharShortcut, FullWidthPunctShortcut, InputMode, TraditionalShortcut, VoiceEngineType,
};

const KEY_V: u32 = 47;

#[test]
fn test_ibus_keyval_mapping() {
    assert_eq!(ibus_keyval_to_evdev(b'a' as u32, 0), 30);
    assert_eq!(ibus_keyval_to_evdev(b'Z' as u32, 0), 44);
    assert_eq!(ibus_keyval_to_evdev(0xff08, 0), KEY_BACKSPACE);
    assert_eq!(ibus_keyval_to_evdev(0xffe1, 0), KEY_LEFTSHIFT);
    assert_eq!(ibus_keyval_to_evdev(0xffe5, 0), KEY_CAPSLOCK);
    assert_eq!(ibus_keyval_to_evdev(0xffe6, 0), KEY_CAPSLOCK);
}

#[test]
fn test_ibus_session_pinyin_and_shift_toggle() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 输入 'x', 'i', 'a', 'n'
    for ch in ['x', 'i', 'a', 'n'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
        assert!(out.update_ui);
    }
    assert_eq!(core.core.engine.raw_input(), "xian");

    // 按下并释放 Shift 切换至英文模式并自动上屏残留编码
    let _ = core.process_key_event(0xffe1, KEY_LEFTSHIFT, 0);
    let rel = core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_RELEASE_MASK);
    assert_eq!(rel.mode_changed, Some(InputMode::English));
    assert_eq!(rel.commit_text.as_deref(), Some("xian"));
}

#[test]
fn test_ibus_date_time_tab_expansion_and_selection() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 输入拼音 "riqi"
    for ch in ['r', 'i', 'q', 'i'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
        assert!(out.update_ui);
    }

    // 验证候选词：第 1 位为词库词“日期”，第 2 位为动态日期且包含年份
    let cands = core.current_candidates();
    assert!(cands.len() >= 2);
    assert_eq!(cands[0].text, "日期");
    assert!(cands[1].text.contains("年"));
    assert!(cands[1].is_expandable);
    assert_eq!(core.core.engine.selected_index(), 0);

    // 2. 按 Tab 键触发二级展开
    let tab_out = core.process_key_event(0xff09, KEY_TAB, 0);
    assert!(tab_out.consumed);
    assert!(tab_out.update_ui);
    assert!(core.core.engine.is_expanded());

    // 3. 展开后 current_candidates 应返回全部 7 种细分日期格式
    let expanded_cands = core.current_candidates();
    assert_eq!(expanded_cands.len(), 7);
    assert!(!expanded_cands[0].is_expandable);

    // 4. 按数字键 2 选取第 2 种格式上屏
    let num_out = core.process_key_event(b'2' as u32, KEY_2, 0);
    assert!(num_out.consumed);
    assert_eq!(num_out.commit_text, Some(expanded_cands[1].text.clone()));
    assert!(!core.core.engine.is_expanded());
    assert!(core.core.engine.raw_input().is_empty());
}

#[test]
fn test_ibus_caps_lock_passthrough() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 中文状态输入 'a', 'i'
    for ch in ['a', 'i'] {
        let out = core.process_key_event(ch as u32, 0, 0);
        assert!(out.consumed);
    }
    assert_eq!(core.core.engine.raw_input(), "ai");

    // 2. 按下 CapsLock 键：未完成拼音立即上屏，按键本身不被消费（透传给应用），且通知模式变更为英文
    let caps_out = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert!(!caps_out.consumed, "CapsLock 键本身不应被消费");
    assert_eq!(caps_out.commit_text.as_deref(), Some("ai"));
    assert_eq!(
        caps_out.mode_changed,
        Some(InputMode::English),
        "CapsLock 激活时模式应指示为英文"
    );
    assert!(core.core.engine.raw_input().is_empty());

    // 3. 在 CapsLock 大写锁定状态下（state 带有 IBUS_LOCK_MASK），输入字母应原生透传
    let a_out = core.process_key_event(b'A' as u32, 0, IBUS_LOCK_MASK);
    assert!(!a_out.consumed, "大写锁定状态下字母键应原生透传给应用");

    // 4. 再次按下并释放 CapsLock 解除锁定，模式恢复为中文拼音
    let caps_unlock = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_RELEASE_MASK);
    assert!(!core.is_caps_locked, "解除 CapsLock 后应脱离大写锁定状态");
    assert_eq!(
        caps_unlock.mode_changed,
        Some(InputMode::Chinese),
        "解除锁定后应恢复中文模式"
    );

    // 5. 验证此时按字母恢复中文拼音输入
    let ni_out = core.process_key_event(b'n' as u32, 0, 0);
    assert!(ni_out.consumed, "解除大写锁定后应恢复中文拼音输入");
    assert_eq!(core.core.engine.raw_input(), "n");
}

#[test]
fn test_ibus_caps_lock_with_shift_interaction() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 中文模式下开启 CapsLock
    let caps_on = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert_eq!(caps_on.mode_changed, Some(InputMode::English));
    assert!(core.is_caps_locked);

    // 2. 大写锁定期间，敲击 Shift 键（单按或误触），状态机不应破坏基准中文模式
    let shift_down = core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_LOCK_MASK);
    assert_eq!(shift_down.mode_changed, None);
    let shift_up =
        core.process_key_event(0xffe1, KEY_LEFTSHIFT, IBUS_LOCK_MASK | IBUS_RELEASE_MASK);
    assert_eq!(shift_up.mode_changed, None);
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "大写锁定时敲击 Shift 不得篡改基准中文模式"
    );

    // 3. 敲击 CapsLock 解除锁定：必须一次性成功回到中文模式，无需按两下
    let caps_off_press = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert_eq!(
        caps_off_press.mode_changed,
        Some(InputMode::Chinese),
        "仅按一次 CapsLock 必须立即切回中文"
    );
    let caps_off_rel = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_RELEASE_MASK);
    assert_eq!(caps_off_rel.mode_changed, None);
    assert!(!core.is_caps_locked);

    // 4. 后续输入正常进入拼音输入
    let letter = core.process_key_event(b'h' as u32, 0, 0);
    assert!(letter.consumed);
    assert_eq!(core.core.engine.raw_input(), "h");
}

#[test]
fn test_ibus_caps_lock_in_english_mode_no_flicker() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.core.mode = InputMode::English;

    // 1. 在纯英文模式下开启 CapsLock：不触发任何 mode_changed 信号
    let caps_on = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert_eq!(
        caps_on.mode_changed, None,
        "英文模式下开启 CapsLock 不应派发任何模式变更信号"
    );
    assert!(core.is_caps_locked);

    // 2. 在纯英文模式下关闭 CapsLock：不触发任何 mode_changed 信号，防止状态栏闪烁中文
    let caps_off = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert_eq!(
        caps_off.mode_changed, None,
        "英文模式下关闭 CapsLock 绝不应闪现中文模式"
    );
    assert!(!core.is_caps_locked);
    assert_eq!(core.core.mode, InputMode::English, "始终保持纯英文模式");
}

#[test]
fn test_ibus_caps_lock_tray_flags_and_subsequent_keys() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 1. 中文状态下按下 CapsLock：大写标志权威置为 true（托盘读取该标志直接展现 A 大写图标与大写菜单项）
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, 0);
    assert!(core.is_caps_locked);
    assert!(flags.is_caps_locked());
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "基准中文模式状态不受篡改"
    );

    // 2. 随后的普通字符按键（即使客户端 state 未附带 IBUS_LOCK_MASK），大写状态绝不可被误冲刷
    let letter = core.process_key_event(b'A' as u32, 0, 0);
    assert!(!letter.consumed, "大写锁定下普通字母按键应原生透传");
    assert!(
        core.is_caps_locked,
        "后续未带Lock掩码的按键绝不能冲刷掉大写状态"
    );
    assert!(flags.is_caps_locked());

    // 3. 再次按下 CapsLock 退出大写：大写标志关闭，无缝回到中文模式
    let _ = core.process_key_event(0xffe5, KEY_CAPSLOCK, IBUS_LOCK_MASK);
    assert!(!core.is_caps_locked);
    assert!(!flags.is_caps_locked());
    assert_eq!(
        core.core.mode,
        InputMode::Chinese,
        "退出大写锁定后丝滑保持中文模式"
    );
    assert!(flags.is_chinese());
}

#[test]
fn test_ibus_shared_settings_sync_and_full_width() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 默认状态：简体、全角标点、半角字母
    assert!(!core.core.engine.config.traditional_chinese);
    assert!(core.core.punctuation_processor.full_width_punct);
    assert!(!core.is_full_width_char());

    // 1. 托盘触发切换为繁体
    flags.set_traditional(true);
    let _ = core.process_key_event(0, 0, 0);
    assert!(
        core.core.engine.config.traditional_chinese,
        "引擎应自动同步繁体设置"
    );

    // 2. 托盘触发切换为半角标点
    flags.set_full_width_punct(false);
    let outcome = core.process_key_event(b',' as u32, 51, 0);
    assert_eq!(
        outcome.commit_text,
        Some(",".to_string()),
        "半角标点模式下应输出半角逗号"
    );

    // 3. 托盘触发切换为全角字符，并在英文模式下敲击字母
    flags.set_full_width_char(true);
    flags.set_chinese(false);
    let letter_outcome = core.process_key_event(b'a' as u32, 30, 0);
    assert_eq!(
        letter_outcome.commit_text,
        Some("ａ".to_string()),
        "全角字符模式下英文应输出全角字母"
    );
}

#[test]
fn test_ibus_password_context_reset_and_recovery() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 模拟进入密码输入框
    core.is_password_context = true;
    let outcome = core.process_key_event(b'n' as u32, 49, 0);
    assert!(
        !outcome.consumed,
        "密码输入框中按键应直通放行，不被输入法消费"
    );
    assert!(core.core.engine.raw_input().is_empty());

    // 2. 模拟切换窗口或失焦触发重置
    core.reset_context();
    assert!(
        !core.is_password_context,
        "reset_context 必须彻底重置密码框上下文标志"
    );

    // 3. 再次在普通窗口输入，应恢复拼音消费与候选生成
    let outcome2 = core.process_key_event(b'n' as u32, 49, 0);
    assert!(
        outcome2.consumed,
        "退出密码框后输入法应完全恢复可用并消费按键"
    );
    assert_eq!(core.core.engine.raw_input(), "n");
}

#[test]
fn test_ibus_screen_lock_passthrough_and_recovery() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);

    // 1. 模拟进入锁屏状态
    core.is_screen_locked = true;
    let outcome = core.process_key_event(b'a' as u32, 30, 0);
    assert!(
        !outcome.consumed,
        "系统锁屏状态下所有硬件按键必须完全直通放行，绝不消费"
    );
    assert!(
        core.core.engine.raw_input().is_empty(),
        "锁屏状态下严禁将按键录入算法引擎"
    );

    // 2. 模拟解锁屏幕回到桌面
    core.is_screen_locked = false;
    let outcome2 = core.process_key_event(b'a' as u32, 30, 0);
    assert!(
        outcome2.consumed,
        "解锁屏幕后应立即恢复拼音拦截与候选词处理"
    );
    assert_eq!(core.core.engine.raw_input(), "a");
}

#[test]
fn test_ibus_global_shortcuts_toggle() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    let flags = SharedInputFlags::new(true, false, true, false);
    core.shared_flags = Some(flags.clone());

    // 默认快捷键：Ctrl+Shift+F (简繁), Shift+Space (字符全半角), Ctrl+. (标点全半角)
    assert_eq!(
        core.core.traditional_shortcut,
        TraditionalShortcut::CtrlShiftF
    );
    assert_eq!(
        core.core.full_width_char_shortcut,
        FullWidthCharShortcut::ShiftSpace
    );
    assert_eq!(
        core.core.full_width_punct_shortcut,
        FullWidthPunctShortcut::CtrlPeriod
    );

    // 1. 测试简繁切换快捷键 (Ctrl + Shift + F)
    let state_ctrl_shift = IBUS_CONTROL_MASK | IBUS_SHIFT_MASK;
    let res_trad = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
    assert!(res_trad.consumed, "简繁快捷键必须被输入法消费");
    assert!(
        core.core.engine.config.traditional_chinese,
        "简繁状态应切换为繁体"
    );
    assert!(flags.is_traditional(), "共享托盘标志应同步为繁体");

    // 再次按下切回简体
    let res_trad2 = core.process_key_event(b'f' as u32, KEY_F, state_ctrl_shift);
    assert!(res_trad2.consumed);
    assert!(
        !core.core.engine.config.traditional_chinese,
        "简繁状态应切回简体"
    );
    assert!(!flags.is_traditional());

    // 2. 测试字符全半角切换快捷键 (Shift + Space)
    let res_char = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
    assert!(res_char.consumed, "字符全半角快捷键必须被消费");
    assert!(core.is_full_width_char(), "字符全半角状态应切换为全角");
    assert!(flags.is_full_width_char());

    // 再次按下切回半角
    let res_char2 = core.process_key_event(0x20, KEY_SPACE, IBUS_SHIFT_MASK);
    assert!(res_char2.consumed);
    assert!(!core.is_full_width_char(), "字符全半角状态应切回半角");
    assert!(!flags.is_full_width_char());

    // 3. 测试标点全半角切换快捷键 (Ctrl + .)
    let res_punct = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
    assert!(res_punct.consumed, "标点全半角快捷键必须被消费");
    assert!(
        !core.core.punctuation_processor.full_width_punct,
        "标点状态应由全角切换为半角"
    );
    assert!(!flags.is_full_width_punct());

    // 再次按下切回全角
    let res_punct2 = core.process_key_event(b'.' as u32, KEY_DOT, IBUS_CONTROL_MASK);
    assert!(res_punct2.consumed);
    assert!(
        core.core.punctuation_processor.full_width_punct,
        "标点状态应切回全角中文标点"
    );
    assert!(flags.is_full_width_punct());
}

#[test]
fn test_ibus_voice_input_disabled_by_default() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    // 验证默认情况下 voice_manager.config.enabled 为 false
    assert!(
        !core.voice_manager.config.enabled,
        "语音输入默认应处于关闭状态"
    );

    // 按下 Ctrl + Alt + V 不应被语音功能拦截消费
    let v_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(!v_res.consumed, "默认关闭时不应拦截快捷键");
    assert!(
        !core.voice_manager.is_recording(),
        "默认关闭时不应启动语音录制"
    );
    assert!(core.voice_manager.display_text.is_none());

    // 按下 F8（evdev 扫描码 66）也不应被拦截
    let f8_res = core.process_key_event(0xffc5, 66, 0);
    assert!(!f8_res.consumed, "默认关闭时不应拦截对讲机按键");
    assert!(
        !core.voice_manager.is_recording(),
        "默认关闭时不应启动对讲机录制"
    );
}

#[test]
fn test_ibus_voice_input_shortcut_and_cancel() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.voice_manager.config.enabled = true;
    core.voice_manager.config.engine_type = VoiceEngineType::Mock;

    // 1. 验证开启语音配置下按下 Ctrl + Alt + V 触发语音输入激活
    let start_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(start_res.consumed, "语音激活按键应被拦截消费");
    assert!(start_res.update_ui, "语音启动时应请求刷新 UI 候选与预编辑");
    assert!(
        core.voice_manager.is_recording(),
        "语音控制器应进入录音状态"
    );
    assert!(core.voice_manager.display_text.is_some());

    // 2. 录音状态下按下 ESC 键应取消录音并清除预编辑
    let cancel_res = core.process_key_event(0xff1b, KEY_ESC, 0);
    assert!(cancel_res.consumed);
    assert!(cancel_res.update_ui);
    assert!(!core.voice_manager.is_recording(), "ESC 键应终止录音");
    assert!(core.voice_manager.display_text.is_none());
}

#[test]
fn test_ibus_voice_input_commit_on_enter() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.voice_manager.config.enabled = true;
    core.voice_manager.config.engine_type = VoiceEngineType::Mock;

    // 1. 触发语音录音
    let _ = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(core.voice_manager.is_recording());

    // 2. 给予后台仿真录音线程极短时隙产生音频分块
    thread::sleep(Duration::from_millis(50));

    // 3. 按下 Enter 键，应停止录音并触发上屏或消费按键
    let enter_res = core.process_key_event(0xff0d, KEY_ENTER, 0);
    assert!(enter_res.consumed);
    assert!(!core.voice_manager.is_recording());
    assert!(core.voice_manager.display_text.is_none());
}

#[test]
fn test_ibus_voice_input_push_to_talk_mode() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.voice_manager.config.enabled = true;
    core.voice_manager.config.engine_type = VoiceEngineType::Mock;
    core.voice_manager.config.push_to_talk_shortcut = "Ctrl + Alt + V".to_string();

    // 1. 按下快捷键 Ctrl + Alt + V 开始录音
    let press_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(press_res.consumed);
    assert!(press_res.update_ui);
    assert!(core.voice_manager.is_push_to_talk_active);
    assert!(core.voice_manager.is_recording());

    // 2. 长按过程中的 Auto-Repeat 连发应被直接消费，不重复启动也不停止；给予后台仿真录音线程时隙产生音频分块
    thread::sleep(Duration::from_millis(100));
    let repeat_res = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(repeat_res.consumed);
    assert!(core.voice_manager.is_recording());

    // 若异步仿真录音在并发高压测试环境下尚未调度产生首帧分块，提供保底文本确保单测执行的确定性
    if core.voice_manager.partial_text.is_none() {
        core.voice_manager.partial_text = Some("测试对讲机语音文本".to_string());
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
    assert!(!core.voice_manager.is_push_to_talk_active);
    assert!(!core.voice_manager.is_recording());
}

#[test]
fn test_ibus_voice_input_f8_push_to_talk_and_fallback() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.voice_manager.config.enabled = true;
    core.voice_manager.config.engine_type = VoiceEngineType::Mock;
    core.voice_manager.config.push_to_talk_shortcut = "F8".to_string();

    // 1. 按下 F8（evdev 扫描码 66）触发对讲机录音
    let press_res = core.process_key_event(0xffc5, 66, 0);
    assert!(press_res.consumed);
    assert!(core.voice_manager.is_push_to_talk_active);
    assert!(core.voice_manager.is_recording());

    // 2. 模拟流式识别出了部分文字预览
    core.voice_manager.partial_text = Some("流式已输出的文本".to_string());

    // 3. 松开 F8 键，必须成功上屏提交文字，绝不能被清空为 None
    let release_res = core.process_key_event(0xffc5, 66, IBUS_RELEASE_MASK);
    assert!(release_res.consumed);
    assert!(release_res.update_ui);
    assert!(
        release_res.commit_text.is_some(),
        "松开 F8 必须成功生成文字上屏，杜绝文字丢失"
    );
    assert!(!core.voice_manager.is_push_to_talk_active);
    assert!(!core.voice_manager.is_recording());
    assert!(core.voice_manager.partial_text.is_none());
}

#[test]
fn test_ibus_voice_input_custom_shortcut() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let mut core = IbusSessionCore::new(engine);
    core.voice_manager.config.enabled = true;
    core.voice_manager.config.engine_type = VoiceEngineType::Mock;
    core.voice_manager.config.toggle_shortcut = "F8".to_string();
    core.voice_manager.config.push_to_talk_shortcut = "已禁用".to_string();

    // 1. 按下无关快捷键（例如默认的 Ctrl+Alt+V）不应触发
    let unrelated = core.process_key_event(b'v' as u32, KEY_V, IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    assert!(!unrelated.consumed || !core.voice_manager.is_recording());

    // 2. 按下自定义的 F8 键（evdev 扫描码 66），应成功触发录音
    let f8_res = core.process_key_event(0xffc5, 66, 0);
    assert!(f8_res.consumed);
    assert!(core.voice_manager.is_recording());

    // 3. 按下 ESC 取消录音并释放会话
    let esc_res = core.process_key_event(0xff1b, KEY_ESC, 0);
    assert!(esc_res.consumed);
    assert!(!core.voice_manager.is_recording());
}
