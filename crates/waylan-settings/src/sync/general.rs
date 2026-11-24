//! 常规输入模式、快捷键、候选排版与主题状态双向同步模块

use crate::SettingsWindow;
use waylan_core::config::{
    AppConfig, CandidateDensity, CandidateIndexStyle, CandidateOrientation, CandidateThemePreset,
    CandidateWindowMode, DesktopConfig, FuzzyPinyinRules, PageKeys, PinyinConfig, ShortcutConfig,
    SwitchKey, TrayDisplayMode, UiConfig, UiIndexEnum,
};

/// 同步常规输入、按键习惯与外观排版属性至 Slint 界面属性
pub(super) fn sync_general_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_pinyin_basic(window, &cfg.pinyin);
    sync_ui_appearance(window, &cfg.ui);
    sync_shortcuts(window, &cfg.shortcuts);
    sync_pinyin_rules(window, &cfg.pinyin);
    sync_fuzzy_rules(window, &cfg.pinyin.fuzzy_rules, cfg.pinyin.fuzzy_pinyin);
    sync_desktop_settings(window, &cfg.desktop);
}

/// 从 Slint 界面属性增量读取常规输入、按键习惯与外观排版并更新至 AppConfig
pub(super) fn update_general_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    update_pinyin_basic(window, &mut cfg.pinyin);
    update_shortcuts(window, &mut cfg.shortcuts);
    update_ui_appearance(window, &mut cfg.ui);
    update_pinyin_rules(window, &mut cfg.pinyin);
    update_fuzzy_rules(window, &mut cfg.pinyin);
    update_desktop_settings(window, &mut cfg.desktop);
}

/// 同步拼音基础模式与字宽标点状态
fn sync_pinyin_basic(window: &SettingsWindow, pinyin: &PinyinConfig) {
    window.set_init_lang_index(if pinyin.init_chinese { 0 } else { 1 });
    window.set_init_char_width_index(if pinyin.full_width_char { 1 } else { 0 });
    window.set_init_punct_width_index(if pinyin.full_width_punct { 0 } else { 1 });
    window.set_init_simp_trad_index(if pinyin.traditional_chinese { 1 } else { 0 });
}

/// 从界面更新拼音基础模式与字宽标点状态
fn update_pinyin_basic(window: &SettingsWindow, pinyin: &mut PinyinConfig) {
    pinyin.init_chinese = window.get_init_lang_index() == 0;
    pinyin.full_width_char = window.get_init_char_width_index() == 1;
    pinyin.full_width_punct = window.get_init_punct_width_index() == 0;
    pinyin.traditional_chinese = window.get_init_simp_trad_index() == 1;
}

/// 同步快捷键与翻页按键
fn sync_shortcuts(window: &SettingsWindow, shortcuts: &ShortcutConfig) {
    window.set_switch_key_index(shortcuts.switch_key.as_ui_index());
    window.set_page_keys_minus_equal(shortcuts.page_keys.minus_equal);
    window.set_page_keys_bracket(shortcuts.page_keys.bracket);
    window.set_page_keys_comma_period(shortcuts.page_keys.comma_period);
    window.set_traditional_shortcut(slint::SharedString::from(
        shortcuts.traditional_shortcut.as_str(),
    ));
    window.set_full_width_char_shortcut(slint::SharedString::from(
        shortcuts.full_width_char_shortcut.as_str(),
    ));
    window.set_full_width_punct_shortcut(slint::SharedString::from(
        shortcuts.full_width_punct_shortcut.as_str(),
    ));
    window.set_shift_select_second(shortcuts.shift_select_second);
}

/// 从界面更新快捷键与翻页按键
fn update_shortcuts(window: &SettingsWindow, shortcuts: &mut ShortcutConfig) {
    shortcuts.switch_key = SwitchKey::from_ui_index(window.get_switch_key_index());
    shortcuts.page_keys = PageKeys {
        minus_equal: window.get_page_keys_minus_equal(),
        bracket: window.get_page_keys_bracket(),
        comma_period: window.get_page_keys_comma_period(),
    };
    shortcuts.traditional_shortcut = sanitize_shortcut_string(window.get_traditional_shortcut());
    shortcuts.full_width_char_shortcut =
        sanitize_shortcut_string(window.get_full_width_char_shortcut());
    shortcuts.full_width_punct_shortcut =
        sanitize_shortcut_string(window.get_full_width_punct_shortcut());
    shortcuts.shift_select_second = window.get_shift_select_second();
}

/// 快捷键字符串规整处理（统一将空值与 `disabled` 归一化为「已禁用」）
fn sanitize_shortcut_string(val: slint::SharedString) -> String {
    let s = val.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("disabled") || s == "已禁用" {
        "已禁用".to_string()
    } else {
        s.to_string()
    }
}

/// 同步候选框外观排版与主题渲染参数
fn sync_ui_appearance(window: &SettingsWindow, ui: &UiConfig) {
    window.set_window_mode_index(ui.candidate_window_mode.as_ui_index());
    window.set_candidate_freeze_during_composition(ui.candidate_freeze_during_composition);
    window.set_density_index(ui.candidate_density.as_ui_index());
    window.set_candidate_item_spacing(ui.candidate_item_spacing);
    window.set_candidate_padding_v(ui.candidate_padding_v);
    window.set_candidate_padding_h(ui.candidate_padding_h);
    window.set_candidate_corner_radius(ui.candidate_corner_radius);
    window.set_candidate_opacity(ui.candidate_opacity as f32);
    window.set_theme_preset_index(ui.candidate_theme.as_ui_index());
    window.set_page_size_val(ui.page_size as f32);
    window.set_font_size_val(ui.font_size);
    window.set_orientation_index(ui.candidate_orientation.as_ui_index());
    window.set_index_style_index(ui.candidate_index_style.as_ui_index());
    window.set_show_pinyin_comment(ui.show_pinyin_comment);
}

/// 从界面更新候选框外观排版与主题渲染参数
fn update_ui_appearance(window: &SettingsWindow, ui: &mut UiConfig) {
    ui.candidate_window_mode = CandidateWindowMode::from_ui_index(window.get_window_mode_index());
    ui.candidate_freeze_during_composition = window.get_candidate_freeze_during_composition();
    ui.candidate_density = CandidateDensity::from_ui_index(window.get_density_index());
    ui.candidate_theme = CandidateThemePreset::from_ui_index(window.get_theme_preset_index());
    ui.candidate_item_spacing = window.get_candidate_item_spacing();
    ui.candidate_padding_v = window.get_candidate_padding_v();
    ui.candidate_padding_h = window.get_candidate_padding_h();
    ui.candidate_corner_radius = window.get_candidate_corner_radius();
    ui.candidate_opacity = window.get_candidate_opacity().round() as u32;
    ui.page_size = window.get_page_size_val().round() as usize;
    ui.font_size = window.get_font_size_val().round();
    ui.candidate_orientation = CandidateOrientation::from_ui_index(window.get_orientation_index());
    ui.candidate_index_style = CandidateIndexStyle::from_ui_index(window.get_index_style_index());
    ui.show_pinyin_comment = window.get_show_pinyin_comment();
}

/// 同步拼音输入特性与纠错规则
fn sync_pinyin_rules(window: &SettingsWindow, pinyin: &PinyinConfig) {
    window.set_dynamic_adjust_freq(pinyin.dynamic_adjust_freq);
    window.set_auto_pangu_spacing(pinyin.auto_pangu_spacing);
    window.set_v_mode(pinyin.v_mode);
    window.set_slash_to_dunhao(pinyin.slash_to_dunhao);
    window.set_backslash_to_dunhao(pinyin.backslash_to_dunhao);
    window.set_protect_digits_punctuation(pinyin.protect_digits_punctuation);
    window.set_auto_complete_pairs(pinyin.auto_complete_pairs);
    window.set_enable_acronym(pinyin.enable_acronym);
    window.set_enable_u_mode(pinyin.enable_u_mode);
    window.set_enable_i_mode(pinyin.enable_i_mode);
    window.set_filter_uncommon_charset(pinyin.filter_uncommon_charset);
    window.set_auto_dedup(pinyin.auto_dedup);

    // 拼音纠错：界面仅暴露总闸，细分改写规则由 config.toml 承载
    window.set_typo_enable(pinyin.typo_rules.enable);
}

/// 从界面更新拼音输入特性与纠错规则
fn update_pinyin_rules(window: &SettingsWindow, pinyin: &mut PinyinConfig) {
    pinyin.dynamic_adjust_freq = window.get_dynamic_adjust_freq();
    pinyin.enable_acronym = window.get_enable_acronym();
    pinyin.slash_to_dunhao = window.get_slash_to_dunhao();
    pinyin.backslash_to_dunhao = window.get_backslash_to_dunhao();
    pinyin.protect_digits_punctuation = window.get_protect_digits_punctuation();
    pinyin.auto_complete_pairs = window.get_auto_complete_pairs();
    pinyin.v_mode = window.get_v_mode();
    pinyin.auto_pangu_spacing = window.get_auto_pangu_spacing();
    pinyin.enable_u_mode = window.get_enable_u_mode();
    pinyin.enable_i_mode = window.get_enable_i_mode();
    pinyin.filter_uncommon_charset = window.get_filter_uncommon_charset();
    pinyin.auto_dedup = window.get_auto_dedup();

    // 仅回写总闸：细分规则不在界面呈现，必须保留磁盘加载值，避免保存时被默认值覆盖
    pinyin.typo_rules.enable = window.get_typo_enable();
}

/// 同步模糊音开关与各细分规则
fn sync_fuzzy_rules(window: &SettingsWindow, rules: &FuzzyPinyinRules, enabled: bool) {
    window.set_fuzzy_pinyin(enabled);
    window.set_fuzzy_c_ch(rules.c_ch);
    window.set_fuzzy_z_zh(rules.z_zh);
    window.set_fuzzy_s_sh(rules.s_sh);
    window.set_fuzzy_an_ang(rules.an_ang);
    window.set_fuzzy_en_eng(rules.en_eng);
    window.set_fuzzy_in_ing(rules.in_ing);
    window.set_fuzzy_l_n(rules.l_n);
    window.set_fuzzy_f_h(rules.f_h);
    window.set_fuzzy_r_l(rules.r_l);
    window.set_fuzzy_g_k(rules.g_k);
}

/// 从界面更新模糊音开关与各细分规则
fn update_fuzzy_rules(window: &SettingsWindow, pinyin: &mut PinyinConfig) {
    pinyin.fuzzy_pinyin = window.get_fuzzy_pinyin();
    pinyin.fuzzy_rules = FuzzyPinyinRules {
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
}

/// 同步桌面托盘与指示器模式
fn sync_desktop_settings(window: &SettingsWindow, desktop: &DesktopConfig) {
    window.set_enable_tray_index(
        match (desktop.enable_tray, desktop.exclusive_input_source) {
            (TrayDisplayMode::Never, _) => 1,      // GNOME 原生单图标
            (TrayDisplayMode::Always, false) => 2, // 双指示器并存
            _ => 0,                                // 微澜专属彩色托盘（推荐）
        },
    );
    window.set_window_mode_memory(desktop.window_mode_memory);
    window.set_terminal_default_english(desktop.terminal_default_english);
}

/// 从界面更新桌面托盘与指示器模式
fn update_desktop_settings(window: &SettingsWindow, desktop: &mut DesktopConfig) {
    match window.get_enable_tray_index() {
        1 => {
            desktop.enable_tray = TrayDisplayMode::Never;
            desktop.exclusive_input_source = false;
        }
        2 => {
            desktop.enable_tray = TrayDisplayMode::Always;
            desktop.exclusive_input_source = false;
        }
        _ => {
            desktop.enable_tray = TrayDisplayMode::Always;
            desktop.exclusive_input_source = true;
        }
    }
    desktop.window_mode_memory = window.get_window_mode_memory();
    desktop.terminal_default_english = window.get_terminal_default_english();
}
