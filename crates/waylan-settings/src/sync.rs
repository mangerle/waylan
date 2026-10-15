//! 配置模型与 Slint 窗口属性双向数据同步模块
//!
//! # 设计原理
//! - **实现初衷**：解耦界面渲染与核心配置领域模型，集中管理 GUI 属性与持久化配置的映射。
//! - **核心优势**：
//!   1. 集中防守，对索引越界、异常数值进行安全边界校验与降级；
//!   2. 避免在事件回调中分散编写冗长脆弱的属性映射逻辑；
//!   3. 支持无缝双向转换，便于单元测试与自动化验证。

use crate::{CloudProfileItem, SettingsWindow};
use waylan_core::config::{
    AppConfig, CandidateIndexStyle, CandidateOrientation, FullWidthCharShortcut,
    FullWidthPunctShortcut, FuzzyPinyinRules, PageKeys, SwitchKey, TraditionalShortcut,
    TypoCorrectionRules,
};

/// 将 AppConfig 数据应用到 Slint 界面属性
pub fn sync_config_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    // 初始状态
    window.set_init_lang_index(if cfg.init_chinese { 0 } else { 1 });
    window.set_init_char_width_index(if cfg.full_width_char { 1 } else { 0 });
    window.set_init_punct_width_index(if cfg.full_width_punct { 0 } else { 1 });
    window.set_init_simp_trad_index(if cfg.traditional_chinese { 1 } else { 0 });

    // 候选框外观
    window.set_page_size_val(cfg.page_size as f32);
    window.set_font_size_val(cfg.font_size);
    window.set_orientation_index(match cfg.candidate_orientation {
        CandidateOrientation::Horizontal => 0,
        CandidateOrientation::Vertical => 1,
    });
    window.set_index_style_index(match cfg.candidate_index_style {
        CandidateIndexStyle::ModernSpace => 0,
        CandidateIndexStyle::ClassicDot => 1,
        CandidateIndexStyle::Circled => 2,
        CandidateIndexStyle::SquareBracket => 3,
        CandidateIndexStyle::Parenthesis => 4,
        CandidateIndexStyle::MinimalFirstDot => 5,
        CandidateIndexStyle::None => 6,
    });

    // 按键习惯
    window.set_switch_key_index(match cfg.switch_key {
        SwitchKey::Shift => 0,
        SwitchKey::Ctrl => 1,
    });
    window.set_page_keys_minus_equal(cfg.page_keys.minus_equal);
    window.set_page_keys_bracket(cfg.page_keys.bracket);
    window.set_page_keys_comma_period(cfg.page_keys.comma_period);
    window.set_traditional_shortcut_index(match cfg.traditional_shortcut {
        TraditionalShortcut::CtrlShiftF => 0,
        TraditionalShortcut::CtrlAltF => 1,
        TraditionalShortcut::CtrlShiftT => 2,
        TraditionalShortcut::Disabled => 3,
    });
    window.set_full_width_char_shortcut_index(match cfg.full_width_char_shortcut {
        FullWidthCharShortcut::ShiftSpace => 0,
        FullWidthCharShortcut::CtrlShiftSpace => 1,
        FullWidthCharShortcut::Disabled => 2,
    });
    window.set_full_width_punct_shortcut_index(match cfg.full_width_punct_shortcut {
        FullWidthPunctShortcut::CtrlPeriod => 0,
        FullWidthPunctShortcut::AltPeriod => 1,
        FullWidthPunctShortcut::Disabled => 2,
    });

    // 输入与标点特性
    window.set_dynamic_adjust_freq(cfg.dynamic_adjust_freq);
    window.set_shift_select_second(cfg.shift_select_second);
    window.set_auto_pangu_spacing(cfg.auto_pangu_spacing);
    window.set_v_mode(cfg.v_mode);
    window.set_slash_to_dunhao(cfg.slash_to_dunhao);
    window.set_protect_digits_punctuation(cfg.protect_digits_punctuation);
    window.set_auto_complete_pairs(cfg.auto_complete_pairs);

    // 拼音检索与纠错
    window.set_enable_acronym(cfg.enable_acronym);
    window.set_typo_enable(cfg.typo_rules.enable);
    window.set_typo_gn_ng(cfg.typo_rules.gn_ng);
    window.set_typo_mg_ng(cfg.typo_rules.mg_ng);
    window.set_typo_iou_iu(cfg.typo_rules.iou_iu);
    window.set_typo_uei_ui(cfg.typo_rules.uei_ui);
    window.set_typo_uen_un(cfg.typo_rules.uen_un);
    window.set_typo_ue_ve(cfg.typo_rules.ue_ve);
    window.set_typo_v_u(cfg.typo_rules.v_u);
    window.set_typo_on_ong(cfg.typo_rules.on_ong);

    // 模糊音
    window.set_fuzzy_pinyin(cfg.fuzzy_pinyin);
    window.set_fuzzy_c_ch(cfg.fuzzy_rules.c_ch);
    window.set_fuzzy_z_zh(cfg.fuzzy_rules.z_zh);
    window.set_fuzzy_s_sh(cfg.fuzzy_rules.s_sh);
    window.set_fuzzy_an_ang(cfg.fuzzy_rules.an_ang);
    window.set_fuzzy_en_eng(cfg.fuzzy_rules.en_eng);
    window.set_fuzzy_in_ing(cfg.fuzzy_rules.in_ing);
    window.set_fuzzy_l_n(cfg.fuzzy_rules.l_n);
    window.set_fuzzy_f_h(cfg.fuzzy_rules.f_h);
    window.set_fuzzy_r_l(cfg.fuzzy_rules.r_l);
    window.set_fuzzy_g_k(cfg.fuzzy_rules.g_k);

    // 系统托盘与指示器
    window.set_enable_tray_index(match (cfg.enable_tray, cfg.exclusive_input_source) {
        (Some(false), _) => 1,    // GNOME 原生单图标
        (Some(true), false) => 2, // 双指示器并存
        _ => 0,                   // 微澜专属彩色托盘（推荐）
    });

    // 高级模式与候选特性
    window.set_enable_u_mode(cfg.enable_u_mode);
    window.set_enable_i_mode(cfg.enable_i_mode);
    window.set_filter_uncommon_charset(cfg.filter_uncommon_charset);
    window.set_show_pinyin_comment(cfg.show_pinyin_comment);
    window.set_auto_dedup(cfg.auto_dedup);

    // 语音输入特性与模型同步
    sync_voice_to_window(window, cfg);
}

/// 基于 Slint 界面属性增量更新已有的 AppConfig（保留未在界面中展示的高级配置项，防误清空）
pub fn update_config_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    cfg.init_chinese = window.get_init_lang_index() == 0;
    cfg.full_width_char = window.get_init_char_width_index() == 1;
    cfg.full_width_punct = window.get_init_punct_width_index() == 0;
    cfg.traditional_chinese = window.get_init_simp_trad_index() == 1;
    cfg.switch_key = match window.get_switch_key_index() {
        1 => SwitchKey::Ctrl,
        _ => SwitchKey::Shift,
    };
    cfg.page_keys = PageKeys {
        minus_equal: window.get_page_keys_minus_equal(),
        bracket: window.get_page_keys_bracket(),
        comma_period: window.get_page_keys_comma_period(),
    };
    cfg.traditional_shortcut = match window.get_traditional_shortcut_index() {
        1 => TraditionalShortcut::CtrlAltF,
        2 => TraditionalShortcut::CtrlShiftT,
        3 => TraditionalShortcut::Disabled,
        _ => TraditionalShortcut::CtrlShiftF,
    };
    cfg.full_width_char_shortcut = match window.get_full_width_char_shortcut_index() {
        1 => FullWidthCharShortcut::CtrlShiftSpace,
        2 => FullWidthCharShortcut::Disabled,
        _ => FullWidthCharShortcut::ShiftSpace,
    };
    cfg.full_width_punct_shortcut = match window.get_full_width_punct_shortcut_index() {
        1 => FullWidthPunctShortcut::AltPeriod,
        2 => FullWidthPunctShortcut::Disabled,
        _ => FullWidthPunctShortcut::CtrlPeriod,
    };
    cfg.page_size = window.get_page_size_val().round() as usize;
    cfg.font_size = window.get_font_size_val().round();
    cfg.candidate_orientation = match window.get_orientation_index() {
        1 => CandidateOrientation::Vertical,
        _ => CandidateOrientation::Horizontal,
    };
    cfg.candidate_index_style = match window.get_index_style_index() {
        1 => CandidateIndexStyle::ClassicDot,
        2 => CandidateIndexStyle::Circled,
        3 => CandidateIndexStyle::SquareBracket,
        4 => CandidateIndexStyle::Parenthesis,
        5 => CandidateIndexStyle::MinimalFirstDot,
        6 => CandidateIndexStyle::None,
        _ => CandidateIndexStyle::ModernSpace,
    };
    cfg.dynamic_adjust_freq = window.get_dynamic_adjust_freq();
    cfg.enable_acronym = window.get_enable_acronym();
    cfg.shift_select_second = window.get_shift_select_second();
    cfg.slash_to_dunhao = window.get_slash_to_dunhao();
    cfg.protect_digits_punctuation = window.get_protect_digits_punctuation();
    cfg.auto_complete_pairs = window.get_auto_complete_pairs();
    cfg.v_mode = window.get_v_mode();
    cfg.fuzzy_pinyin = window.get_fuzzy_pinyin();
    cfg.fuzzy_rules = FuzzyPinyinRules {
        c_ch: window.get_fuzzy_c_ch(),
        z_zh: window.get_fuzzy_z_zh(),
        s_sh: window.get_fuzzy_s_sh(),
        an_ang: window.get_fuzzy_an_ang(),
        en_eng: window.get_fuzzy_en_eng(),
        in_ing: window.get_fuzzy_in_ing(),
        l_n: window.get_fuzzy_l_n(),
        f_h: window.get_fuzzy_f_h(),
        r_l: window.get_fuzzy_r_l(),
        g_k: window.get_fuzzy_g_k(),
    };
    cfg.typo_rules = TypoCorrectionRules {
        enable: window.get_typo_enable(),
        gn_ng: window.get_typo_gn_ng(),
        mg_ng: window.get_typo_mg_ng(),
        iou_iu: window.get_typo_iou_iu(),
        uei_ui: window.get_typo_uei_ui(),
        uen_un: window.get_typo_uen_un(),
        ue_ve: window.get_typo_ue_ve(),
        v_u: window.get_typo_v_u(),
        on_ong: window.get_typo_on_ong(),
    };
    cfg.auto_pangu_spacing = window.get_auto_pangu_spacing();
    match window.get_enable_tray_index() {
        1 => {
            // GNOME 原生单图标（融入系统，关闭微澜独立托盘）
            cfg.enable_tray = Some(false);
            cfg.exclusive_input_source = false;
        }
        2 => {
            // 双指示器并存
            cfg.enable_tray = Some(true);
            cfg.exclusive_input_source = false;
        }
        _ => {
            // 微澜专属彩色托盘（推荐）
            cfg.enable_tray = Some(true);
            cfg.exclusive_input_source = true;
        }
    }
    cfg.enable_u_mode = window.get_enable_u_mode();
    cfg.enable_i_mode = window.get_enable_i_mode();
    cfg.filter_uncommon_charset = window.get_filter_uncommon_charset();
    cfg.show_pinyin_comment = window.get_show_pinyin_comment();
    cfg.auto_dedup = window.get_auto_dedup();

    // 语音输入特性与模型同步
    update_voice_from_window(window, cfg);

    cfg.sanitize();
}

/// 同步语音输入配置与模型状态至 Slint 界面属性
fn sync_voice_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    window.set_voice_enabled(cfg.voice.enabled);

    // 连续听写开关快捷键 (Toggle)
    let toggle_str = cfg.voice.toggle_shortcut.trim();
    window.set_toggle_shortcut(slint::SharedString::from(toggle_str));
    let toggle_idx = if toggle_str.is_empty()
        || toggle_str.eq_ignore_ascii_case("disabled")
        || toggle_str == "已禁用"
    {
        5
    } else if toggle_str.eq_ignore_ascii_case("ctrl + alt + v")
        || toggle_str.eq_ignore_ascii_case("ctrl+alt+v")
    {
        0
    } else if toggle_str.eq_ignore_ascii_case("alt + space")
        || toggle_str.eq_ignore_ascii_case("alt+space")
    {
        1
    } else if toggle_str.eq_ignore_ascii_case("ctrl + space")
        || toggle_str.eq_ignore_ascii_case("ctrl+space")
    {
        2
    } else if toggle_str.eq_ignore_ascii_case("super + v")
        || toggle_str.eq_ignore_ascii_case("super+v")
    {
        3
    } else {
        4
    };
    window.set_toggle_shortcut_index(toggle_idx);

    // 长按对讲机快捷键 (Push-to-Talk)
    let ptt_str = cfg.voice.push_to_talk_shortcut.trim();
    window.set_push_to_talk_shortcut(slint::SharedString::from(ptt_str));
    let ptt_idx = if ptt_str.is_empty()
        || ptt_str.eq_ignore_ascii_case("disabled")
        || ptt_str == "已禁用"
    {
        5
    } else if ptt_str.eq_ignore_ascii_case("f8") {
        0
    } else if ptt_str.eq_ignore_ascii_case("alt + space")
        || ptt_str.eq_ignore_ascii_case("alt+space")
    {
        1
    } else if ptt_str.eq_ignore_ascii_case("ctrl + space")
        || ptt_str.eq_ignore_ascii_case("ctrl+space")
    {
        2
    } else if ptt_str.eq_ignore_ascii_case("super + v") || ptt_str.eq_ignore_ascii_case("super+v") {
        3
    } else {
        4
    };
    window.set_ptt_shortcut_index(ptt_idx);

    // 阶段一 AFE 前端增强
    window.set_afe_enabled(cfg.voice.afe.enabled);
    window.set_high_pass_filter(cfg.voice.afe.high_pass_filter);
    window.set_noise_suppression(cfg.voice.afe.noise_suppression);
    window.set_auto_gain_control(cfg.voice.afe.auto_gain_control);

    // 阶段二 VAD 端点截断
    window.set_vad_silence_delay_index(match cfg.voice.vad.silence_timeout_ms {
        0..=500 => 0,
        501..=700 => 1,
        701..=900 => 2,
        _ => 3,
    });

    // 阶段四 标点预测
    window.set_auto_punctuation(cfg.voice.auto_punctuation);

    // 阶段五 文本规整与语气词过滤 (ITN)
    window.set_itn_convert_numbers(cfg.voice.itn.convert_numbers);
    window.set_itn_filter_filler_words(cfg.voice.itn.filter_filler_words);
    window.set_itn_pangu_spacing(cfg.voice.itn.pangu_spacing);

    // 阶段六 交互展现与流式上屏
    window.set_voice_real_time_preview(cfg.voice.interaction.real_time_preview);
    window.set_voice_show_floating_bar(cfg.voice.interaction.show_floating_bar);

    // 云端服务 Profile 列表装载
    let active_cloud_id = if let Some(ref sel) = cfg.voice.selected_asr {
        sel.strip_prefix("cloud:")
    } else {
        None
    };
    let cloud_items: Vec<CloudProfileItem> = cfg
        .voice
        .cloud_profiles
        .iter()
        .map(|p| CloudProfileItem {
            id: slint::SharedString::from(&p.id),
            name: slint::SharedString::from(&p.name),
            endpoint: slint::SharedString::from(&p.endpoint),
            api_key: slint::SharedString::from(
                p.api_key
                    .as_deref()
                    .map(|k| {
                        if k.len() > 6 {
                            format!("{}******", &k[..4])
                        } else {
                            "******".to_string()
                        }
                    })
                    .unwrap_or_else(|| "未设置".to_string()),
            ),
            model: slint::SharedString::from(&p.model),
            is_active: active_cloud_id == Some(p.id.as_str()),
        })
        .collect();
    window.set_cloud_profiles(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(cloud_items),
    )));

    sync_voice_models_to_window(window, cfg);
}

/// 同步语音四大模型（ASR / VAD / 标点 / NS降噪）列表与当前选中项至界面
fn sync_voice_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    // 1. 统一 ASR 模型下拉列表（本地离线模型 + 线上云端 Profile）
    let local_asrs = waylan_voice::list_asr_models();
    let mut all_asr_names: Vec<slint::SharedString> = Vec::new();
    for m in &local_asrs {
        all_asr_names.push(slint::SharedString::from(format!(
            "[本地] {}",
            m.display_name
        )));
    }
    for p in &cfg.voice.cloud_profiles {
        all_asr_names.push(slint::SharedString::from(format!(
            "[云端] {} ({})",
            p.name, p.model
        )));
    }

    let mut selected_idx = 0;
    let mut is_cloud = false;
    let mut notice = "当前处于【本地离线模式】: 采用阿里 Paraformer 流式 int8 ONNX 引擎，所有音频计算均在本地完成，零延迟、零网络依赖、保障隐私安全。".to_string();

    if let Some(ref sel) = cfg.voice.selected_asr {
        if let Some(cloud_id) = sel.strip_prefix("cloud:") {
            if let Some(pos) = cfg
                .voice
                .cloud_profiles
                .iter()
                .position(|p| p.id == cloud_id)
            {
                selected_idx = local_asrs.len() + pos;
                is_cloud = true;
                let profile = &cfg.voice.cloud_profiles[pos];
                notice = format!(
                    "当前处于【云端转写模式 - {}】: 说话结束后将音频打包发送至目标云端 API，由远端 {} 模型转录。支持超大参数高精转写，请保持网络畅通。",
                    profile.name, profile.model
                );
            }
        } else {
            // 本地模型匹配
            if let Some(ref dir) = cfg.voice.offline.model_dir {
                for (i, m) in local_asrs.iter().enumerate() {
                    if m.path.as_deref() == Some(dir)
                        || m.path
                            .as_deref()
                            .map(|p| p.ends_with(dir) || dir.ends_with(p))
                            .unwrap_or(false)
                    {
                        selected_idx = i;
                        break;
                    }
                }
            }
        }
    } else if let Some(ref dir) = cfg.voice.offline.model_dir {
        for (i, m) in local_asrs.iter().enumerate() {
            if m.path.as_deref() == Some(dir)
                || m.path
                    .as_deref()
                    .map(|p| p.ends_with(dir) || dir.ends_with(p))
                    .unwrap_or(false)
            {
                selected_idx = i;
                break;
            }
        }
    }

    window.set_asr_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(all_asr_names),
    )));
    window.set_asr_model_index(selected_idx as i32);
    window.set_asr_notice_text(slint::SharedString::from(notice));
    window.set_is_asr_cloud(is_cloud);

    // 2. VAD 端点检测模型
    let vad_models = waylan_voice::list_vad_models();
    let vad_names: Vec<slint::SharedString> = vad_models
        .iter()
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let mut vad_idx = 0;
    if cfg.voice.vad.vad_type == waylan_core::VadModelType::Energy {
        for (i, m) in vad_models.iter().enumerate() {
            if m.path.as_deref() == Some("__energy__") {
                vad_idx = i;
                break;
            }
        }
    } else if let Some(ref path) = cfg.voice.vad.model_path {
        for (i, m) in vad_models.iter().enumerate() {
            if m.path.as_deref() == Some(path)
                || m.path
                    .as_deref()
                    .map(|p| p.ends_with(path) || path.ends_with(p))
                    .unwrap_or(false)
            {
                vad_idx = i;
                break;
            }
        }
    }
    window.set_vad_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(vad_names),
    )));
    window.set_vad_model_index(vad_idx as i32);

    // 3. 标点符号预测模型
    let punct_models = waylan_voice::list_punct_models();
    let punct_names: Vec<slint::SharedString> = punct_models
        .iter()
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let mut punct_idx = 0;
    if let Some(ref path) = cfg.voice.punct_model_path {
        for (i, m) in punct_models.iter().enumerate() {
            if m.path.as_deref() == Some(path)
                || m.path
                    .as_deref()
                    .map(|p| p.ends_with(path) || path.ends_with(p))
                    .unwrap_or(false)
            {
                punct_idx = i;
                break;
            }
        }
    }
    window.set_punct_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(punct_names),
    )));
    window.set_punct_model_index(punct_idx as i32);

    // 4. 人声智能降噪 (NS) 模型
    let ns_models = waylan_voice::list_ns_models();
    let ns_names: Vec<slint::SharedString> = ns_models
        .iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let mut ns_idx = 0;
    match cfg.voice.afe.ns_engine {
        waylan_core::NsEngineType::Rnnoise => ns_idx = 0,
        waylan_core::NsEngineType::DeepFilterNet => ns_idx = 1,
        waylan_core::NsEngineType::Gtcrn => ns_idx = 2,
        _ => {}
    }
    window.set_ns_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(ns_names),
    )));
    window.set_ns_model_index(ns_idx);
}

/// 从 Slint 界面属性增量读取语音输入配置
fn update_voice_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    cfg.voice.enabled = window.get_voice_enabled();

    // 连续听写开关快捷键 (Toggle)
    let toggle_str = window.get_toggle_shortcut().trim().to_string();
    cfg.voice.toggle_shortcut = if window.get_toggle_shortcut_index() == 5
        || toggle_str.is_empty()
        || toggle_str.eq_ignore_ascii_case("disabled")
        || toggle_str == "已禁用"
    {
        "已禁用".to_string()
    } else {
        toggle_str
    };

    // 长按对讲机快捷键 (Push-to-Talk)
    let ptt_str = window.get_push_to_talk_shortcut().trim().to_string();
    cfg.voice.push_to_talk_shortcut = if window.get_ptt_shortcut_index() == 5
        || ptt_str.is_empty()
        || ptt_str.eq_ignore_ascii_case("disabled")
        || ptt_str == "已禁用"
    {
        "已禁用".to_string()
    } else {
        ptt_str
    };

    // 阶段一 AFE 前端增强
    cfg.voice.afe.enabled = window.get_afe_enabled();
    cfg.voice.afe.high_pass_filter = window.get_high_pass_filter();
    cfg.voice.afe.noise_suppression = window.get_noise_suppression();
    cfg.voice.afe.auto_gain_control = window.get_auto_gain_control();

    // 阶段二 VAD 端点检测截断时间
    cfg.voice.vad.silence_timeout_ms = match window.get_vad_silence_delay_index() {
        0 => 400,
        2 => 800,
        3 => 1000,
        _ => 600,
    };

    // 阶段四 标点预测
    cfg.voice.auto_punctuation = window.get_auto_punctuation();

    // 阶段五 文本规整与语气词过滤 (ITN)
    cfg.voice.itn.convert_numbers = window.get_itn_convert_numbers();
    cfg.voice.itn.filter_filler_words = window.get_itn_filter_filler_words();
    cfg.voice.itn.pangu_spacing = window.get_itn_pangu_spacing();

    // 阶段六 交互展现与流式上屏
    cfg.voice.interaction.real_time_preview = window.get_voice_real_time_preview();
    cfg.voice.interaction.show_floating_bar = window.get_voice_show_floating_bar();

    update_voice_models_from_window(window, cfg);
}

/// 从 Slint 界面属性读取模型选择并持久化至配置
fn update_voice_models_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let local_asrs = waylan_voice::list_asr_models();
    let asr_idx = window.get_asr_model_index() as usize;

    if asr_idx < local_asrs.len() {
        // 选中本地离线 ASR 模型
        cfg.voice.engine_type = waylan_core::VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        if let Some(m) = local_asrs.get(asr_idx) {
            cfg.voice.offline.model_dir = m.path.clone();
        }
    } else {
        // 选中云端 Profile
        let cloud_idx = asr_idx - local_asrs.len();
        if let Some(profile) = cfg.voice.cloud_profiles.get(cloud_idx) {
            cfg.voice.engine_type = waylan_core::VoiceEngineType::Cloud;
            cfg.voice.selected_asr = Some(format!("cloud:{}", profile.id));
            cfg.voice.cloud.endpoint = profile.endpoint.clone();
            cfg.voice.cloud.api_key = profile.api_key.clone();
            cfg.voice.cloud.model = profile.model.clone();
        }
    }

    let vad_models = waylan_voice::list_vad_models();
    let vad_idx = window.get_vad_model_index() as usize;
    if let Some(m) = vad_models.get(vad_idx) {
        if m.path.as_deref() == Some("__energy__") {
            cfg.voice.vad.vad_type = waylan_core::VadModelType::Energy;
            cfg.voice.vad.model_path = None;
        } else {
            cfg.voice.vad.vad_type = waylan_core::VadModelType::SileroOnnx;
            cfg.voice.vad.model_path = m.path.clone();
        }
    }

    let punct_models = waylan_voice::list_punct_models();
    let punct_idx = window.get_punct_model_index() as usize;
    if let Some(m) = punct_models.get(punct_idx) {
        cfg.voice.punct_model_path = m.path.clone();
    }

    let ns_models = waylan_voice::list_ns_models();
    let ns_filtered: Vec<_> = ns_models
        .into_iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .collect();
    let ns_idx = window.get_ns_model_index() as usize;
    if let Some(m) = ns_filtered.get(ns_idx) {
        if m.path.as_deref() == Some("__rnnoise__") {
            cfg.voice.afe.ns_engine = waylan_core::NsEngineType::Rnnoise;
            cfg.voice.afe.custom_ns_model_path = None;
        } else if m.path.as_deref() == Some("__deepfilternet__") {
            cfg.voice.afe.ns_engine = waylan_core::NsEngineType::DeepFilterNet;
            cfg.voice.afe.custom_ns_model_path = None;
        } else if m.path.as_deref() == Some("__gtcrn__") {
            cfg.voice.afe.ns_engine = waylan_core::NsEngineType::Gtcrn;
            cfg.voice.afe.custom_ns_model_path = None;
        }
    }
}

/// 从 Slint 界面属性读取并构造 AppConfig 数据
pub fn extract_config_from_window(window: &SettingsWindow) -> AppConfig {
    let mut cfg = AppConfig::load_or_create();
    update_config_from_window(window, &mut cfg);
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_and_update_preserves_custom_icons() {
        let window = SettingsWindow::new().expect("创建窗口对象应成功");
        let initial_cfg = AppConfig {
            tray_icon_theme_path: Some("/opt/my-theme".to_string()),
            tray_icon_zh: Some("custom-zh.svg".to_string()),
            tray_icon_en: Some("custom-en.svg".to_string()),
            enable_tray: Some(true),
            exclusive_input_source: true,
            enable_u_mode: false,
            enable_i_mode: false,
            filter_uncommon_charset: true,
            show_pinyin_comment: false,
            auto_dedup: false,
            ..Default::default()
        };

        sync_config_to_window(&window, &initial_cfg);

        // 验证界面读取与同步状态（Some(true) + true 对应索引 0: 微澜专属彩色托盘）
        assert_eq!(window.get_enable_tray_index(), 0);
        assert!(!window.get_enable_u_mode());
        assert!(!window.get_enable_i_mode());
        assert!(window.get_filter_uncommon_charset());
        assert!(!window.get_show_pinyin_comment());
        assert!(!window.get_auto_dedup());

        assert_eq!(window.get_traditional_shortcut_index(), 0);
        assert_eq!(window.get_full_width_char_shortcut_index(), 0);
        assert_eq!(window.get_full_width_punct_shortcut_index(), 0);

        // 模拟用户在界面修改某项（切为索引 1: GNOME 原生单图标，索引 2: 圆圈序号，修改快捷键）
        assert_eq!(window.get_index_style_index(), 0);
        window.set_page_size_val(8.0);
        window.set_enable_tray_index(1);
        window.set_index_style_index(2);
        window.set_traditional_shortcut_index(1); // CtrlAltF
        window.set_full_width_char_shortcut_index(1); // CtrlShiftSpace
        window.set_full_width_punct_shortcut_index(1); // AltPeriod

        let mut updated_cfg = initial_cfg.clone();
        update_config_from_window(&window, &mut updated_cfg);

        // 验证修改生效
        assert_eq!(updated_cfg.page_size, 8);
        assert_eq!(
            updated_cfg.candidate_index_style,
            CandidateIndexStyle::Circled
        );
        assert_eq!(updated_cfg.enable_tray, Some(false));
        assert!(!updated_cfg.exclusive_input_source);
        assert_eq!(
            updated_cfg.traditional_shortcut,
            TraditionalShortcut::CtrlAltF
        );
        assert_eq!(
            updated_cfg.full_width_char_shortcut,
            FullWidthCharShortcut::CtrlShiftSpace
        );
        assert_eq!(
            updated_cfg.full_width_punct_shortcut,
            FullWidthPunctShortcut::AltPeriod
        );
        // 关键：未在界面展示的高级图标配置绝对不可被清空
        assert_eq!(
            updated_cfg.tray_icon_theme_path.as_deref(),
            Some("/opt/my-theme")
        );
        assert_eq!(updated_cfg.tray_icon_zh.as_deref(), Some("custom-zh.svg"));
        assert_eq!(updated_cfg.tray_icon_en.as_deref(), Some("custom-en.svg"));

        // 验证语音双独立快捷键初始同步
        assert_eq!(window.get_toggle_shortcut().as_str(), "Ctrl + Alt + V");
        assert_eq!(window.get_toggle_shortcut_index(), 0);
        assert_eq!(window.get_push_to_talk_shortcut().as_str(), "F8");
        assert_eq!(window.get_ptt_shortcut_index(), 0);

        // 模拟用户在 UI 界面自定义设置：切换长按对讲机为 Super + V，禁用连续听写
        window.set_ptt_shortcut_index(3);
        window.set_push_to_talk_shortcut("Super + V".into());
        window.set_toggle_shortcut_index(5);
        window.set_toggle_shortcut("已禁用".into());

        update_config_from_window(&window, &mut updated_cfg);

        // 验证语音快捷键修改并持久化成功
        assert_eq!(updated_cfg.voice.push_to_talk_shortcut, "Super + V");
        assert_eq!(updated_cfg.voice.toggle_shortcut, "已禁用");
        assert!(updated_cfg.voice.get_effective_ptt_shortcut().is_some());
        assert!(updated_cfg.voice.get_effective_toggle_shortcut().is_none());

        // 验证语音流水线与云端 Profile 双向同步与模型切换
        let mut voice_pipeline_cfg = updated_cfg.clone();
        voice_pipeline_cfg.voice.afe.enabled = true;
        voice_pipeline_cfg.voice.afe.high_pass_filter = false;
        voice_pipeline_cfg.voice.afe.noise_suppression = true;
        voice_pipeline_cfg.voice.afe.auto_gain_control = false;
        voice_pipeline_cfg.voice.vad.silence_timeout_ms = 800;
        voice_pipeline_cfg.voice.auto_punctuation = false;
        voice_pipeline_cfg.voice.cloud_profiles = vec![waylan_core::CloudProfile {
            id: "test-openai".to_string(),
            name: "测试 OpenAI".to_string(),
            provider: waylan_core::CloudProviderType::OpenAiCompatible,
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key: Some("sk-test12345678".to_string()),
            model: "whisper-1".to_string(),
        }];
        voice_pipeline_cfg.voice.selected_asr = Some("cloud:test-openai".to_string());
        voice_pipeline_cfg.voice.engine_type = waylan_core::VoiceEngineType::Cloud;

        sync_config_to_window(&window, &voice_pipeline_cfg);

        // 验证初始同步到窗口属性
        assert!(window.get_afe_enabled());
        assert!(!window.get_high_pass_filter());
        assert!(window.get_noise_suppression());
        assert!(!window.get_auto_gain_control());
        assert_eq!(window.get_vad_silence_delay_index(), 2); // 800ms 对应索引 2
        assert!(!window.get_auto_punctuation());
        assert!(window.get_is_asr_cloud());
        assert!(window.get_itn_convert_numbers());
        assert!(window.get_itn_filter_filler_words());
        assert!(window.get_itn_pangu_spacing());
        assert!(window.get_voice_real_time_preview());
        assert!(window.get_voice_show_floating_bar());

        // 模拟用户在流水线界面修改：开启自动增益、切换 400ms 停顿、开启标点、切回本地 ASR (索引 0)、关闭语气词过滤、关闭波形条
        window.set_auto_gain_control(true);
        window.set_vad_silence_delay_index(0); // 400ms
        window.set_auto_punctuation(true);
        window.set_asr_model_index(0); // 本地模型
        window.set_itn_filter_filler_words(false);
        window.set_voice_show_floating_bar(false);

        let mut roundtrip_cfg = voice_pipeline_cfg.clone();
        update_config_from_window(&window, &mut roundtrip_cfg);

        // 验证回写持久化配置
        assert!(roundtrip_cfg.voice.afe.auto_gain_control);
        assert_eq!(roundtrip_cfg.voice.vad.silence_timeout_ms, 400);
        assert!(roundtrip_cfg.voice.auto_punctuation);
        assert!(!roundtrip_cfg.voice.itn.filter_filler_words);
        assert!(roundtrip_cfg.voice.itn.convert_numbers);
        assert!(roundtrip_cfg.voice.itn.pangu_spacing);
        assert!(roundtrip_cfg.voice.interaction.real_time_preview);
        assert!(!roundtrip_cfg.voice.interaction.show_floating_bar);
        assert_eq!(
            roundtrip_cfg.voice.engine_type,
            waylan_core::VoiceEngineType::Offline
        );
        assert!(roundtrip_cfg.voice.selected_asr.is_none());
        assert_eq!(roundtrip_cfg.voice.cloud_profiles.len(), 1);
        assert_eq!(roundtrip_cfg.voice.cloud_profiles[0].name, "测试 OpenAI");
    }
}
