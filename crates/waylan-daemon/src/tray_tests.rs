use super::*;
use std::sync::mpsc::sync_channel;
use waylan_core::state::SharedInputFlags;
use zbus::Variant;

#[test]
fn test_tray_mode_toggle() {
    let shared_flags = SharedInputFlags::new(true, false, true, false);
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, icon_update_rx) = sync_channel::<()>(8);
    let coordinator = ShutdownCoordinator::from_parts(exit_requested, None);
    let tray = TrayService {
        shared_flags,
        reload_requested,
        coordinator,
        icon_update_tx,
        join_handle: None,
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
    let shared_flags = SharedInputFlags::new(true, false, true, false);
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, icon_update_rx) = sync_channel::<()>(8);
    let coordinator = ShutdownCoordinator::from_parts(exit_requested, None);
    let menu = DBusMenu::new(DBusMenuOptions {
        shared_flags: shared_flags.clone(),
        reload_requested: Arc::clone(&reload_requested),
        coordinator,
        settings_running: Arc::new(AtomicBool::new(false)),
        icon_update_tx,
    });

    let (_rev, (_parent_id, _props, items)) = menu.get_layout(0, -1, vec![]);
    assert_eq!(items.len(), 9);

    // 模拟点击菜单第1项（模式切换）
    menu.event(1, "clicked".to_string(), Variant::from(0), 0);
    assert!(!shared_flags.is_chinese());
    assert!(icon_update_rx.try_recv().is_ok());

    // 模拟点击第6项（简繁切换）
    menu.event(6, "clicked".to_string(), Variant::from(0), 0);
    assert!(shared_flags.is_traditional());
    assert!(icon_update_rx.try_recv().is_ok());

    // 模拟点击第7项（标点全半角切换）
    menu.event(7, "clicked".to_string(), Variant::from(0), 0);
    assert!(!shared_flags.is_full_width_punct());
    assert!(icon_update_rx.try_recv().is_ok());

    // 模拟点击第8项（字符全半角切换）
    menu.event(8, "clicked".to_string(), Variant::from(0), 0);
    assert!(shared_flags.is_full_width_char());
    assert!(icon_update_rx.try_recv().is_ok());

    // 再次点击切回中文
    menu.event(1, "clicked".to_string(), Variant::from(0), 0);
    assert!(shared_flags.is_chinese());
    assert!(icon_update_rx.try_recv().is_ok());

    // 模拟点击菜单第2项（重新加载词库与配置）
    assert!(!reload_requested.load(Ordering::Relaxed));
    menu.event(2, "clicked".to_string(), Variant::from(0), 0);
    assert!(reload_requested.load(Ordering::Relaxed));
}

#[test]
fn test_tray_menu_exit_event() {
    let shared_flags = SharedInputFlags::new(true, false, true, false);
    let reload_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    let (icon_update_tx, _icon_update_rx) = sync_channel::<()>(8);
    let coordinator = ShutdownCoordinator::from_parts(Arc::clone(&exit_requested), None);
    let menu = DBusMenu::new(DBusMenuOptions {
        shared_flags,
        reload_requested,
        coordinator,
        settings_running: Arc::new(AtomicBool::new(false)),
        icon_update_tx,
    });

    assert!(!exit_requested.load(Ordering::Relaxed));
    // 模拟点击菜单第5项（退出微澜输入法）
    menu.event(5, "clicked".to_string(), Variant::from(0), 0);
    assert!(exit_requested.load(Ordering::Relaxed));
}

#[test]
fn test_status_notifier_item_activate_sends_icon_update() {
    let shared_flags = SharedInputFlags::new(true, false, false, false);
    let (icon_update_tx, icon_update_rx) = sync_channel::<()>(8);
    let sni = StatusNotifierItem::new(
        shared_flags,
        icon_update_tx,
        &crate::config::AppConfig::default(),
        None,
    );

    if sni.icon_theme_path().is_empty() {
        assert_eq!(sni.icon_name(), "input-keyboard");
        sni.activate(0, 0);
        assert_eq!(sni.icon_name(), "input-latin");
    } else {
        assert!(sni.icon_name().ends_with("tray-zh.svg") || sni.icon_name() == "tray-zh");
        sni.activate(0, 0);
        assert!(sni.icon_name().ends_with("tray-en.svg") || sni.icon_name() == "tray-en");
    }
    assert!(icon_update_rx.try_recv().is_ok());
}
