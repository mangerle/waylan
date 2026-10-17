//! 微澜 (WayLan) 输入法偏好设置中心图形界面程序入口
//!
//! # 设计原理
//! - **实现初衷**：作为独立的设置中心客户端二进制执行文件，与常驻输入法后台进程物理隔离。
//! - **核心优势**：用完即退，关闭后由操作系统完整回收内存与 GPU 驱动句柄，彻底杜绝守护进程常驻内存污染。

use waylan_settings::{run_settings_gui, SettingsError};

fn main() -> Result<(), SettingsError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan-settings {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 初始化控制台基础日志输出
    let _ = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}] {}",
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .apply();

    run_settings_gui()
}
