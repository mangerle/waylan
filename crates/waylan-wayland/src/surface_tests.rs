use super::*;
use std::os::fd::AsRawFd;

#[test]
fn test_cursor_popup_position() {
    let cursor = CursorRect::new(100, 200, 2, 20);
    // 普通情况：光标下方
    let (x, y) = cursor.compute_popup_position(300, 40, 1080);
    assert_eq!(x, 100);
    assert_eq!(y, 224); // 200 + 20 + 4

    // 接近屏幕底部：翻转至光标上方
    let bottom_cursor = CursorRect::new(100, 1050, 2, 20);
    let (bx, by) = bottom_cursor.compute_popup_position(300, 40, 1080);
    assert_eq!(bx, 100);
    assert_eq!(by, 1050 - 40 - 4); // 1006

    // 接近屏幕右边缘：自动向左平移留出安全间距，防止被右侧边缘截断
    let right_cursor = CursorRect::new(1800, 500, 2, 20);
    let (rx, ry) = right_cursor.compute_popup_position_constrained(300, 40, 1920, 1080);
    assert_eq!(rx, 1920 - 300 - 4); // 1616
    assert_eq!(ry, 524);
}

#[test]
fn test_rgba_to_argb8888_le_conversion() {
    let mut manager = CandidateSurfaceManager::new();
    // 输入两个像素: [R=10, G=20, B=30, A=255] 与 [R=100, G=150, B=200, A=128]
    let rgba = vec![10, 20, 30, 255, 100, 150, 200, 128];
    manager.convert_rgba_to_pixel_buffer(&rgba);
    assert_eq!(
        manager.pixel_buffer,
        vec![30, 20, 10, 255, 200, 150, 100, 128]
    );
}

#[test]
fn test_shm_slot_reuse_and_ping_pong() {
    let mut manager = CandidateSurfaceManager::new();
    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![
            waylan_core::Candidate::new("策", "ce", 100),
            waylan_core::Candidate::new("测", "ce", 90),
        ],
        ..CandidateBoxState::default()
    };

    // 第一次渲染：应初始化 slot 1
    let (w1, h1, file1) = manager.render_state(&state).expect("第一次渲染失败");
    assert!(w1 > 0 && h1 > 0);
    let fd1 = file1.as_raw_fd();
    assert_eq!(manager.current_slot, 1);

    // 第二次渲染：应轮换初始化 slot 0
    let (w2, h2, file2) = manager.render_state(&state).expect("第二次渲染失败");
    assert!(w2 > 0 && h2 > 0);
    let fd2 = file2.as_raw_fd();
    assert_eq!(manager.current_slot, 0);
    assert_ne!(fd1, fd2, "双缓冲两个槽位应对应独立的内核文件描述符");

    // 第三次渲染：应轮换复用 slot 1 的 fd，不再重复调用 tempfile()
    let (_, _, file3) = manager.render_state(&state).expect("第三次渲染失败");
    let fd3 = file3.as_raw_fd();
    assert_eq!(manager.current_slot, 1);
    assert_eq!(fd1, fd3, "第三次渲染应复用 slot 1 的文件描述符");

    // 第四次渲染：应轮换复用 slot 0 的 fd
    let (_, _, file4) = manager.render_state(&state).expect("第四次渲染失败");
    let fd4 = file4.as_raw_fd();
    assert_eq!(manager.current_slot, 0);
    assert_eq!(fd2, fd4, "第四次渲染应复用 slot 0 的文件描述符");
}

#[test]
fn test_surface_manager_scale() {
    let mut manager = CandidateSurfaceManager::new();
    assert_eq!(manager.scale, 1);

    manager.set_scale(2);
    assert_eq!(manager.scale, 2);
    assert!((manager.renderer.scale - 2.0).abs() < 0.001);

    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![waylan_core::Candidate::new("测试", "ce'shi", 100)],
        ..CandidateBoxState::default()
    };

    let (w, h, _) = manager.render_state(&state).expect("2x 渲染位图失败");
    assert_eq!(h, 76);
    assert!(w > 100);
}

#[test]
fn test_shm_slot_all_busy_drops_frame() {
    let mut manager = CandidateSurfaceManager::new();
    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![waylan_core::Candidate::new("测试", "ce'shi", 100)],
        ..CandidateBoxState::default()
    };

    // 渲染两次填满两个槽位
    let _ = manager.render_state(&state);
    let _ = manager.render_state(&state);

    // 人工模拟合成器尚未释放两个槽位（两槽皆 busy）
    for slot in manager.shm_slots.iter_mut().flatten() {
        slot.is_busy = true;
    }

    // 此时再次尝试渲染，应安全返回 None 丢帧，杜绝并发写入 busy 槽位引发花屏
    let third_res = manager.render_state(&state);
    assert!(third_res.is_none(), "两槽皆 busy 时应放弃提交");
}
