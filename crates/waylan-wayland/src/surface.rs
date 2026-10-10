use log::info;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use waylan_core::CandidateBoxState;
use waylan_ui::{CandidateBoxRenderer, UiTheme};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Dispatch, QueueHandle};

/// 候选框共享内存持久槽位（复用匿名文件描述符与预分配内存空间）
pub struct ShmSlot {
    /// 匿名共享内存文件
    pub file: File,
    /// 当前已向内核申请的文件容量（字节）
    pub capacity: usize,
    /// 标记当前槽位的缓冲区是否正被合成器读取占用
    pub is_busy: bool,
}

impl ShmSlot {
    /// 创建带有指定初始容量的共享内存槽位
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        let file = CandidateSurfaceManager::create_shm_fd(capacity)?;
        Ok(Self {
            file,
            capacity,
            is_busy: false,
        })
    }

    /// 确保文件容量至少达到目标字节数（容量不足时按 2 倍扩容）
    pub fn ensure_capacity(&mut self, required_bytes: usize) -> std::io::Result<()> {
        if self.capacity < required_bytes {
            let new_cap = required_bytes.max(self.capacity * 2).max(64 * 1024);
            self.file.set_len(new_cap as u64)?;
            self.capacity = new_cap;
        }
        Ok(())
    }
}

/// 焦点应用中的光标位置矩形 (Cursor Rectangle)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorRect {
    /// 焦点光标相对窗口左上角的 X 坐标
    pub x: i32,
    /// 焦点光标相对窗口左上角的 Y 坐标
    pub y: i32,
    /// 光标宽度（像素）
    pub width: i32,
    /// 光标高度（像素，通常为字符行高，如 18~24px）
    pub height: i32,
}

impl CursorRect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 计算输入法候选框的最佳悬浮锚定坐标（优先悬浮在光标正下方，留出 4px 间隙）
    pub fn compute_popup_position(
        &self,
        _popup_width: u32,
        popup_height: u32,
        screen_height: i32,
    ) -> (i32, i32) {
        let popup_h = popup_height as i32;
        let mut target_y = self.y + self.height + 4;

        // 如果超出屏幕下方，智能翻转至光标上方，防止遮挡文字
        if screen_height > 0 && (target_y + popup_h) > screen_height {
            target_y = (self.y - popup_h - 4).max(0);
        }

        let target_x = self.x;
        (target_x, target_y)
    }
}

/// 候选框透明悬浮图层管理器
pub struct CandidateSurfaceManager {
    /// 当前跟踪的光标几何坐标
    pub cursor_rect: CursorRect,
    /// 是否正处于显示状态
    pub is_visible: bool,
    /// 最近一次渲染的位图宽高
    pub current_size: (u32, u32),
    /// 候选框矢量渲染器实例
    pub renderer: CandidateBoxRenderer,
    /// 共享内存双缓冲槽位（交替使用以避免单缓冲画面撕裂与频繁创建文件）
    pub shm_slots: [Option<ShmSlot>; 2],
    /// 各槽位绑定的 Wayland 活跃缓冲区对象（等待 release 回调）
    pub slot_buffers: [Option<WlBuffer>; 2],
    /// 当前活跃槽位索引
    pub current_slot: usize,
    /// 复用的像素格式转换内存缓冲区（避免每次按键触发堆内存重新分配）
    pub pixel_buffer: Vec<u8>,
}

impl Default for CandidateSurfaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidateSurfaceManager {
    pub fn new() -> Self {
        Self {
            cursor_rect: CursorRect::default(),
            is_visible: false,
            current_size: (0, 0),
            renderer: CandidateBoxRenderer::new(UiTheme::light()),
            shm_slots: [None, None],
            slot_buffers: [None, None],
            current_slot: 0,
            pixel_buffer: Vec::with_capacity(64 * 1024),
        }
    }

    /// 确保指定槽位已经初始化并具备足够的容量
    fn ensure_slot(&mut self, slot_idx: usize, required_bytes: usize) -> std::io::Result<()> {
        if self.shm_slots[slot_idx].is_none() {
            let init_cap = required_bytes.max(64 * 1024);
            self.shm_slots[slot_idx] = Some(ShmSlot::new(init_cap)?);
        } else if let Some(slot) = &mut self.shm_slots[slot_idx] {
            slot.ensure_capacity(required_bytes)?;
        }
        Ok(())
    }

    /// 将 RGBA 像素流转换为 BGRA 格式并写入复用的内部缓冲区
    fn convert_rgba_to_pixel_buffer(&mut self, rgba: &[u8]) {
        self.pixel_buffer.clear();
        self.pixel_buffer.reserve(rgba.len());
        for chunk in rgba.chunks_exact(4) {
            self.pixel_buffer.push(chunk[2]); // B
            self.pixel_buffer.push(chunk[1]); // G
            self.pixel_buffer.push(chunk[0]); // R
            self.pixel_buffer.push(chunk[3]); // A
        }
    }

    /// 更新来自合成器的光标矩形位置
    pub fn update_cursor_rect(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.cursor_rect = CursorRect::new(x, y, width, height);
    }

    /// 当用户开始输入时，准备呈现候选框
    pub fn on_input_active(&mut self, width: u32, height: u32) {
        self.is_visible = true;
        self.current_size = (width, height);
        info!(
            "[悬浮窗口] 显示候选框图层，尺寸: {}x{}，吸附光标: ({}, {})",
            width, height, self.cursor_rect.x, self.cursor_rect.y
        );
    }

    /// 当输入上屏或清空缓冲区时，隐藏候选框
    pub fn on_input_inactive(&mut self) {
        if self.is_visible {
            self.is_visible = false;
            info!("[悬浮窗口] 隐藏候选框悬浮图层。");
        }
    }

    /// 响应合成器发射的 wl_buffer.release 事件，标记对应槽位为空闲并销毁旧 buffer
    pub fn on_buffer_released(&mut self, released_buffer: &WlBuffer) {
        for (idx, slot_buf) in self.slot_buffers.iter_mut().enumerate() {
            if let Some(buf) = slot_buf {
                if buf == released_buffer {
                    if let Some(ref mut slot) = self.shm_slots[idx] {
                        slot.is_busy = false;
                    }
                    buf.destroy();
                    *slot_buf = None;
                    break;
                }
            }
        }
    }

    /// 隐藏并卸载绑定的 Wayland 候选框表面缓冲
    pub fn hide_wayland_surface(
        &mut self,
        surface: Option<&WlSurface>,
        active_buffer: &mut Option<WlBuffer>,
    ) {
        self.on_input_inactive();
        if let Some(surf) = surface {
            surf.attach(None, 0, 0);
            surf.commit();
        }
        if let Some(old_buf) = active_buffer.take() {
            old_buf.destroy();
        }
        for (idx, slot_buf) in self.slot_buffers.iter_mut().enumerate() {
            if let Some(buf) = slot_buf.take() {
                buf.destroy();
            }
            if let Some(ref mut slot) = self.shm_slots[idx] {
                slot.is_busy = false;
            }
        }
    }

    /// 渲染候选框状态并将其通过 wl_shm 共享内存池挂载提交至 Wayland wl_surface
    pub fn commit_to_wayland<S>(
        &mut self,
        state: &CandidateBoxState,
        shm: Option<&WlShm>,
        surface: Option<&WlSurface>,
        qhandle: Option<&QueueHandle<S>>,
        active_buffer: &mut Option<WlBuffer>,
    ) where
        S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let Some((width, height, file)) = self.render_state(state) else {
            self.hide_wayland_surface(surface, active_buffer);
            return;
        };

        if let (Some(shm), Some(surf), Some(qh)) = (shm, surface, qhandle) {
            let stride = (width * 4) as i32;
            let pool_size = stride * (height as i32);
            let pool = shm.create_pool(file.as_fd(), pool_size, qh, ());
            let buffer = pool.create_buffer(
                0,
                width as i32,
                height as i32,
                stride,
                wl_shm::Format::Argb8888,
                qh,
                (),
            );
            pool.destroy();

            let old_slot_buf = self.slot_buffers[self.current_slot].take();
            if let Some(old) = old_slot_buf {
                old.destroy();
            }
            self.slot_buffers[self.current_slot] = Some(buffer.clone());
            if let Some(ref mut slot) = self.shm_slots[self.current_slot] {
                slot.is_busy = true;
            }

            if let Some(old_buf) = active_buffer.take() {
                old_buf.destroy();
            }

            surf.attach(Some(&buffer), 0, 0);
            surf.damage(0, 0, width as i32, height as i32);
            surf.commit();
            *active_buffer = Some(buffer);
        }
    }

    /// 根据当前候选框状态渲染位图并复用共享内存槽位准备提交
    pub fn render_state(&mut self, state: &CandidateBoxState) -> Option<(u32, u32, &File)> {
        if !state.visible || state.candidates.is_empty() {
            self.on_input_inactive();
            return None;
        }

        let pixmap = self.renderer.render(state)?;
        let width = pixmap.width();
        let height = pixmap.height();
        self.on_input_active(width, height);

        // 1. 复用内部内存缓冲完成 RGBA -> BGRA 格式转换
        self.convert_rgba_to_pixel_buffer(pixmap.data());
        let required_bytes = self.pixel_buffer.len();

        // 2. 轮换至下一个空闲持久化槽位（双缓冲交替，优先选择合成器已 release 的空闲槽位）
        let mut next_slot = (self.current_slot + 1) % self.shm_slots.len();
        for offset in 1..=self.shm_slots.len() {
            let candidate = (self.current_slot + offset) % self.shm_slots.len();
            if let Some(ref slot) = self.shm_slots[candidate] {
                if !slot.is_busy {
                    next_slot = candidate;
                    break;
                }
            } else {
                next_slot = candidate;
                break;
            }
        }

        self.ensure_slot(next_slot, required_bytes).ok()?;
        self.current_slot = next_slot;

        // 3. 复用已有文件描述符写入像素数据，消除频繁的 tempfile() 系统调用
        let slot = self.shm_slots[self.current_slot].as_mut()?;
        slot.file.seek(SeekFrom::Start(0)).ok()?;
        slot.file.write_all(&self.pixel_buffer).ok()?;

        Some((width, height, &slot.file))
    }

    /// 将 tiny-skia 预乘 RGBA 字节流转换为 Wayland WL_SHM_FORMAT_ARGB8888 小端序 (BGRA) 字节流
    pub fn rgba_to_argb8888_le(rgba: &[u8]) -> Vec<u8> {
        let mut bgra = Vec::with_capacity(rgba.len());
        for chunk in rgba.chunks_exact(4) {
            bgra.push(chunk[2]); // B
            bgra.push(chunk[1]); // G
            bgra.push(chunk[0]); // R
            bgra.push(chunk[3]); // A
        }
        bgra
    }

    /// 将 RGBA/BGRA 像素位图写入匿名内存映射文件（用于跨进程传递给 Wayland 合成器）
    pub fn create_shm_fd(size: usize) -> std::io::Result<std::fs::File> {
        let file = tempfile::tempfile()?;
        file.set_len(size as u64)?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn test_rgba_to_argb8888_le_conversion() {
        // 输入两个像素: [R=10, G=20, B=30, A=255] 与 [R=100, G=150, B=200, A=128]
        let rgba = vec![10, 20, 30, 255, 100, 150, 200, 128];
        let bgra = CandidateSurfaceManager::rgba_to_argb8888_le(&rgba);
        assert_eq!(bgra, vec![30, 20, 10, 255, 200, 150, 100, 128]);
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
}
