//! 常规输入模式、快捷键、候选排版与主题状态双向同步模块

use crate::SettingsWindow;
use waylan_core::config::{
    AppConfig, CandidateDensity, CandidateIndexStyle, CandidateOrientation, CandidateThemePreset,
    CandidateWindowMode, FullWidthCharShortcut, FullWidthPunctShortcut, FuzzyPinyinRules, PageKeys,
    SwitchKey, TraditionalShortcut, TypoCorrectionRules,
};

/// 同步常规输入、按键习惯与外观排版属性至 Slint 界面属性
pub(super) fn sync_general_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    // 初始状态
    window.set_init_lang_index(if cfg.init_chinese { 0 } else { 1 });
    window.set_init_char_width_index(if cfg.full_width_char { 1 } else { 0 });
    window.set_init_punct_width_index(if cfg.full_width_punct { 0 } else { 1 });
    window.set_init_simp_trad_index(if cfg.traditional_chinese { 1 } else { 0 });

    // 候选框外观与渲染模式
    window.set_window_mode_index(match cfg.candidate_window_mode {
        CandidateWindowMode::WaylanCustom => 0,
        CandidateWindowMode::Native => 1,
    });
    window.set_density_index(match cfg.candidate_density {
        CandidateDensity::Compact => 0,
        CandidateDensity::Normal => 1,
        CandidateDensity::Spacious => 2,
    });
    window.set_candidate_item_spacing(cfg.candidate_item_spacing);
    window.set_candidate_padding_v(cfg.candidate_padding_v);
    window.set_candidate_padding_h(cfg.candidate_padding_h);
    window.set_candidate_corner_radius(cfg.candidate_corner_radius);
    window.set_candidate_opacity(cfg.candidate_opacity as f32);
    window.set_theme_preset_index(match cfg.candidate_theme {
        CandidateThemePreset::Emerald => 0,
        CandidateThemePreset::Monochrome => 1,
        CandidateThemePreset::IceBlue => 2,
        CandidateThemePreset::WarmLatte => 3,
    });
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
}

/// 从 Slint 界面属性增量读取常规输入、按键习惯与外观排版并更新至 AppConfig
pub(super) fn update_general_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
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
    cfg.candidate_window_mode = match window.get_window_mode_index() {
        1 => CandidateWindowMode::Native,
        _ => CandidateWindowMode::WaylanCustom,
    };
    cfg.candidate_density = match window.get_density_index() {
        0 => CandidateDensity::Compact,
        2 => CandidateDensity::Spacious,
        _ => CandidateDensity::Normal,
    };
    cfg.candidate_theme = match window.get_theme_preset_index() {
        1 => CandidateThemePreset::Monochrome,
        2 => CandidateThemePreset::IceBlue,
        3 => CandidateThemePreset::WarmLatte,
        _ => CandidateThemePreset::Emerald,
    };
    cfg.candidate_item_spacing = window.get_candidate_item_spacing();
    cfg.candidate_padding_v = window.get_candidate_padding_v();
    cfg.candidate_padding_h = window.get_candidate_padding_h();
    cfg.candidate_corner_radius = window.get_candidate_corner_radius();
    cfg.candidate_opacity = window.get_candidate_opacity().round() as u32;
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
}
