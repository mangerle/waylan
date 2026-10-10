use super::*;

#[test]
fn test_tray_mode_toggle() {
    let is_chinese = Arc::new(AtomicBool::new(true));
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, icon_update_rx) = std::sync::mpsc::sync_channel::<()>(8);
    let tray = TrayService {
        is_chinese: Arc::clone(&is_chinese),
        reload_requested,
        exit_requested,
        icon_update_tx,
    };

    assert!(tray.is_chinese());
    assert!(!tray.is_exit_requested());
    tray.update_mode(InputMode::English);
    assert!(!tray.is_chinese());
    assert!(icon_update_rx.try_recv().is_ok());

    tray.update_mode(InputMode::Chinese);
    assert!(tray.is_chinese());
    assert!(icon_update_rx.try_recv().is_ok());
}

#[test]
fn test_tray_menu_layout_and_event() {
    let is_chinese = Arc::new(AtomicBool::new(true));
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, icon_update_rx) = std::sync::mpsc::sync_channel::<()>(8);
    let menu = DBusMenu {
        is_chinese: Arc::clone(&is_chinese),
        reload_requested: Arc::clone(&reload_requested),
        exit_requested,
        icon_update_tx,
    };

    let (_rev, (_parent_id, _props, items)) = menu.get_layout(0, -1, vec![]);
    assert_eq!(items.len(), 5);

    // 模拟点击菜单第1项（模式切换）
    menu.event(1, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
    assert!(!is_chinese.load(Ordering::Relaxed));
    assert!(icon_update_rx.try_recv().is_ok());

    // 再次点击切回中文
    menu.event(1, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
    assert!(is_chinese.load(Ordering::Relaxed));
    assert!(icon_update_rx.try_recv().is_ok());

    // 模拟点击菜单第2项（重新加载词库与配置）
    assert!(!reload_requested.load(Ordering::Relaxed));
    menu.event(2, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
    assert!(reload_requested.load(Ordering::Relaxed));
}

#[test]
fn test_tray_menu_exit_event() {
    let is_chinese = Arc::new(AtomicBool::new(true));
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, _icon_update_rx) = std::sync::mpsc::sync_channel::<()>(8);
    let menu = DBusMenu {
        is_chinese,
        reload_requested,
        exit_requested: Arc::clone(&exit_requested),
        icon_update_tx,
    };

    assert!(!exit_requested.load(Ordering::Relaxed));
    // 模拟点击菜单第5项（退出微澜输入法）
    menu.event(5, "clicked".to_string(), zbus::zvariant::Value::from(0), 0);
    assert!(exit_requested.load(Ordering::Relaxed));
}

#[test]
fn test_status_notifier_item_activate_sends_icon_update() {
    let is_chinese = Arc::new(AtomicBool::new(true));
    let (icon_update_tx, icon_update_rx) = std::sync::mpsc::sync_channel::<()>(8);
    let sni = StatusNotifierItem {
        is_chinese: Arc::clone(&is_chinese),
        icon_update_tx,
    };

    assert_eq!(sni.icon_name(), "input-keyboard");
    sni.activate(0, 0);
    assert_eq!(sni.icon_name(), "input-latin");
    assert!(icon_update_rx.try_recv().is_ok());
}
