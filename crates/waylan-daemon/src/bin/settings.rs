#[path = "../config.rs"]
mod config;
#[path = "../settings_gui.rs"]
mod settings_gui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan-settings {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    settings_gui::run_settings_gui()
}
