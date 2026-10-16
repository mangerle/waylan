//! 微澜输入法全局配置系统模块
//!
//! 负责整合领域错误枚举、路径探测、数据校验与原子持久化存储。

mod error;
mod paths;
mod storage;

use std::path::PathBuf;

pub use error::ConfigError;
pub use paths::{
    aux_dict_search_paths, base_dict_search_paths, config_path, env_var_non_empty,
    fallback_runtime_dir, opencc_dict_search_paths, user_aux_dict_path, user_dict_path,
};
use serde::{Deserialize, Serialize};

pub use crate::config_types::{
    AudioFrontEndConfig, CandidateDensity, CandidateIndexStyle, CandidateOrientation,
    CandidateThemePreset, CandidateWindowMode, CloudAsrConfig, CloudProfile, CloudProviderType,
    FullWidthCharShortcut, FullWidthPunctShortcut, FuzzyPinyinRules, NsEngineType,
    OfflineAsrConfig, OfflineModelType, PageKeys, ParsedShortcut, SwitchKey, ThemeMode,
    TraditionalShortcut, TypoCorrectionRules, VadConfig, VadModelType, VoiceConfig,
    VoiceEngineType, VoiceInteractionConfig, VoiceItnConfig, VoiceVisualEffect,
};

/// 微澜输入法全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 初始输入语言模式是否为中文（true: 中文，false: 英文）
    pub init_chinese: bool,
    /// 字母/数字全角或半角状态（true: 全角，false: 半角）
    pub full_width_char: bool,
    /// 标点符号全角或半角状态（true: 全角中文标点，false: 半角英文标点）
    pub full_width_punct: bool,
    /// 中英文切换按键
    pub switch_key: SwitchKey,
    /// 翻页快捷键
    pub page_keys: PageKeys,
    /// 简繁体即时切换快捷键
    pub traditional_shortcut: TraditionalShortcut,
    /// 字符全角/半角切换快捷键
    pub full_width_char_shortcut: FullWidthCharShortcut,
    /// 标点全角/半角切换快捷键
    pub full_width_punct_shortcut: FullWidthPunctShortcut,
    /// 单页候选词数量（默认 5）
    pub page_size: usize,
    /// 候选文字大小（像素）
    pub font_size: f32,
    /// 界面主题模式
    pub theme: ThemeMode,
    /// 候选词排列方向（水平 / 垂直）
    pub candidate_orientation: CandidateOrientation,
    /// 候选词序号展示风格（默认现代空白风）
    pub candidate_index_style: CandidateIndexStyle,
    /// 候选框渲染模式（微澜现代专属 / 桌面原生系统）
    pub candidate_window_mode: CandidateWindowMode,
    /// 候选框排版留白密度（紧凑 / 标准 / 宽松）
    pub candidate_density: CandidateDensity,
    /// 候选框色彩主题预设（微澜翡翠绿 / 极简黑白 / 极客暗夜冰蓝 / 暖阳奶茶）
    pub candidate_theme: CandidateThemePreset,
    /// 候选词项间距（像素，范围 2.0 ~ 24.0，默认 8.0）
    #[serde(default = "default_candidate_item_spacing")]
    pub candidate_item_spacing: f32,
    /// 候选框上下内边距（像素，范围 2.0 ~ 16.0，默认 4.0）
    #[serde(default = "default_candidate_padding_v")]
    pub candidate_padding_v: f32,
    /// 候选框左右内边距（像素，范围 4.0 ~ 24.0，默认 8.0）
    #[serde(default = "default_candidate_padding_h")]
    pub candidate_padding_h: f32,
    /// 候选框圆角曲率半径（像素，范围 0.0 ~ 20.0，默认 8.0）
    #[serde(default = "default_candidate_corner_radius")]
    pub candidate_corner_radius: f32,
    /// 候选框背景不透明度百分比（范围 70 ~ 100，默认 95）
    #[serde(default = "default_candidate_opacity")]
    pub candidate_opacity: u32,
    /// 是否动态调整候选词序（自适应词频学习）
    pub dynamic_adjust_freq: bool,
    /// 是否启用简拼（声母缩写检索）
    pub enable_acronym: bool,
    /// 是否允许使用 Shift 键选择次选词
    pub shift_select_second: bool,
    /// 是否开启斜杠转顿号（将 /? 替换为 、）
    pub slash_to_dunhao: bool,
    /// 是否开启数字间英文标点保护（如 12:00、3.14）
    pub protect_digits_punctuation: bool,
    /// 是否开启成对符号自动补全
    pub auto_complete_pairs: bool,
    /// 是否启用 V 模式（计算器与快捷符号）
    pub v_mode: bool,
    /// 是否启用模糊拼音主开关
    pub fuzzy_pinyin: bool,
    /// 模糊拼音 10 项细分规则配置
    pub fuzzy_rules: FuzzyPinyinRules,
    /// 拼音纠错主开关与细分规则配置
    pub typo_rules: TypoCorrectionRules,
    /// 是否默认输出繁体字
    pub traditional_chinese: bool,
    /// 是否开启中西文智能空格（盘古之白，自动在中英文与数字交界处插入半角空格）
    pub auto_pangu_spacing: bool,
    /// 是否启用 U 模式（拆字与笔画反查）
    pub enable_u_mode: bool,
    /// 是否启用 I 模式（特殊符号与序号选择）
    pub enable_i_mode: bool,
    /// 是否过滤超出常用规范汉字范围的生僻字符
    pub filter_uncommon_charset: bool,
    /// 是否在候选词旁边附加拼音/注释提示
    pub show_pinyin_comment: bool,
    /// 是否开启多路候选词自动去重
    pub auto_dedup: bool,
    /// 自定义托盘图标主题目录或图标所在目录路径
    pub tray_icon_theme_path: Option<String>,
    /// 自定义中文状态托盘图标名称或路径
    pub tray_icon_zh: Option<String>,
    /// 自定义英文状态托盘图标名称或路径
    pub tray_icon_en: Option<String>,
    /// 独立系统托盘显示模式（None=自动按桌面环境判定，Some(true)=强制启用专属彩色托盘，Some(false)=强制关闭独立托盘）
    pub enable_tray: Option<bool>,
    /// 是否将微澜设为系统唯一独占输入源（用于单输入源自动隐藏 GNOME 原生顶栏指示器）
    pub exclusive_input_source: bool,
    /// 语音输入功能配置
    pub voice: VoiceConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            init_chinese: true,
            full_width_char: false,
            full_width_punct: true,
            switch_key: SwitchKey::Shift,
            page_keys: PageKeys::default(),
            traditional_shortcut: TraditionalShortcut::default(),
            full_width_char_shortcut: FullWidthCharShortcut::default(),
            full_width_punct_shortcut: FullWidthPunctShortcut::default(),
            page_size: 5,
            font_size: 15.0,
            theme: ThemeMode::Light,
            candidate_orientation: CandidateOrientation::Horizontal,
            candidate_index_style: CandidateIndexStyle::ModernSpace,
            candidate_window_mode: CandidateWindowMode::default(),
            candidate_density: CandidateDensity::default(),
            candidate_theme: CandidateThemePreset::default(),
            candidate_item_spacing: default_candidate_item_spacing(),
            candidate_padding_v: default_candidate_padding_v(),
            candidate_padding_h: default_candidate_padding_h(),
            candidate_corner_radius: default_candidate_corner_radius(),
            candidate_opacity: default_candidate_opacity(),
            dynamic_adjust_freq: true,
            enable_acronym: true,
            shift_select_second: false,
            slash_to_dunhao: true,
            protect_digits_punctuation: true,
            auto_complete_pairs: true,
            v_mode: true,
            fuzzy_pinyin: false,
            fuzzy_rules: FuzzyPinyinRules::default(),
            typo_rules: TypoCorrectionRules::default(),
            traditional_chinese: false,
            auto_pangu_spacing: false,
            enable_u_mode: true,
            enable_i_mode: true,
            filter_uncommon_charset: false,
            show_pinyin_comment: true,
            auto_dedup: true,
            tray_icon_theme_path: None,
            tray_icon_zh: None,
            tray_icon_en: None,
            enable_tray: None,
            exclusive_input_source: false,
            voice: VoiceConfig::default(),
        }
    }
}

fn default_candidate_item_spacing() -> f32 {
    8.0
}

fn default_candidate_padding_v() -> f32 {
    4.0
}

fn default_candidate_padding_h() -> f32 {
    8.0
}

fn default_candidate_corner_radius() -> f32 {
    8.0
}

fn default_candidate_opacity() -> u32 {
    95
}

impl AppConfig {
    /// 校验并钳制配置参数边界，防止手工修改出非法数值引发除零或渲染异常
    pub fn sanitize(&mut self) {
        self.page_size = self.page_size.clamp(5, 9);
        if !self.font_size.is_finite() {
            self.font_size = 15.0;
        } else {
            self.font_size = self.font_size.clamp(12.0, 24.0);
        }
        if !self.candidate_item_spacing.is_finite() {
            self.candidate_item_spacing = default_candidate_item_spacing();
        } else {
            self.candidate_item_spacing = self.candidate_item_spacing.clamp(2.0, 24.0);
        }
        if !self.candidate_padding_v.is_finite() {
            self.candidate_padding_v = default_candidate_padding_v();
        } else {
            self.candidate_padding_v = self.candidate_padding_v.clamp(2.0, 16.0);
        }
        if !self.candidate_padding_h.is_finite() {
            self.candidate_padding_h = default_candidate_padding_h();
        } else {
            self.candidate_padding_h = self.candidate_padding_h.clamp(4.0, 24.0);
        }
        if !self.candidate_corner_radius.is_finite() {
            self.candidate_corner_radius = default_candidate_corner_radius();
        } else {
            self.candidate_corner_radius = self.candidate_corner_radius.clamp(0.0, 20.0);
        }
        self.candidate_opacity = self.candidate_opacity.clamp(70, 100);
        self.voice.vad.silence_timeout_ms = self.voice.vad.silence_timeout_ms.clamp(200, 5000);
        self.voice.vad.speech_threshold = self.voice.vad.speech_threshold.clamp(0.01, 0.99);
    }

    /// 获取无 HOME 环境下的受控运行时回退目录
    #[inline]
    pub fn fallback_runtime_dir() -> PathBuf {
        paths::fallback_runtime_dir()
    }

    /// 获取用户默认配置路径 (~/.config/waylan/config.toml)
    #[inline]
    pub fn config_path() -> Option<PathBuf> {
        paths::config_path()
    }

    /// 获取符合 XDG 规范的用户词库文件存储路径 (~/.local/share/waylan/user_dict.bin)
    #[inline]
    pub fn user_dict_path() -> PathBuf {
        paths::user_dict_path()
    }

    /// 获取默认系统基础词库的多级查找路径（环境变量 -> 用户数据目录 -> 系统公共目录 -> 相对路径）
    #[inline]
    pub fn base_dict_search_paths() -> Vec<PathBuf> {
        paths::base_dict_search_paths()
    }

    /// 获取符合 XDG 规范的用户辅助与混输词库存储路径 (~/.local/share/waylan/aux.dict.bin)
    #[inline]
    pub fn user_aux_dict_path() -> PathBuf {
        paths::user_aux_dict_path()
    }

    /// 获取辅助与混输词库（英文与符号表情）的多级查找路径
    #[inline]
    pub fn aux_dict_search_paths() -> Vec<PathBuf> {
        paths::aux_dict_search_paths()
    }

    /// 获取简繁转换词库（STPhrases + STCharacters）的多级查找路径
    #[inline]
    pub fn opencc_dict_search_paths() -> Vec<PathBuf> {
        paths::opencc_dict_search_paths()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    #[test]
    fn test_config_serialization() {
        let default_cfg = AppConfig::default();
        let serialized = toml::to_string_pretty(&default_cfg).expect("序列化应成功");
        assert!(serialized.contains("slash_to_dunhao = true"));
        assert!(serialized.contains("font_size = 15.0"));

        let deserialized: AppConfig = toml::from_str(&serialized).expect("反序列化应成功");
        assert_eq!(deserialized.page_size, 5);
        assert_eq!(
            deserialized.candidate_index_style,
            CandidateIndexStyle::ModernSpace
        );
        assert_eq!(deserialized.switch_key, SwitchKey::Shift);
        assert_eq!(deserialized.page_keys, PageKeys::default());
        assert!(!deserialized.auto_pangu_spacing);
        assert!(deserialized.enable_u_mode);
        assert!(deserialized.enable_i_mode);
        assert!(!deserialized.filter_uncommon_charset);
        assert!(deserialized.show_pinyin_comment);
        assert!(deserialized.auto_dedup);

        // 测试临时文件保存（使用 tempfile::tempdir() 确保多线程并发测试隔离与 RAII 自动清理）
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let temp_file = temp_dir.path().join("test_config.toml");
        default_cfg
            .save_to_file(&temp_file)
            .expect("应能保存至文件");
        assert!(temp_file.exists());
    }

    #[test]
    fn test_fallback_runtime_dir_is_absolute() {
        let fallback = AppConfig::fallback_runtime_dir();
        assert!(
            fallback.is_absolute(),
            "无 HOME 回退目录必须为绝对路径，避免工作目录只读导致落盘失败"
        );
        let name = fallback.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with("waylan"));
    }

    #[test]
    fn test_env_var_non_empty_filtering() {
        assert_eq!(
            env_var_non_empty("THIS_VARIABLE_SHOULD_NOT_EXIST_XYZ"),
            None
        );
        env::set_var("WAYLAN_TEST_EMPTY_VAR", "   ");
        assert_eq!(env_var_non_empty("WAYLAN_TEST_EMPTY_VAR"), None);
        env::set_var("WAYLAN_TEST_VAL_VAR", "  valid_value  ");
        assert_eq!(
            env_var_non_empty("WAYLAN_TEST_VAL_VAR"),
            Some("valid_value".to_string())
        );
    }

    #[test]
    fn test_config_sanitize_clamps_values() {
        let mut cfg = AppConfig {
            page_size: 1,
            font_size: 5.0,
            ..Default::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.page_size, 5);
        assert_eq!(cfg.font_size, 12.0);

        cfg.page_size = 99;
        cfg.font_size = 100.0;
        cfg.sanitize();
        assert_eq!(cfg.page_size, 9);
        assert_eq!(cfg.font_size, 24.0);

        cfg.font_size = f32::NAN;
        cfg.sanitize();
        assert_eq!(cfg.font_size, 15.0);
    }

    #[test]
    fn test_dict_search_paths() {
        let base_paths = AppConfig::base_dict_search_paths();
        assert!(!base_paths.is_empty());
        let aux_paths = AppConfig::aux_dict_search_paths();
        assert!(!aux_paths.is_empty());
        let opencc_paths = AppConfig::opencc_dict_search_paths();
        assert!(!opencc_paths.is_empty());
    }

    #[test]
    fn test_legacy_string_page_keys_backward_compatibility() {
        let legacy_toml = r#"
init_chinese = true
page_size = 8
font_size = 16.0
switch_key = "Shift"
page_keys = "MinusEqual"
fuzzy_pinyin = true
"#;
        let cfg = AppConfig::parse_tolerant(legacy_toml).expect("旧版字符串翻页键应兼容反序列化");
        assert_eq!(cfg.page_size, 8);
        assert_eq!(cfg.font_size, 16.0);
        assert!(cfg.page_keys.minus_equal);
        assert!(!cfg.page_keys.bracket);
        assert!(!cfg.page_keys.comma_period);
        assert!(cfg.fuzzy_pinyin);

        // 测试中括号
        let bracket_toml = r#"page_keys = "Bracket""#;
        let b_cfg = AppConfig::parse_tolerant(bracket_toml).expect("中括号翻页键应兼容反序列化");
        assert!(!b_cfg.page_keys.minus_equal);
        assert!(b_cfg.page_keys.bracket);
    }

    #[test]
    fn test_tolerant_merge_with_missing_and_extra_fields() {
        let partial_toml = r#"
page_size = 6
font_size = 18.0
unknown_future_field = "some_value"
"#;
        let cfg = AppConfig::parse_tolerant(partial_toml).expect("缺失或多余字段应平滑容错合并");
        assert_eq!(cfg.page_size, 6);
        assert_eq!(cfg.font_size, 18.0);
        // 其余字段应保持默认值
        assert_eq!(cfg.switch_key, SwitchKey::Shift);
        assert!(cfg.v_mode);
    }

    #[test]
    fn test_rescue_from_broken_backups() {
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let broken_path = temp_dir.path().join("config.toml.broken.123456789");
        let broken_content = r#"
page_size = 9
font_size = 20.0
page_keys = "MinusEqual"
fuzzy_pinyin = true
"#;
        fs::write(&broken_path, broken_content).expect("写入 broken 文件失败");

        let target_config = temp_dir.path().join("config.toml");
        let rescued = AppConfig::try_rescue_from_broken_backups(&target_config)
            .expect("应能成功从 broken 备份中恢复配置");
        assert_eq!(rescued.page_size, 9);
        assert_eq!(rescued.font_size, 20.0);
        assert!(rescued.fuzzy_pinyin);
        assert!(rescued.page_keys.minus_equal);
        // 原 broken 文件应已被重命名为 restored
        assert!(!broken_path.exists());
        assert!(broken_path.with_extension("restored").exists());
    }
}
