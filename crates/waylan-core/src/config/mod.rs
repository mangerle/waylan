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

pub mod models;
pub use models::*;

/// 微澜输入法全局分层配置
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    /// 界面视觉排版与候选框渲染配置
    pub ui: UiConfig,
    /// 快捷键与操作按键配置
    pub shortcuts: ShortcutConfig,
    /// 拼音算法、词库处理与字符输出配置
    pub pinyin: PinyinConfig,
    /// 桌面环境集成与系统托盘配置
    pub desktop: DesktopConfig,
    /// 语音输入功能配置
    pub voice: VoiceConfig,
}

impl AppConfig {
    /// 校验并钳制配置参数边界，防止手工修改出非法数值引发除零或渲染异常
    pub fn sanitize(&mut self) {
        self.ui.sanitize();
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
        assert!(serialized.contains("backslash_to_dunhao = true"));
        assert!(serialized.contains("font_size = 15.0"));

        let deserialized: AppConfig = toml::from_str(&serialized).expect("反序列化应成功");
        assert_eq!(deserialized.ui.page_size, 5);
        assert_eq!(
            deserialized.ui.candidate_index_style,
            CandidateIndexStyle::ModernSpace
        );
        assert_eq!(deserialized.shortcuts.switch_key, SwitchKey::Shift);
        assert_eq!(deserialized.shortcuts.page_keys, PageKeys::default());
        assert!(!deserialized.pinyin.auto_pangu_spacing);
        assert!(deserialized.pinyin.enable_u_mode);
        assert!(deserialized.pinyin.enable_i_mode);
        assert!(!deserialized.pinyin.filter_uncommon_charset);
        assert!(deserialized.ui.show_pinyin_comment);
        assert!(deserialized.pinyin.auto_dedup);
        assert!(deserialized.pinyin.backslash_to_dunhao);

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
        let mut cfg = AppConfig::default();
        cfg.ui.page_size = 1;
        cfg.ui.font_size = 5.0;
        cfg.sanitize();
        assert_eq!(cfg.ui.page_size, 5);
        assert_eq!(cfg.ui.font_size, 12.0);

        cfg.ui.page_size = 99;
        cfg.ui.font_size = 100.0;
        cfg.sanitize();
        assert_eq!(cfg.ui.page_size, 9);
        assert_eq!(cfg.ui.font_size, 24.0);

        cfg.ui.font_size = f32::NAN;
        cfg.sanitize();
        assert_eq!(cfg.ui.font_size, 15.0);
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
    fn test_tolerant_merge_with_missing_and_extra_fields() {
        let partial_toml = r#"
page_size = 6
font_size = 18.0
unknown_future_field = "some_value"
"#;
        let cfg = AppConfig::parse_tolerant(partial_toml).expect("缺失或多余字段应平滑容错合并");
        assert_eq!(cfg.ui.page_size, 6);
        assert_eq!(cfg.ui.font_size, 18.0);
        // 其余字段应保持默认值
        assert_eq!(cfg.shortcuts.switch_key, SwitchKey::Shift);
        assert!(cfg.pinyin.v_mode);
    }

    #[test]
    fn test_rescue_from_broken_backups() {
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let broken_path = temp_dir.path().join("config.toml.broken.123456789");
        let broken_content = r#"
page_size = 9
font_size = 20.0
fuzzy_pinyin = true
"#;
        fs::write(&broken_path, broken_content).expect("写入 broken 文件失败");

        let target_config = temp_dir.path().join("config.toml");
        let rescued = AppConfig::try_rescue_from_broken_backups(&target_config)
            .expect("应能成功从 broken 备份中恢复配置");
        assert_eq!(rescued.ui.page_size, 9);
        assert_eq!(rescued.ui.font_size, 20.0);
        assert!(rescued.pinyin.fuzzy_pinyin);
        assert!(rescued.shortcuts.page_keys.minus_equal);
        // 原 broken 文件应已被重命名为 restored
        assert!(!broken_path.exists());
        assert!(broken_path.with_extension("restored").exists());
    }

    #[test]
    fn test_standard_section_and_hybrid_parsing() {
        // 1. 标准 TOML 分段解析
        let sectioned_toml = r#"
[ui]
page_size = 7
font_size = 16.0

[shortcuts]
switch_key = "Ctrl"

[pinyin]
slash_to_dunhao = false
"#;
        let cfg = AppConfig::parse_tolerant(sectioned_toml).expect("标准分段解析应成功");
        assert_eq!(cfg.ui.page_size, 7);
        assert_eq!(cfg.ui.font_size, 16.0);
        assert_eq!(cfg.shortcuts.switch_key, SwitchKey::Ctrl);
        assert!(!cfg.pinyin.slash_to_dunhao);
        assert!(cfg.pinyin.v_mode); // 默认值应补齐

        // 2. 混合解析：标准分段结合根级历史平铺覆盖
        let hybrid_toml = r#"
page_size = 8

[shortcuts]
switch_key = "Ctrl"
"#;
        let hybrid_cfg = AppConfig::parse_tolerant(hybrid_toml).expect("混合格式解析应成功");
        assert_eq!(hybrid_cfg.ui.page_size, 8);
        assert_eq!(hybrid_cfg.shortcuts.switch_key, SwitchKey::Ctrl);
    }
}
