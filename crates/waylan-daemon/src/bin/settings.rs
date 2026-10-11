#[path = "../config.rs"]
mod config;
#[path = "../logger.rs"]
mod logger;
#[path = "../settings_gui/mod.rs"]
mod settings_gui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan-settings {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Err(e) = logger::init_logger() {
        eprintln!("[微澜设置] 初始化日志系统失败: {}", e);
    }
    settings_gui::run_settings_gui()
}
