//! 配置文件的宽容反序列化、损坏备份救援与原子持久化存储
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供坚如磐石的配置持久化，消除手动操作 `toml::Table` 字典树的复杂性与安全隐患。
//! - **核心优势**：利用 serde 强类型派生默认值（`#[serde(default)]`）实现声明式容错；透明兼顾现代标准分段与历史平铺格式。
//! - **代价与局限**：平铺格式合流优先采纳显式子段落配置，平铺历史字段作为缺省增量补齐。

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::error::ConfigError;
use super::models::*;
use super::AppConfig;

/// 宽容配置反序列化兼容载体（支持标准分段与历史平铺字段无缝合流）
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct FlatConfigFallback {
    // 1. 标准现代分段
    ui: Option<UiConfig>,
    shortcuts: Option<ShortcutConfig>,
    pinyin: Option<PinyinConfig>,
    desktop: Option<DesktopConfig>,
    voice: Option<VoiceConfig>,

    // 2. 历史顶层平铺字段兼容（UI）
    page_size: Option<usize>,
    font_size: Option<f32>,
    theme: Option<ThemeMode>,
    candidate_orientation: Option<CandidateOrientation>,
    candidate_index_style: Option<CandidateIndexStyle>,
    candidate_window_mode: Option<CandidateWindowMode>,
    candidate_density: Option<CandidateDensity>,
    candidate_theme: Option<CandidateThemePreset>,
    candidate_item_spacing: Option<f32>,
    candidate_padding_v: Option<f32>,
    candidate_padding_h: Option<f32>,
    candidate_corner_radius: Option<f32>,
    candidate_opacity: Option<u32>,
    show_pinyin_comment: Option<bool>,

    // 3. 历史顶层平铺字段兼容（Shortcuts）
    switch_key: Option<SwitchKey>,
    page_keys: Option<PageKeys>,
    traditional_shortcut: Option<String>,
    full_width_char_shortcut: Option<String>,
    full_width_punct_shortcut: Option<String>,
    shift_select_second: Option<bool>,

    // 4. 历史顶层平铺字段兼容（Pinyin）
    init_chinese: Option<bool>,
    full_width_char: Option<bool>,
    full_width_punct: Option<bool>,
    dynamic_adjust_freq: Option<bool>,
    enable_acronym: Option<bool>,
    slash_to_dunhao: Option<bool>,
    backslash_to_dunhao: Option<bool>,
    protect_digits_punctuation: Option<bool>,
    auto_complete_pairs: Option<bool>,
    v_mode: Option<bool>,
    enable_u_mode: Option<bool>,
    enable_i_mode: Option<bool>,
    fuzzy_pinyin: Option<bool>,
    fuzzy_rules: Option<FuzzyPinyinRules>,
    typo_rules: Option<TypoCorrectionRules>,
    traditional_chinese: Option<bool>,
    auto_pangu_spacing: Option<bool>,
    filter_uncommon_charset: Option<bool>,
    auto_dedup: Option<bool>,

    // 5. 历史顶层平铺字段兼容（Desktop）
    tray_icon_theme_path: Option<String>,
    tray_icon_zh: Option<String>,
    tray_icon_en: Option<String>,
    enable_tray: Option<TrayDisplayMode>,
    exclusive_input_source: Option<bool>,
}

impl FlatConfigFallback {
    /// 合流为标准的结构化 AppConfig
    fn into_app_config(mut self) -> AppConfig {
        let mut cfg = AppConfig::default();

        // 1. 优先加载标准子段落
        if let Some(ui) = self.ui.take() {
            cfg.ui = ui;
        }
        if let Some(shortcuts) = self.shortcuts.take() {
            cfg.shortcuts = shortcuts;
        }
        if let Some(pinyin) = self.pinyin.take() {
            cfg.pinyin = pinyin;
        }
        if let Some(desktop) = self.desktop.take() {
            cfg.desktop = desktop;
        }
        if let Some(voice) = self.voice.take() {
            cfg.voice = voice;
        }

        // 2. 平铺历史字段兜底覆盖（兼容未划分 [ui]、[pinyin] 等段落的旧版配置）
        self.merge_legacy_ui(&mut cfg.ui);
        self.merge_legacy_shortcuts(&mut cfg.shortcuts);
        self.merge_legacy_pinyin(&mut cfg.pinyin);
        self.merge_legacy_desktop(&mut cfg.desktop);

        cfg.sanitize();
        cfg
    }

    /// 合并历史扁平 UI 配置字段
    fn merge_legacy_ui(&mut self, ui: &mut UiConfig) {
        if let Some(v) = self.page_size {
            ui.page_size = v;
        }
        if let Some(v) = self.font_size {
            ui.font_size = v;
        }
        if let Some(v) = self.theme.take() {
            ui.theme = v;
        }
        if let Some(v) = self.candidate_orientation {
            ui.candidate_orientation = v;
        }
        if let Some(v) = self.candidate_index_style {
            ui.candidate_index_style = v;
        }
        if let Some(v) = self.candidate_window_mode {
            ui.candidate_window_mode = v;
        }
        if let Some(v) = self.candidate_density {
            ui.candidate_density = v;
        }
        if let Some(v) = self.candidate_theme.take() {
            ui.candidate_theme = v;
        }
        if let Some(v) = self.candidate_item_spacing {
            ui.candidate_item_spacing = v;
        }
        if let Some(v) = self.candidate_padding_v {
            ui.candidate_padding_v = v;
        }
        if let Some(v) = self.candidate_padding_h {
            ui.candidate_padding_h = v;
        }
        if let Some(v) = self.candidate_corner_radius {
            ui.candidate_corner_radius = v;
        }
        if let Some(v) = self.candidate_opacity {
            ui.candidate_opacity = v;
        }
        if let Some(v) = self.show_pinyin_comment {
            ui.show_pinyin_comment = v;
        }
    }

    /// 合并历史扁平快捷键配置字段
    fn merge_legacy_shortcuts(&mut self, shortcuts: &mut ShortcutConfig) {
        if let Some(v) = self.switch_key {
            shortcuts.switch_key = v;
        }
        if let Some(v) = self.page_keys {
            shortcuts.page_keys = v;
        }
        if let Some(v) = self.traditional_shortcut.take() {
            shortcuts.traditional_shortcut = v;
        }
        if let Some(v) = self.full_width_char_shortcut.take() {
            shortcuts.full_width_char_shortcut = v;
        }
        if let Some(v) = self.full_width_punct_shortcut.take() {
            shortcuts.full_width_punct_shortcut = v;
        }
        if let Some(v) = self.shift_select_second {
            shortcuts.shift_select_second = v;
        }
    }

    /// 合并历史扁平拼音配置字段
    fn merge_legacy_pinyin(&mut self, pinyin: &mut PinyinConfig) {
        if let Some(v) = self.init_chinese {
            pinyin.init_chinese = v;
        }
        if let Some(v) = self.full_width_char {
            pinyin.full_width_char = v;
        }
        if let Some(v) = self.full_width_punct {
            pinyin.full_width_punct = v;
        }
        if let Some(v) = self.dynamic_adjust_freq {
            pinyin.dynamic_adjust_freq = v;
        }
        if let Some(v) = self.enable_acronym {
            pinyin.enable_acronym = v;
        }
        if let Some(v) = self.slash_to_dunhao {
            pinyin.slash_to_dunhao = v;
        }
        if let Some(v) = self.backslash_to_dunhao {
            pinyin.backslash_to_dunhao = v;
        }
        if let Some(v) = self.protect_digits_punctuation {
            pinyin.protect_digits_punctuation = v;
        }
        if let Some(v) = self.auto_complete_pairs {
            pinyin.auto_complete_pairs = v;
        }
        if let Some(v) = self.v_mode {
            pinyin.v_mode = v;
        }
        if let Some(v) = self.enable_u_mode {
            pinyin.enable_u_mode = v;
        }
        if let Some(v) = self.enable_i_mode {
            pinyin.enable_i_mode = v;
        }
        if let Some(v) = self.fuzzy_pinyin {
            pinyin.fuzzy_pinyin = v;
        }
        if let Some(v) = self.fuzzy_rules {
            pinyin.fuzzy_rules = v;
        }
        if let Some(v) = self.typo_rules.take() {
            pinyin.typo_rules = v;
        }
        if let Some(v) = self.traditional_chinese {
            pinyin.traditional_chinese = v;
        }
        if let Some(v) = self.auto_pangu_spacing {
            pinyin.auto_pangu_spacing = v;
        }
        if let Some(v) = self.filter_uncommon_charset {
            pinyin.filter_uncommon_charset = v;
        }
        if let Some(v) = self.auto_dedup {
            pinyin.auto_dedup = v;
        }
    }

    /// 合并历史扁平桌面环境配置字段
    fn merge_legacy_desktop(&mut self, desktop: &mut DesktopConfig) {
        if let Some(v) = self.tray_icon_theme_path.take() {
            desktop.tray_icon_theme_path = Some(v);
        }
        if let Some(v) = self.tray_icon_zh.take() {
            desktop.tray_icon_zh = Some(v);
        }
        if let Some(v) = self.tray_icon_en.take() {
            desktop.tray_icon_en = Some(v);
        }
        if let Some(v) = self.enable_tray {
            desktop.enable_tray = v;
        }
        if let Some(v) = self.exclusive_input_source {
            desktop.exclusive_input_source = v;
        }
    }
}

impl AppConfig {
    /// 宽容解析配置文件：利用声明式强类型载体平滑合流，自动补齐默认配置
    pub fn parse_tolerant(content: &str) -> Result<Self, ConfigError> {
        match toml::from_str::<FlatConfigFallback>(content) {
            Ok(fallback) => Ok(fallback.into_app_config()),
            Err(e) => Err(ConfigError::SyntaxCorrupted(e.to_string())),
        }
    }

    /// 尝试从同目录下的最新历史损坏备份文件恢复用户配置
    pub(crate) fn try_rescue_from_broken_backups(config_path: &Path) -> Option<Self> {
        let parent = config_path.parent()?;
        let entries = fs::read_dir(parent).ok()?;
        let mut broken_files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("config.toml.broken.") {
                    broken_files.push(path);
                }
            }
        }
        broken_files.sort();
        // 取最新的一份备份尝试拯救
        for broken_path in broken_files.into_iter().rev() {
            if let Ok(content) = fs::read_to_string(&broken_path) {
                if let Ok(rescued_cfg) = Self::parse_tolerant(&content) {
                    log::info!(
                        "[配置系统] 检测到历史备份文件 {:?}，已成功恢复用户的历史个性化设置！",
                        broken_path
                    );
                    let restored_path = broken_path.with_extension("restored");
                    let _ = fs::rename(&broken_path, &restored_path);
                    return Some(rescued_cfg);
                }
            }
        }
        None
    }

    /// 加载配置，若不存在则自动初始化默认配置文件（具备容错合并与备份救援能力）
    pub fn load_or_create() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => match Self::parse_tolerant(&content) {
                        Ok(cfg) => {
                            return cfg;
                        }
                        Err(e) => {
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let broken_path = path.with_extension(format!("toml.broken.{}", ts));
                            let _ = fs::rename(&path, &broken_path);
                            log::warn!(
                                "[配置系统] 严重解析错误 ({})，已备份损坏配置至 {:?}，将尝试救援或回退默认配置。",
                                e,
                                broken_path
                            );
                            if let Some(rescued) = Self::try_rescue_from_broken_backups(&path) {
                                let _ = rescued.save_to_file(&path);
                                return rescued;
                            }
                        }
                    },
                    Err(e) => {
                        log::warn!(
                            "[配置系统] 读取配置文件 {:?} 失败 ({})，将回退使用默认配置。",
                            path,
                            e
                        );
                    }
                }
            } else {
                // 配置文件尚不存在，先尝试从历史备份中救援，若无历史备份则初始化默认配置
                if let Some(rescued) = Self::try_rescue_from_broken_backups(&path) {
                    if let Err(e) = rescued.save_to_file(&path) {
                        log::warn!("[配置系统] 保存已救援的配置至 {:?} 失败: {}", path, e);
                    }
                    return rescued;
                }

                let default_cfg = Self::default();
                if let Err(e) = default_cfg.save_to_file(&path) {
                    log::warn!("[配置系统] 初始化写入默认配置文件 {:?} 失败: {}", path, e);
                }
                return default_cfg;
            }
        }
        Self::default()
    }

    /// 保存配置到默认用户路径
    ///
    /// # Errors
    /// 当无法确定用户配置目录或磁盘写入失败时返回 [`ConfigError`]。
    pub fn save_default(&self) -> Result<(), ConfigError> {
        if let Some(path) = Self::config_path() {
            self.save_to_file(&path)
        } else {
            Err(ConfigError::ConfigDirectoryNotFound)
        }
    }

    /// 保存配置到指定路径
    ///
    /// # Errors
    /// 当目录创建失败、TOML 序列化失败或原子重命名失败时返回 [`ConfigError`]。
    pub fn save_to_file(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.recursive(true).mode(0o700);
                let _ = builder.create(parent);
            }
            #[cfg(not(unix))]
            {
                fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
        }
        let serialized = toml::to_string_pretty(self)?;
        // 使用原子写入：写入同目录临时文件，刷盘后原子 rename 覆盖目标文件
        let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        let write_res: std::io::Result<()> = (|| {
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp_path)?;
            file.write_all(serialized.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp_path, path)?;
            Ok(())
        })();
        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
        Ok(())
    }
}
