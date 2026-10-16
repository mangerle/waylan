//! 配置文件的宽容反序列化、损坏备份救援与原子持久化存储

use std::fs;
use std::path::Path;

use super::error::ConfigError;
use super::AppConfig;

impl AppConfig {
    /// 宽容解析配置文件：优先严格解析，失败时尝试提取有效字段并以默认配置补齐
    pub fn parse_tolerant(content: &str) -> Result<Self, ConfigError> {
        match toml::from_str::<Self>(content) {
            Ok(mut cfg) => {
                cfg.sanitize();
                Ok(cfg)
            }
            Err(strict_err) => {
                // 尝试作为通用 toml::Table 解析，最大限度挽救用户已有配置项
                let Ok(table) = toml::from_str::<toml::Table>(content) else {
                    return Err(ConfigError::SyntaxCorrupted(strict_err.to_string()));
                };

                let mut base_val =
                    toml::Value::try_from(Self::default()).map_err(ConfigError::TomlSer)?;

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
                    Err(merge_err) => Err(ConfigError::ParseFailed {
                        strict_err: strict_err.to_string(),
                        merge_err: merge_err.to_string(),
                    }),
                }
            }
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
