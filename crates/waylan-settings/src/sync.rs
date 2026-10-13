//! 配置模型与 Slint 窗口属性双向数据同步模块
//!
//! # 设计原理
//! - **实现初衷**：解耦界面渲染与核心配置领域模型，集中管理 GUI 属性与持久化配置的映射。
//! - **核心优势**：
//!   1. 集中防守，对索引越界、异常数值进行安全边界校验与降级；
//!   2. 避免在事件回调中分散编写冗长脆弱的属性映射逻辑；
//!   3. 支持无缝双向转换，便于单元测试与自动化验证。

use crate::SettingsWindow;
use waylan_core::config::{
    AppConfig, CandidateOrientation, FuzzyPinyinRules, PageKeys, SwitchKey, ThemeMode,
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
    window.set_theme_index(match cfg.theme {
        ThemeMode::Light => 0,
        ThemeMode::Dark => 1,
    });

    // 按键习惯
    window.set_switch_key_index(match cfg.switch_key {
        SwitchKey::Shift => 0,
        SwitchKey::Ctrl => 1,
    });
    window.set_page_keys_minus_equal(cfg.page_keys.minus_equal);
    window.set_page_keys_bracket(cfg.page_keys.bracket);
    window.set_page_keys_comma_period(cfg.page_keys.comma_period);

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
    window.set_enable_tray_index(match cfg.enable_tray {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    });
    window.set_exclusive_input_source(cfg.exclusive_input_source);

    // 高级模式与候选特性
    window.set_enable_u_mode(cfg.enable_u_mode);
    window.set_enable_i_mode(cfg.enable_i_mode);
    window.set_filter_uncommon_charset(cfg.filter_uncommon_charset);
    window.set_show_pinyin_comment(cfg.show_pinyin_comment);
    window.set_auto_dedup(cfg.auto_dedup);
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
    cfg.page_size = window.get_page_size_val().round() as usize;
    cfg.font_size = window.get_font_size_val().round();
    cfg.theme = match window.get_theme_index() {
        1 => ThemeMode::Dark,
        _ => ThemeMode::Light,
    };
    cfg.candidate_orientation = match window.get_orientation_index() {
        1 => CandidateOrientation::Vertical,
        _ => CandidateOrientation::Horizontal,
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
    cfg.enable_tray = match window.get_enable_tray_index() {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    };
    cfg.exclusive_input_source = window.get_exclusive_input_source();
    cfg.enable_u_mode = window.get_enable_u_mode();
    cfg.enable_i_mode = window.get_enable_i_mode();
    cfg.filter_uncommon_charset = window.get_filter_uncommon_charset();
    cfg.show_pinyin_comment = window.get_show_pinyin_comment();
    cfg.auto_dedup = window.get_auto_dedup();
    cfg.sanitize();
}

/// 从 Slint 界面属性读取并构造 AppConfig 数据
pub fn extract_config_from_window(window: &SettingsWindow) -> AppConfig {
    let mut cfg = AppConfig::load_or_create();
    update_config_from_window(window, &mut cfg);
    cfg
}
