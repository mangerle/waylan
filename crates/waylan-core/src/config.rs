use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub use crate::config_types::{
    AudioFrontEndConfig, CandidateIndexStyle, CandidateOrientation, CloudAsrConfig, CloudProfile,
    CloudProviderType, FullWidthCharShortcut, FullWidthPunctShortcut, FuzzyPinyinRules,
    NsEngineType, OfflineAsrConfig, OfflineModelType, PageKeys, ParsedShortcut, SwitchKey,
    ThemeMode, TraditionalShortcut, TypoCorrectionRules, VadConfig, VadModelType, VoiceConfig,
    VoiceEngineType, VoiceInteractionConfig, VoiceItnConfig,
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

/// 读取非空环境变量（自动过滤空白字符串与未设置变量）
pub fn env_var_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
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
        self.voice.vad.silence_timeout_ms = self.voice.vad.silence_timeout_ms.clamp(200, 5000);
        self.voice.vad.speech_threshold = self.voice.vad.speech_threshold.clamp(0.01, 0.99);
    }

    /// 获取无 HOME 环境下的受控运行时回退目录
    /// 优先使用 XDG_RUNTIME_DIR/waylan，其次回退到带有当前用户 UID 标识且权限为 0700 的 /tmp/waylan-<uid>
    pub fn fallback_runtime_dir() -> PathBuf {
        if let Some(runtime_dir) = env_var_non_empty("XDG_RUNTIME_DIR") {
            let dir = PathBuf::from(runtime_dir).join("waylan");
            let _ = fs::create_dir_all(&dir);
            return dir;
        }

        #[cfg(unix)]
        let uid = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(1000)
        };
        #[cfg(not(unix))]
        let uid = 1000;

        let dir = env::temp_dir().join(format!("waylan-{}", uid));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            let _ = builder.create(&dir);
        }
        #[cfg(not(unix))]
        {
            let _ = fs::create_dir_all(&dir);
        }
        dir
    }

    /// 获取用户默认配置路径 (~/.config/waylan/config.toml)
    pub fn config_path() -> Option<PathBuf> {
        if let Some(custom_path) = env_var_non_empty("WAYLAN_CONFIG_PATH") {
            return Some(PathBuf::from(custom_path));
        }
        if let Some(config_home) = env_var_non_empty("XDG_CONFIG_HOME") {
            return Some(
                PathBuf::from(config_home)
                    .join("waylan")
                    .join("config.toml"),
            );
        }
        if let Some(home) = env_var_non_empty("HOME") {
            return Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("waylan")
                    .join("config.toml"),
            );
        }
        Some(Self::fallback_runtime_dir().join("config.toml"))
    }

    /// 获取符合 XDG 规范的用户词库文件存储路径 (~/.local/share/waylan/user_dict.bin)
    #[allow(dead_code)]
    pub fn user_dict_path() -> PathBuf {
        if let Some(custom) = env_var_non_empty("WAYLAN_USER_DICT") {
            return PathBuf::from(custom);
        }
        if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
            return PathBuf::from(data_home)
                .join("waylan")
                .join("user_dict.bin");
        }
        if let Some(home) = env_var_non_empty("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("waylan")
                .join("user_dict.bin");
        }
        Self::fallback_runtime_dir().join("user_dict.bin")
    }

    /// 辅助根据执行文件所在目录向多级父路径推入候选词库位置（适应开发、测试与各类宿主子进程环境）
    fn push_exe_relative_candidates(paths: &mut Vec<PathBuf>, rel_subpath: &str) {
        if let Ok(exe) = env::current_exe() {
            if let Some(parent) = exe.parent() {
                paths.push(parent.join(rel_subpath));
                paths.push(parent.join("..").join(rel_subpath));
                paths.push(parent.join("../..").join(rel_subpath));
                paths.push(parent.join("../../..").join(rel_subpath));
                paths.push(parent.join("../share/waylan").join(rel_subpath));
            }
        }
    }

    /// 获取默认系统基础词库的多级查找路径（环境变量 -> 用户数据目录 -> 系统公共目录 -> 相对路径）
    #[allow(dead_code)]
    pub fn base_dict_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(16);
        if let Some(custom) = env_var_non_empty("WAYLAN_DICT_PATH") {
            paths.push(PathBuf::from(custom));
        }
        if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
            paths.push(
                PathBuf::from(data_home)
                    .join("waylan")
                    .join("base.dict.bin"),
            );
        }
        if let Some(home) = env_var_non_empty("HOME") {
            paths.push(
                PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("waylan")
                    .join("base.dict.bin"),
            );
        }
        paths.push(PathBuf::from("/usr/share/waylan/data/base.dict.bin"));
        paths.push(PathBuf::from("/usr/share/waylan/base.dict.bin"));
        paths.push(PathBuf::from("/usr/local/share/waylan/data/base.dict.bin"));
        paths.push(PathBuf::from("/usr/local/share/waylan/base.dict.bin"));
        Self::push_exe_relative_candidates(&mut paths, "data/base.dict.bin");
        Self::push_exe_relative_candidates(&mut paths, "base.dict.bin");
        paths.push(PathBuf::from("data/base.dict.bin"));
        paths.push(PathBuf::from("base.dict.bin"));
        paths
    }

    /// 获取符合 XDG 规范的用户辅助与混输词库存储路径 (~/.local/share/waylan/aux.dict.bin)
    #[allow(dead_code)]
    pub fn user_aux_dict_path() -> PathBuf {
        if let Some(custom) = env_var_non_empty("WAYLAN_AUX_DICT_PATH") {
            return PathBuf::from(custom);
        }
        if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
            return PathBuf::from(data_home).join("waylan").join("aux.dict.bin");
        }
        if let Some(home) = env_var_non_empty("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("waylan")
                .join("aux.dict.bin");
        }
        Self::fallback_runtime_dir().join("aux.dict.bin")
    }

    /// 获取辅助与混输词库（英文与符号表情）的多级查找路径
    #[allow(dead_code)]
    pub fn aux_dict_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(16);
        paths.push(Self::user_aux_dict_path());
        paths.push(PathBuf::from("/usr/share/waylan/data/aux.dict.bin"));
        paths.push(PathBuf::from("/usr/share/waylan/aux.dict.bin"));
        paths.push(PathBuf::from("/usr/local/share/waylan/data/aux.dict.bin"));
        paths.push(PathBuf::from("/usr/local/share/waylan/aux.dict.bin"));
        Self::push_exe_relative_candidates(&mut paths, "data/aux.dict.bin");
        Self::push_exe_relative_candidates(&mut paths, "aux.dict.bin");
        paths.push(PathBuf::from("data/aux.dict.bin"));
        paths.push(PathBuf::from("aux.dict.bin"));
        paths
    }

    /// 获取简繁转换词库（STPhrases + STCharacters）的多级查找路径
    #[allow(dead_code)]
    pub fn opencc_dict_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(16);
        if let Some(custom) = env_var_non_empty("WAYLAN_OPENCC_DICT_PATH") {
            paths.push(PathBuf::from(custom));
        }
        if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
            paths.push(
                PathBuf::from(data_home)
                    .join("waylan")
                    .join("opencc.dict.bin"),
            );
        }
        if let Some(home) = env_var_non_empty("HOME") {
            paths.push(
                PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("waylan")
                    .join("opencc.dict.bin"),
            );
        }
        paths.push(PathBuf::from("/usr/share/waylan/data/opencc.dict.bin"));
        paths.push(PathBuf::from("/usr/share/waylan/opencc.dict.bin"));
        paths.push(PathBuf::from(
            "/usr/local/share/waylan/data/opencc.dict.bin",
        ));
        paths.push(PathBuf::from("/usr/local/share/waylan/opencc.dict.bin"));
        Self::push_exe_relative_candidates(&mut paths, "data/opencc.dict.bin");
        Self::push_exe_relative_candidates(&mut paths, "opencc.dict.bin");
        paths.push(PathBuf::from("data/opencc.dict.bin"));
        paths.push(PathBuf::from("opencc.dict.bin"));
        paths
    }

    /// 宽容解析配置文件：优先严格解析，失败时尝试提取有效字段并以默认配置补齐
    pub fn parse_tolerant(content: &str) -> Result<Self, String> {
        match toml::from_str::<Self>(content) {
            Ok(mut cfg) => {
                cfg.sanitize();
                Ok(cfg)
            }
            Err(strict_err) => {
                // 尝试作为通用 toml::Table 解析，最大限度挽救用户已有配置项
                let Ok(table) = toml::from_str::<toml::Table>(content) else {
                    return Err(format!("TOML 语法损坏无法解析: {}", strict_err));
                };

                let mut base_val = toml::Value::try_from(Self::default())
                    .map_err(|e| format!("序列化默认配置失败: {}", e))?;

                if let toml::Value::Table(ref mut base_table) = base_val {
                    for (k, v) in table {
                        if let Some(existing) = base_table.get_mut(&k) {
                            match (existing, v) {
                                (
                                    toml::Value::Table(ref mut ex_tbl),
                                    toml::Value::Table(new_tbl),
                                ) => {
                                    for (sub_k, sub_v) in new_tbl {
                                        ex_tbl.insert(sub_k, sub_v);
                                    }
                                }
                                (_, non_tbl_val) => {
                                    base_table.insert(k, non_tbl_val);
                                }
                            }
                        } else {
                            base_table.insert(k, v);
                        }
                    }
                }

                match base_val.try_into::<Self>() {
                    Ok(mut merged_cfg) => {
                        merged_cfg.sanitize();
                        log::warn!(
                            "[配置系统] 配置文件存在格式偏差 ({})，已平滑容错合并有效配置项并补充默认字段。",
                            strict_err
                        );
                        Ok(merged_cfg)
                    }
                    Err(merge_err) => Err(format!(
                        "严格解析失败: {}，容错合并失败: {}",
                        strict_err, merge_err
                    )),
                }
            }
        }
    }

    /// 尝试从同目录下的最新历史损坏备份文件恢复用户配置
    fn try_rescue_from_broken_backups(config_path: &Path) -> Option<Self> {
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
    /// 当无法确定用户配置目录或磁盘写入失败时返回错误。
    pub fn save_default(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(path) = Self::config_path() {
            self.save_to_file(&path)?;
            Ok(())
        } else {
            Err("无法确定用户配置文件存储路径".into())
        }
    }

    /// 保存配置到指定路径
    ///
    /// # Errors
    /// 当目录创建失败、TOML 序列化失败或原子重命名失败时返回错误。
    pub fn save_to_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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
                fs::create_dir_all(parent)?;
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
            return Err(e.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
