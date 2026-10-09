use waylan_wayland::probe_wayland_environment;

fn main() {
    println!("正在尝试连接 Wayland 合成器并探测协议能力...");
    match probe_wayland_environment() {
        Ok(caps) => {
            println!("成功连接到 Wayland 会话！");
            println!("核心接口支持情况：");
            println!("  - wl_compositor: {}", caps.has_compositor);
            println!("  - wl_seat: {}", caps.has_seat);
            println!(
                "  - zwp_input_method_v1 (输入法 v1): {}",
                caps.has_input_method_v1
            );
            println!(
                "  - zwp_input_method_manager_v2 (特权输入法 v2): {}",
                caps.has_input_method_v2
            );
            println!(
                "  - zwlr_layer_shell_v1 (无焦点置顶窗口): {}",
                caps.has_layer_shell_v1
            );
            println!(
                "\n合成器广播的所有全局接口列表 (共 {} 项):",
                caps.all_globals.len()
            );
            for g in &caps.all_globals {
                println!("    * {}", g);
            }
        }
        Err(e) => {
            eprintln!("无法连接到 Wayland 会话: {}", e);
        }
    }
}
