//! 配置文件的宽容反序列化、损坏备份救援与原子持久化存储
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供坚如磐石的配置持久化，消除手动操作 `toml::Table` 字典树的复杂性与安全隐患。
//! - **核心优势**：利用 serde 强类型派生默认值（`#[serde(default)]`）实现声明式容错；自动补齐未声明段落的默认配置。

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::error::ConfigError;
use super::AppConfig;

impl AppConfig {
    /// 宽容解析配置文件：利用 serde(default) 声明式容错，自动补齐缺失默认配置
    pub fn parse_tolerant(content: &str) -> Result<Self, ConfigError> {
        let mut cfg: Self =
            toml::from_str(content).map_err(|e| ConfigError::SyntaxCorrupted(e.to_string()))?;
        cfg.sanitize();
        Ok(cfg)
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
            // 备份文件同样是非受信任的历史磁盘数据，必须沿用同一体积护盾，
            // 否则超限主配置被改名备份后仍会在救援阶段被全量读入，守卫形同虚设。
            let within_limit = broken_path
                .metadata()
                .map(|meta| meta.len() <= crate::constants::MAX_CONFIG_FILE_BYTES)
                .unwrap_or(false);
            if !within_limit {
                log::warn!(
                    "[配置系统] 历史备份文件 {:?} 体积超过安全上限，跳过该备份的救援尝试。",
                    broken_path
                );
                continue;
            }
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
        let Some(path) = Self::config_path() else {
            return Self::default();
        };

        if path.exists() {
            Self::load_existing_config(&path)
        } else {
            Self::initialize_fresh_config(&path)
        }
    }

    /// 读取并解析已存在的配置文件，解析失败时备份损坏文件并尝试从历史备份中救援
    ///
    /// 设计意图：将「体积守卫 -> 读取 -> 宽容解析 -> 损坏备份 -> 历史救援」这条容错链与主加载流程分离，
    /// 使主流程仅保留分支判定，容错细节的演进不影响调用方契约。
    fn load_existing_config(path: &Path) -> Self {
        // 体积守卫必须先于整读：异常膨胀的配置文件不得被全量读入内存
        let oversized = path
            .metadata()
            .map(|meta| meta.len() > crate::constants::MAX_CONFIG_FILE_BYTES)
            .unwrap_or(false);
        if oversized {
            return Self::recover_broken_config(path, "配置文件体积超过安全上限");
        }

        match fs::read_to_string(path) {
            Ok(content) => match Self::parse_tolerant(&content) {
                Ok(cfg) => cfg,
                Err(e) => Self::recover_broken_config(path, &format!("严重解析错误 ({})", e)),
            },
            Err(e) => {
                log::warn!(
                    "[配置系统] 读取配置文件 {:?} 失败 ({})，将回退使用默认配置。",
                    path,
                    e
                );
                Self::default()
            }
        }
    }

    /// 将损坏配置文件改名备份，并优先尝试从历史备份救援，全部失败时回退默认配置
    fn recover_broken_config(path: &Path, reason: &str) -> Self {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let broken_path = path.with_extension(format!("toml.broken.{}", ts));
        let _ = fs::rename(path, &broken_path);
        log::warn!(
            "[配置系统] {}，已备份损坏配置至 {:?}，将尝试救援或回退默认配置。",
            reason,
            broken_path
        );
        if let Some(rescued) = Self::try_rescue_from_broken_backups(path) {
            let _ = rescued.save_to_file(path);
            return rescued;
        }
        Self::default()
    }

    /// 在配置文件尚不存在时初始化默认配置，并优先尝试从历史损坏备份中救援
    ///
    /// 设计意图：首次启动与「配置文件被误删」两种场景必须走同一条救援路径，
    /// 避免用户个性化设置因文件丢失而永久丢失。
    fn initialize_fresh_config(path: &Path) -> Self {
        // 配置文件尚不存在，先尝试从历史备份中救援，若无历史备份则初始化默认配置
        if let Some(rescued) = Self::try_rescue_from_broken_backups(path) {
            if let Err(e) = rescued.save_to_file(path) {
                log::warn!("[配置系统] 保存已救援的配置至 {:?} 失败: {}", path, e);
            }
            return rescued;
        }

        let default_cfg = Self::default();
        if let Err(e) = default_cfg.save_to_file(path) {
            log::warn!("[配置系统] 初始化写入默认配置文件 {:?} 失败: {}", path, e);
        }
        default_cfg
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
        if let Err(e) = write_serialized_atomically(&tmp_path, path, &serialized) {
            let _ = fs::remove_file(&tmp_path);
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
        Ok(())
    }
}

/// 将已序列化文本原子写入目标路径（临时文件 -> 刷盘 -> rename 覆盖）
///
/// # 设计原理
/// - **实现初衷**：配置文件承载用户全部偏好，必须保证任何时刻磁盘上都是完整可读的版本，
///   严禁出现写入中断造成的半截文件。
/// - **核心优势**：`sync_all` 确保数据落盘后才执行 `rename`，rename 在同目录内为原子操作；
///   Unix 下临时文件与最终文件均以 0o600 创建，避免多用户主机上的偏好泄露。
/// - **代价与局限**：跨文件系统重命名不在支持范围内，故临时文件必须与目标文件同目录。
fn write_serialized_atomically(
    tmp_path: &Path,
    path: &Path,
    serialized: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(tmp_path)?;
    file.write_all(serialized.as_bytes())?;
    file.sync_all()?;
    fs::rename(tmp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MAX_CONFIG_FILE_BYTES;

    #[test]
    fn test_oversized_config_is_backed_up_and_defaults() {
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let config_path = temp_dir.path().join("config.toml");
        // 写入超过安全上限的畸形配置文件，验证其被拒绝读入并转入损坏救援分支
        let oversized = format!(
            "[ui]\npage_size = 9\n# {}{}",
            "填充".repeat(2_000),
            "x".repeat(MAX_CONFIG_FILE_BYTES as usize)
        );
        fs::write(&config_path, oversized).expect("写入超限配置文件失败");

        let cfg = AppConfig::load_existing_config(&config_path);
        assert_eq!(cfg, AppConfig::default(), "超限配置必须回退为默认配置");
        assert!(
            !config_path.exists(),
            "超限配置应已被改名备份，不得继续留在原路径"
        );

        let backed_up = std::fs::read_dir(temp_dir.path())
            .expect("读取临时目录失败")
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.broken.")
            });
        assert!(backed_up, "应存在 config.toml.broken.* 备份文件");
    }

    #[test]
    fn test_normal_config_within_limit_is_parsed() {
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, "[ui]\npage_size = 7\n").expect("写入配置文件失败");

        let cfg = AppConfig::load_existing_config(&config_path);
        assert_eq!(cfg.ui.page_size, 7);
        assert!(config_path.exists(), "正常配置不得被备份移走");
    }
}
