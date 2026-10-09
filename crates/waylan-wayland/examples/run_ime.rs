use waylan_wayland::protocol_v1::input_method_v1::zwp_input_method_v1::ZwpInputMethodV1;
use waylan_wayland::session_v1::WaylanV1Session;
use wayland_client::Connection;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--------------------------------------------------");
    println!("  微澜 (WayLan) 输入法服务正在启动...");
    println!("--------------------------------------------------");

    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = wayland_client::globals::registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    // 绑定 zwp_input_method_v1
    let mut im_global = None;
    for g in globals.contents().clone_list() {
        if g.interface == "zwp_input_method_v1" {
            im_global = Some(g);
            break;
        }
    }

    let Some(_global_info) = im_global else {
        eprintln!("[微澜 错误] 当前 Wayland 合成器未暴露 zwp_input_method_v1 接口！");
        return Ok(());
    };

    let _input_method: ZwpInputMethodV1 = globals.bind(&qh, 1..=1, ())?;
    let mut session = WaylanV1Session::new();

    println!("[微澜] 成功注册为系统输入法服务！");
    println!(
        "[微澜] 状态：运行中。随时可以在任意支持 Wayland 的应用（如终端或文本框）中点击测试。"
    );
    println!("[微澜] 快捷键说明：单按 Shift 切换中英文，中文模式下敲击字母输入拼音，按空格选词上屏，按 / 自动输入顿号。");

    loop {
        event_queue.blocking_dispatch(&mut session)?;
    }
}
