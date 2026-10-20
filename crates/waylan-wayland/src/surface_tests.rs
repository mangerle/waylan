use super::cursor_rect::PopupPositionParams;
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
    let (rx, ry) = right_cursor.compute_popup_position_constrained(&PopupPositionParams {
        popup_width: 300,
        popup_height: 40,
        screen_width: 1920,
        screen_height: 1080,
    });
    assert_eq!(rx, 1920 - 300 - 4); // 1616
    assert_eq!(ry, 524);
}

#[test]
fn test_rgba_to_argb8888_le_conversion() {
    let mut slot = ShmSlot::new(8 * 1024).expect("创建 ShmSlot");
    // 输入两个像素: [R=10, G=20, B=30, A=255] 与 [R=100, G=150, B=200, A=128]
    let rgba = vec![10, 20, 30, 255, 100, 150, 200, 128];
    CandidateSurfaceManager::write_rgba_to_slot(&mut slot, &rgba).expect("写入零拷贝 mmap");
    let mmap = slot.mmap.as_ref().unwrap();
    assert_eq!(&mmap[0..8], &[30, 20, 10, 255, 200, 150, 100, 128]);

    // 验证大批量多像素连续重排正确性
    let mut batch_rgba = Vec::with_capacity(1024 * 4);
    for i in 0..1024u32 {
        batch_rgba.push((i & 0xFF) as u8); // R
        batch_rgba.push(((i + 1) & 0xFF) as u8); // G
        batch_rgba.push(((i + 2) & 0xFF) as u8); // B
        batch_rgba.push(255); // A
    }
    CandidateSurfaceManager::write_rgba_to_slot(&mut slot, &batch_rgba).expect("写入零拷贝 mmap");
    let mmap = slot.mmap.as_ref().unwrap();
    for (idx, chunk) in mmap[..1024 * 4].as_chunks::<4>().0.iter().enumerate() {
        let i = idx as u32;
        assert_eq!(chunk[0], ((i + 2) & 0xFF) as u8); // B
        assert_eq!(chunk[1], ((i + 1) & 0xFF) as u8); // G
        assert_eq!(chunk[2], (i & 0xFF) as u8); // R
        assert_eq!(chunk[3], 255); // A
    }
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

    // 第二次渲染：应初始化 slot 2
    let (w2, h2, file2) = manager.render_state(&state).expect("第二次渲染失败");
    assert!(w2 > 0 && h2 > 0);
    let fd2 = file2.as_raw_fd();
    assert_eq!(manager.current_slot, 2);
    assert_ne!(fd1, fd2, "不同槽位应对应独立的内核文件描述符");

    // 第三次渲染：应初始化 slot 3
    let (_, _, _) = manager.render_state(&state).expect("第三次渲染失败");
    assert_eq!(manager.current_slot, 3);

    // 第四次渲染：应初始化 slot 0
    let (_, _, _) = manager.render_state(&state).expect("第四次渲染失败");
    assert_eq!(manager.current_slot, 0);

    // 第五次渲染：完成一轮闭环，轮换复用 slot 1 的 fd，不再重复调用 tempfile()
    let (_, _, file5) = manager.render_state(&state).expect("第五次渲染失败");
    let fd5 = file5.as_raw_fd();
    assert_eq!(manager.current_slot, 1);
    assert_eq!(fd1, fd5, "第五次渲染应复用 slot 1 的文件描述符");
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
fn test_shm_slot_all_busy_recycles_oldest_slot() {
    let mut manager = CandidateSurfaceManager::new();
    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![waylan_core::Candidate::new("测试", "ce'shi", 100)],
        ..CandidateBoxState::default()
    };

    // 渲染四次填满所有四个槽位
    for _ in 0..4 {
        let _ = manager.render_state(&state);
    }

    // 人工模拟合成器尚未释放全部槽位（全槽皆 busy）
    for slot in manager.shm_slots.iter_mut().flatten() {
        slot.is_busy = true;
    }

    // 此时再次尝试渲染，应自愈降级轮转复用最旧槽位，杜绝丢帧导致候选框画面卡死
    let res = manager.render_state(&state);
    assert!(
        res.is_some(),
        "全槽皆 busy 时应自愈轮转复用最旧槽位完成渲染"
    );

    // 验证隐藏表面并清空无活跃缓冲后，pending_release_buffers 为空且状态一致
    let mut active_buf = None;
    manager.hide_wayland_surface(None, &mut active_buf);
    assert!(!manager.is_visible);
    assert!(manager.pending_release_buffers.is_empty());
}

#[test]
fn test_zero_copy_slot_mmap_writing() {
    let mut slot = ShmSlot::new(1024).expect("创建 ShmSlot");
    assert!(slot.mmap.is_some(), "ShmSlot 初始化必须具备有效内存映射");

    let rgba = vec![10, 20, 30, 255, 100, 150, 200, 128];
    CandidateSurfaceManager::write_rgba_to_slot(&mut slot, &rgba).expect("写入零拷贝 mmap");

    let mmap = slot.mmap.as_ref().unwrap();
    // 验证前两个像素是否已转换为 BGRA/ARGB8888 格式
    assert_eq!(&mmap[0..8], &[30, 20, 10, 255, 200, 150, 100, 128]);
}
