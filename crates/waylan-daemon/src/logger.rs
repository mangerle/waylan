use chrono::Local;
use log::LevelFilter;
use std::env;
use std::fs;
use std::path::PathBuf;

/// 初始化微澜输入法双通道日志系统
///
/// 1. 终端输出（stdout）
/// 2. 日志文件（遵循 Linux XDG 规范写入 ~/.local/state/waylan/waylan.log）
pub fn init_logger() -> Result<PathBuf, Box<dyn std::error::Error>> {
    // 确定日志文件路径
    let log_path = log_file_path();

    // 确保日志文件所在的目录存在
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
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

    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // 使用 fern 构建多目标日志格式化器
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
        // 终端标准输出通道
        .chain(std::io::stdout())
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
