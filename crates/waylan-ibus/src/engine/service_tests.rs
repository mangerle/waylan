//! IBus Engine 服务密码框生命周期与能力感知单元测试

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use waylan_engine::{AlgoConfig, AlgoEngine};

use crate::keys::IbusSessionCore;

#[test]
fn test_password_capabilities_and_reset_lifecycle() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let core = Arc::new(Mutex::new(IbusSessionCore::new(engine)));
    let is_password = Arc::new(AtomicBool::new(false));
    let service = IbusEngineService {
        core: Arc::clone(&core),
        reload_requested: None,
        loop_wakeup: None,
        menu_handler: None,
        settings_running: Arc::new(AtomicBool::new(false)),
        suppress_properties: false,
        is_password: Arc::clone(&is_password),
        lock_healer: Arc::new(ScreenLockHealer::default()),
    };

    // 1. 模拟进入密码框，设置密码能力位
    const IBUS_CAP_PASSWORD: u32 = 1 << 4;
    service.set_capabilities(IBUS_CAP_PASSWORD);
    assert!(is_password.load(Ordering::Acquire));
    assert!(core.lock().unwrap().is_password_context);

    // 2. 模拟切回普通窗口，设置非密码能力位（必须成功复位）
    service.set_capabilities(0);
    assert!(!is_password.load(Ordering::Acquire));
    assert!(!core.lock().unwrap().is_password_context);

    // 3. 再次通过 set_content_type 进入密码框
    service.set_content_type((IBUS_INPUT_PURPOSE_PASSWORD, 0));
    assert!(is_password.load(Ordering::Acquire));
    assert!(core.lock().unwrap().is_password_context);

    // 4. 普通内容类型复位
    service.set_content_type((0, 0));
    assert!(!is_password.load(Ordering::Acquire));
    assert!(!core.lock().unwrap().is_password_context);
}

#[test]
fn test_screen_locked_recovery_on_plain_context() {
    let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
    let core = Arc::new(Mutex::new(IbusSessionCore::new(engine)));
    let is_password = Arc::new(AtomicBool::new(false));
    let service = IbusEngineService {
        core: Arc::clone(&core),
        reload_requested: None,
        loop_wakeup: None,
        menu_handler: None,
        settings_running: Arc::new(AtomicBool::new(false)),
        suppress_properties: false,
        is_password: Arc::clone(&is_password),
        lock_healer: Arc::new(ScreenLockHealer::default()),
    };

    // 1. 模拟系统此前记录为锁屏直通状态
    {
        let mut guard = core.lock().unwrap();
        guard.is_screen_locked = true;
        guard.is_password_context = false;
    }

    // 2. 验证获焦自愈检测器正确解除普通窗口的锁屏残留保护
    {
        let mut guard = core.lock().unwrap();
        let healed = service.lock_healer.heal_on_focus(
            guard.is_password_context,
            &mut guard.is_screen_locked,
            None,
        );
        assert!(healed);
    }

    // 3. 验证锁屏直通保护已正确解除
    assert!(!core.lock().unwrap().is_screen_locked);
}
