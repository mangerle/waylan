use crate::config::{env_var_non_empty, AppConfig};
use crate::error::DaemonError;
use chrono::Local;
use log::LevelFilter;
use std::fs;
use std::io::{self, stdout, LineWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// 单个日志文件最大体积上限（10 MB），超过时自动轮转重命名以防无限膨胀
const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// 支持长生命周期运行中根据体积阈值自动轮转的日志文件写入器
pub(crate) struct RollingFileWriter {
    log_path: PathBuf,
    max_bytes: u64,
    current_bytes: u64,
    writer: Option<LineWriter<fs::File>>,
}

impl RollingFileWriter {
    /// 打开日志文件并创建常驻自动轮转写入器
    ///
    /// # Errors
    /// 当无法创建或打开目标日志文件时返回 I/O 错误。
    pub fn open(log_path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        // 启动初期先执行一次预检轮转
        rotate_log_file_if_needed(&log_path, max_bytes);

        let current_bytes = fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        let file = Self::open_file(&log_path)?;
        Ok(Self {
            log_path,
            max_bytes,
            current_bytes,
            writer: Some(LineWriter::new(file)),
        })
    }

    fn open_file(path: &Path) -> io::Result<fs::File> {
        let mut open_opts = fs::OpenOptions::new();
        open_opts.create(true).append(true);
        #[cfg(unix)]
        open_opts.mode(0o600);
        open_opts.open(path)
    }

    /// 执行文件轮转并重新打开新的日志文件
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }

        rotate_log_file_if_needed(&self.log_path, 0);

        let file = Self::open_file(&self.log_path)?;
        self.writer = Some(LineWriter::new(file));
        self.current_bytes = 0;
        Ok(())
    }
}

impl Write for RollingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 若当前文件已有数据且加上待写入数据将超过上限，立即触发轮转
        if self.current_bytes > 0
            && self.current_bytes.saturating_add(buf.len() as u64) > self.max_bytes
        {
            self.rotate()?;
        }
        if let Some(writer) = self.writer.as_mut() {
            let n = writer.write(buf)?;
            self.current_bytes = self.current_bytes.saturating_add(n as u64);
            Ok(n)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "日志写入器底层文件未就绪",
            ))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()
        } else {
            Ok(())
        }
    }
}

/// 初始化微澜输入法双通道日志系统
///
/// 1. 终端输出（stdout）
/// 2. 日志文件（遵循 Linux XDG 规范写入 ~/.local/state/waylan/waylan.log）
///
/// # Errors
/// 当日志文件打开失败或全局日志器已被重复注册时返回错误。
/// 确保日志存储目录存在并设置安全访问权限 (0o700)
fn ensure_log_directory(log_path: &Path) {
    let Some(parent) = log_path.parent() else {
        return;
    };
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        if let Err(e) = builder.create(parent) {
            eprintln!("[微澜日志 警告] 创建日志存储目录 {:?} 失败: {}", parent, e);
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("[微澜日志 警告] 创建日志存储目录 {:?} 失败: {}", parent, e);
        }
    }
}

/// 解析环境变量 `RUST_LOG` 为对应的日志过滤级别
fn parse_log_level_filter() -> LevelFilter {
    let env_level = env_var_non_empty("RUST_LOG").unwrap_or_else(|| "info".to_string());
    match env_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    }
}

pub fn init_logger() -> Result<PathBuf, DaemonError> {
    let log_path = log_file_path();
    ensure_log_directory(&log_path);
    let level_filter = parse_log_level_filter();
    let rolling_writer = RollingFileWriter::open(log_path.clone(), MAX_LOG_FILE_BYTES)?;

    // 使用 fern 构建多目标日志格式化器，并对嘈杂的第三方底层库限制在 Warn 级别
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}] [{}] [{}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(level_filter)
        .level_for("zbus", LevelFilter::Warn)
        .level_for("eframe", LevelFilter::Warn)
        .level_for("egui", LevelFilter::Warn)
        .level_for("winit", LevelFilter::Warn)
        .level_for("wayland_client", LevelFilter::Warn)
        .level_for("rustix", LevelFilter::Warn)
        .chain(stdout())
        .chain(Box::new(rolling_writer) as Box<dyn Write + Send>)
        .apply()
        .map_err(|e| DaemonError::Logger(e.to_string()))?;

    Ok(log_path)
}

/// 检查并执行日志文件轮转（若超过指定体积上限）
pub(crate) fn rotate_log_file_if_needed(log_path: &Path, max_bytes: u64) {
    if let Ok(meta) = fs::metadata(log_path) {
        if meta.len() > max_bytes {
            let backup_2 = log_path.with_extension("log.2");
            let backup_1 = log_path.with_extension("log.1");
            let _ = fs::remove_file(&backup_2);
            let _ = fs::rename(&backup_1, &backup_2);
            let _ = fs::rename(log_path, &backup_1);
        }
    }
}

/// 解析符合 Linux XDG 状态目录规范的日志存储路径
pub fn log_file_path() -> PathBuf {
    if let Some(custom_path) = env_var_non_empty("WAYLAN_LOG_PATH") {
        return PathBuf::from(custom_path);
    }

    if let Some(state_home) = env_var_non_empty("XDG_STATE_HOME") {
        return PathBuf::from(state_home).join("waylan").join("waylan.log");
    }

    if let Some(home) = env_var_non_empty("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("waylan")
            .join("waylan.log");
    }

    // 无 HOME 环境下统一回退至受控安全的运行时目录
    AppConfig::fallback_runtime_dir().join("waylan.log")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_log_file_path_resolution() {
        let p = log_file_path();
        assert!(p.is_absolute());
        assert!(p.ends_with("waylan.log"));
    }

    #[test]
    fn test_rotate_log_file_logic() {
        let temp_dir = tempfile::tempdir().expect("创建测试临时目录失败");
        let log_file = temp_dir.path().join("test.log");

        // 写入初始数据
        {
            let mut f = fs::File::create(&log_file).expect("创建日志文件失败");
            f.write_all(b"initial line of log").expect("写入失败");
        }

        // 阈值设为 5 字节，触发首次轮转
        rotate_log_file_if_needed(&log_file, 5);
        let backup_1 = temp_dir.path().join("test.log.1");
        assert!(backup_1.exists(), "首次轮转应生成 .log.1");
        assert!(!log_file.exists(), "原文件应已被重命名为 .log.1");

        // 再次创建新日志文件
        {
            let mut f = fs::File::create(&log_file).expect("创建新日志文件失败");
            f.write_all(b"second line of log").expect("写入失败");
        }

        // 再次轮转：.log.1 -> .log.2, .log -> .log.1
        rotate_log_file_if_needed(&log_file, 5);
        let backup_2 = temp_dir.path().join("test.log.2");
        assert!(backup_2.exists(), "二次轮转应生成 .log.2");
        assert!(backup_1.exists(), ".log.1 应更新为第二轮内容");
    }

    #[test]
    fn test_line_writer_flushes_on_newline() {
        let temp_dir = tempfile::tempdir().expect("创建测试临时目录失败");
        let log_file_path = temp_dir.path().join("line_writer.log");
        let file = fs::File::create(&log_file_path).expect("创建测试文件失败");
        let mut writer = LineWriter::new(file);

        // 未写换行符时数据仍在缓冲区中
        writer
            .write_all(b"hello without newline")
            .expect("写入失败");
        let len_before = fs::metadata(&log_file_path).unwrap().len();
        assert_eq!(len_before, 0, "未写入换行符时文件大小应为 0");

        // 写入换行符后应自动刷盘到底层文件
        writer.write_all(b"\n").expect("写入换行符失败");
        let len_after = fs::metadata(&log_file_path).unwrap().len();
        assert!(len_after > 0, "写入换行符后应自动刷盘到文件");
    }

    #[test]
    fn test_rolling_file_writer_runtime_rotation() {
        let temp_dir = tempfile::tempdir().expect("创建测试临时目录失败");
        let log_file = temp_dir.path().join("rolling_test.log");

        // 阈值设为 20 字节
        let mut writer = RollingFileWriter::open(log_file.clone(), 20).expect("打开日志写入器");

        // 写入 15 字节（未超限）
        writer.write_all(b"1234567890\n").expect("写入行1");
        writer.flush().expect("刷盘");
        assert!(log_file.exists());
        assert!(!log_file.with_extension("log.1").exists());

        // 再次写入 15 字节（累积 26 字节，达到阈值触发常驻自动轮转）
        writer.write_all(b"abcdefghij\n").expect("写入行2");
        writer.flush().expect("刷盘");

        let backup_1 = log_file.with_extension("log.1");
        assert!(backup_1.exists(), "运行期超出阈值后必须自动生成 .log.1");
        assert!(log_file.exists(), "必须自动生成新的日志文件用于后续写入");

        // 验证原文件已被轮转，新文件只包含第二批数据
        let backup_content = fs::read_to_string(&backup_1).expect("读取备份");
        assert!(backup_content.contains("1234567890"));

        let current_content = fs::read_to_string(&log_file).expect("读取新日志");
        assert!(current_content.contains("abcdefghij"));
    }
}
