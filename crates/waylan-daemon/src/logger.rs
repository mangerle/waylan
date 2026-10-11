use chrono::Local;
use log::LevelFilter;
use std::env;
use std::error::Error;
use std::fs;
use std::io::stdout;
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

    // 确保日志文件所在的目录存在
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // 若现有日志文件超过最大体积限制，自动多槽位滚动（.log.2 -> 删除, .log.1 -> .log.2, .log -> .log.1）防止磁盘耗尽
    if let Ok(meta) = fs::metadata(&log_path) {
        if meta.len() > MAX_LOG_FILE_BYTES {
            let backup_2 = log_path.with_extension("log.2");
            let backup_1 = log_path.with_extension("log.1");
            let _ = fs::remove_file(&backup_2);
            let _ = fs::rename(&backup_1, &backup_2);
            let _ = fs::rename(&log_path, &backup_1);
        }
    }

    // 根据环境变量 RUST_LOG 决定日志级别，默认为 Info
    let env_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
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
        // 终端标准输出通道
        .chain(stdout())
        // 日志文件持久化通道
        .chain(log_file)
        .apply()?;

    Ok(log_path)
}

/// 解析符合 Linux XDG 状态目录规范的日志存储路径
pub fn log_file_path() -> PathBuf {
    if let Ok(custom_path) = env::var("WAYLAN_LOG_PATH") {
        return PathBuf::from(custom_path);
    }

    if let Ok(state_home) = env::var("XDG_STATE_HOME") {
        return PathBuf::from(state_home).join("waylan").join("waylan.log");
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("waylan")
            .join("waylan.log");
    }

    // 回退到当前目录
    PathBuf::from("waylan.log")
}
