use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 中英切换键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SwitchKey {
    #[default]
    Shift,
    Ctrl,
}

/// 主题模式选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// 翻页快捷键配置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PageKeys {
    /// 减号与等号 (- / =)
    #[default]
    MinusEqual,
    /// 左右中括号 ([ / ])
    Bracket,
    /// 逗号与句号 (, / .)
    CommaPeriod,
}

/// 微澜输入法全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 中英文切换按键
    pub switch_key: SwitchKey,
    /// 翻页快捷键
    pub page_keys: PageKeys,
    /// 单页候选词数量（默认 7）
    pub page_size: usize,
    /// 候选文字大小（像素）
    pub font_size: f32,
    /// 界面主题模式
    pub theme: ThemeMode,
    /// 是否开启斜杠转顿号（将 /? 替换为 、）
    pub slash_to_dunhao: bool,
    /// 是否开启数字间英文标点保护（如 12:00、3.14）
    pub protect_digits_punctuation: bool,
    /// 是否开启成对符号自动补全
    pub auto_complete_pairs: bool,
    /// 是否启用 V 模式（计算器与快捷符号）
    pub v_mode: bool,
    /// 是否启用模糊拼音（平翘舌、前后鼻音）
    pub fuzzy_pinyin: bool,
    /// 是否默认输出繁体字
    pub traditional_chinese: bool,
    /// 是否开启中西文智能空格（盘古之白，自动在中英文与数字交界处插入半角空格）
    pub auto_pangu_spacing: bool,
    /// 自定义托盘图标主题目录或图标所在目录路径
    pub tray_icon_theme_path: Option<String>,
    /// 自定义中文状态托盘图标名称或路径
    pub tray_icon_zh: Option<String>,
    /// 自定义英文状态托盘图标名称或路径
    pub tray_icon_en: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            switch_key: SwitchKey::Shift,
            page_keys: PageKeys::MinusEqual,
            page_size: 7,
            font_size: 15.0,
            theme: ThemeMode::Light,
            slash_to_dunhao: true,
            protect_digits_punctuation: true,
            auto_complete_pairs: true,
            v_mode: true,
            fuzzy_pinyin: false,
            traditional_chinese: false,
            auto_pangu_spacing: false,
            tray_icon_theme_path: None,
            tray_icon_zh: None,
            tray_icon_en: None,
        }
    }
}

/// 读取非空环境变量（自动过滤空白字符串与未设置变量）
pub(crate) fn env_var_non_empty(key: &str) -> Option<String> {
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
    }

    /// 获取无 HOME 环境下的受控运行时回退目录
    /// 优先使用 XDG_RUNTIME_DIR/waylan，其次回退到带有当前用户 UID 标识且权限为 0700 的 /tmp/waylan-<uid>
    pub(crate) fn fallback_runtime_dir() -> PathBuf {
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

    /// 获取默认系统基础词库的多级查找路径（环境变量 -> 用户数据目录 -> 系统公共目录 -> 相对路径）
    #[allow(dead_code)]
    pub fn base_dict_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(8);
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
        paths.push(PathBuf::from("data/base.dict.bin"));
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
        vec![
            Self::user_aux_dict_path(),
            PathBuf::from("/usr/share/waylan/data/aux.dict.bin"),
            PathBuf::from("/usr/share/waylan/aux.dict.bin"),
            PathBuf::from("/usr/local/share/waylan/data/aux.dict.bin"),
            PathBuf::from("/usr/local/share/waylan/aux.dict.bin"),
            PathBuf::from("data/aux.dict.bin"),
        ]
    }

    /// 获取简繁转换词库（STPhrases + STCharacters）的多级查找路径
    #[allow(dead_code)]
    pub fn opencc_dict_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(8);
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
        paths.push(PathBuf::from("data/opencc.dict.bin"));
        paths
    }

    /// 加载配置，若不存在则自动初始化默认配置文件
    pub fn load_or_create() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str::<Self>(&content) {
                        Ok(mut cfg) => {
                            cfg.sanitize();
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
                                "[配置系统] 解析配置文件 {:?} 失败 ({})，已备份损坏配置至 {:?}，将回退使用默认配置。",
                                path,
                                e,
                                broken_path
                            );
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
        assert_eq!(deserialized.page_size, 7);
        assert_eq!(deserialized.switch_key, SwitchKey::Shift);
        assert_eq!(deserialized.page_keys, PageKeys::MinusEqual);
        assert!(!deserialized.auto_pangu_spacing);

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
        std::env::set_var("WAYLAN_TEST_EMPTY_VAR", "   ");
        assert_eq!(env_var_non_empty("WAYLAN_TEST_EMPTY_VAR"), None);
        std::env::set_var("WAYLAN_TEST_VAL_VAR", "  valid_value  ");
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
}
