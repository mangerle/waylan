use crate::config::{env_var_non_empty, AppConfig};
use chrono::Local;
use log::LevelFilter;
use std::error::Error;
use std::fs;
use std::io::{stdout, LineWriter};
use std::path::PathBuf;

/// 单个日志文件最大体积上限（10 MB），超过时自动轮转重命名以防无限膨胀
const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// 初始化微澜输入法双通道日志系统
///
/// 1. 终端输出（stdout）
/// 2. 日志文件（遵循 Linux XDG 规范写入 ~/.local/state/waylan/waylan.log）
///
/// # Errors
/// 当日志文件打开失败或全局日志器已被重复注册时返回错误。
pub fn init_logger() -> Result<PathBuf, Box<dyn Error>> {
    // 确定日志文件路径
    let log_path = log_file_path();

    // 确保日志文件所在的目录存在并设置 0o700 安全权限
    if let Some(parent) = log_path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            let _ = builder.create(parent);
        }
        #[cfg(not(unix))]
        {
            let _ = fs::create_dir_all(parent);
        }
    }

    // 若现有日志文件超过最大体积限制，自动多槽位滚动（.log.2 -> 删除, .log.1 -> .log.2, .log -> .log.1）防止磁盘耗尽
    rotate_log_file_if_needed(&log_path, MAX_LOG_FILE_BYTES);

    // 根据环境变量 RUST_LOG 决定日志级别，默认为 Info
    let env_level = env_var_non_empty("RUST_LOG").unwrap_or_else(|| "info".to_string());
    let level_filter = match env_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };

    let mut open_opts = fs::OpenOptions::new();
    open_opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }
    let log_file = open_opts.open(&log_path)?;
    let line_writer = LineWriter::new(log_file);

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
        // 终端标准输出通道
        .chain(stdout())
        // 日志文件持久化通道（按行自动刷盘，防止未捕获异常退出或异常中断时末尾日志丢失）
        .chain(Box::new(line_writer) as Box<dyn std::io::Write + Send>)
        .apply()?;

    Ok(log_path)
}

/// 检查并执行日志文件轮转（若超过指定体积上限）
pub(crate) fn rotate_log_file_if_needed(log_path: &std::path::Path, max_bytes: u64) {
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
}
