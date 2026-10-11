use log::debug;
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
            let new_cap = required_bytes
                .max(self.capacity.saturating_mul(2))
                .max(64 * 1024);
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
        popup_width: u32,
        popup_height: u32,
        screen_height: i32,
    ) -> (i32, i32) {
        self.compute_popup_position_constrained(popup_width, popup_height, 0, screen_height)
    }

    /// 在指定屏幕宽度与高度约束下计算候选框悬浮坐标（自动执行水平右边缘防溢出平移与垂直下边缘向上翻转）
    pub fn compute_popup_position_constrained(
        &self,
        popup_width: u32,
        popup_height: u32,
        screen_width: i32,
        screen_height: i32,
    ) -> (i32, i32) {
        let popup_w = popup_width as i32;
        let popup_h = popup_height as i32;

        let mut target_y = self.y + self.height + 4;
        // 如果超出屏幕下方，智能翻转至光标上方，防止遮挡文字
        if screen_height > 0 && (target_y + popup_h) > screen_height {
            target_y = (self.y - popup_h - 4).max(0);
        }

        let mut target_x = self.x;
        // 如果超出屏幕右边缘，自动向左平移留出 4px 安全边距，防止候选框被屏幕边缘截断
        if screen_width > 0 && (target_x + popup_w) > screen_width {
            target_x = (screen_width - popup_w - 4).max(0);
        }

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
    /// 当前 HiDPI 缩放因子（Wayland 整数缩放，默认 1）
    pub scale: i32,
    /// 候选框矢量渲染器实例
    pub renderer: CandidateBoxRenderer,
    /// 共享内存环形多缓冲槽位（交替使用以避免单缓冲画面撕裂与频繁创建文件）
    pub shm_slots: [Option<ShmSlot>; 4],
    /// 各槽位绑定的 Wayland 活跃缓冲区对象（等待 release 回调）
    pub slot_buffers: [Option<WlBuffer>; 4],
    /// 当前活跃槽位索引
    pub current_slot: usize,
    /// 复用的像素格式转换内存缓冲区（避免每次按键触发堆内存重新分配）
    pub pixel_buffer: Vec<u8>,
    /// 目标显示器逻辑分辨率 (宽, 高)，用于贴边防溢出平移与向上翻转
    pub screen_size: (i32, i32),
    /// 待合成器 release 的退役缓冲区列表（附带所属槽位索引，防止隐藏表面后 is_busy 永久锁死）
    pub pending_release_buffers: Vec<(usize, WlBuffer)>,
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
            scale: 1,
            renderer: CandidateBoxRenderer::new(UiTheme::light()),
            shm_slots: [None, None, None, None],
            slot_buffers: [None, None, None, None],
            current_slot: 0,
            pixel_buffer: Vec::with_capacity(64 * 1024),
            screen_size: (1920, 1080),
            pending_release_buffers: Vec::with_capacity(4),
        }
    }

    /// 设置 HiDPI 缩放比例并同步更新矢量渲染器
    pub fn set_scale(&mut self, scale: i32) {
        let scale = scale.max(1);
        self.scale = scale;
        self.renderer.set_scale(scale as f32);
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
        let len = rgba.len();
        self.pixel_buffer.resize(len, 0);
        let src_chunks = rgba.chunks_exact(4);
        let dst_chunks = self.pixel_buffer.chunks_exact_mut(4);
        for (src, dst) in src_chunks.zip(dst_chunks) {
            dst[0] = src[2]; // B
            dst[1] = src[1]; // G
            dst[2] = src[0]; // R
            dst[3] = src[3]; // A
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
        debug!(
            "[悬浮窗口] 显示候选框图层，尺寸: {}x{}，吸附光标: ({}, {})",
            width, height, self.cursor_rect.x, self.cursor_rect.y
        );
    }

    /// 当输入上屏或清空缓冲区时，隐藏候选框
    pub fn on_input_inactive(&mut self) {
        if self.is_visible {
            self.is_visible = false;
            debug!("[悬浮窗口] 隐藏候选框悬浮图层。");
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
                    return;
                }
            }
        }

        // 检查退役缓冲区队列中等待 release 的旧缓冲，若该槽位当前无新活跃缓冲则复位 is_busy
        if let Some(pos) = self
            .pending_release_buffers
            .iter()
            .position(|(_, b)| b == released_buffer)
        {
            let (slot_idx, buf) = self.pending_release_buffers.remove(pos);
            if self.slot_buffers[slot_idx].is_none() {
                if let Some(ref mut slot) = self.shm_slots[slot_idx] {
                    slot.is_busy = false;
                }
            }
            buf.destroy();
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
        // active_buffer 与 slot_buffers 引用同一底层 WlBuffer，统一由 slot_buffers 执行销毁防止重复销毁
        *active_buffer = None;
        for (idx, slot_buf) in self.slot_buffers.iter_mut().enumerate() {
            let is_busy = self.shm_slots[idx]
                .as_ref()
                .map(|s| s.is_busy)
                .unwrap_or(false);
            if !is_busy {
                if let Some(buf) = slot_buf.take() {
                    buf.destroy();
                }
            } else if let Some(buf) = slot_buf.take() {
                self.pending_release_buffers.push((idx, buf));
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
        let scale = self.scale.max(1);
        let scale_u = scale as u32;
        let Some((width, height, file)) = self.render_state(state) else {
            self.hide_wayland_surface(surface, active_buffer);
            return;
        };

        if let (Some(shm), Some(surf), Some(qh)) = (shm, surface, qhandle) {
            let aligned_w = (width / scale_u).max(1).saturating_mul(scale_u);
            let aligned_h = (height / scale_u).max(1).saturating_mul(scale_u);
            let stride = (width as i32).saturating_mul(4);
            let pool_size = stride.saturating_mul(height as i32);
            let pool = shm.create_pool(file.as_fd(), pool_size, qh, ());
            let buffer = pool.create_buffer(
                0,
                aligned_w as i32,
                aligned_h as i32,
                stride,
                wl_shm::Format::Argb8888,
                qh,
                (),
            );
            pool.destroy();

            let is_slot_busy = self.shm_slots[self.current_slot]
                .as_ref()
                .map(|s| s.is_busy)
                .unwrap_or(false);
            let old_slot_buf = self.slot_buffers[self.current_slot].take();
            if let Some(old) = old_slot_buf {
                if !is_slot_busy {
                    old.destroy();
                } else {
                    self.pending_release_buffers.push((self.current_slot, old));
                }
            }
            self.slot_buffers[self.current_slot] = Some(buffer.clone());
            if let Some(ref mut slot) = self.shm_slots[self.current_slot] {
                slot.is_busy = true;
            }

            surf.set_buffer_scale(scale);
            surf.attach(Some(&buffer), 0, 0);
            let logical_w = (aligned_w / scale_u) as i32;
            let logical_h = (aligned_h / scale_u) as i32;
            surf.damage(0, 0, logical_w, logical_h);
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

        // 2. 轮换至下一个空闲持久化槽位（双缓冲交替，严格检查合成器已 release 的空闲槽位）
        let mut available_slot = None;
        for offset in 1..=self.shm_slots.len() {
            let candidate = (self.current_slot + offset) % self.shm_slots.len();
            if let Some(ref slot) = self.shm_slots[candidate] {
                if !slot.is_busy {
                    available_slot = Some(candidate);
                    break;
                }
            } else {
                available_slot = Some(candidate);
                break;
            }
        }

        let next_slot = match available_slot {
            Some(slot) => slot,
            None => {
                log::debug!("[悬浮窗口] 合成器未及时下发 release 事件，轮转复用最旧槽位保证候选框画面实时刷新。");
                (self.current_slot + 1) % self.shm_slots.len()
            }
        };

        if let Err(e) = self.ensure_slot(next_slot, required_bytes) {
            log::warn!(
                "[悬浮窗口] 扩容共享内存槽位失败，槽位: {}，原因: {}",
                next_slot,
                e
            );
            return None;
        }
        self.current_slot = next_slot;

        // 3. 复用已有文件描述符写入像素数据，消除频繁的 tempfile() 系统调用
        let slot = self.shm_slots[self.current_slot].as_mut()?;
        if let Err(e) = slot
            .file
            .seek(SeekFrom::Start(0))
            .and_then(|_| slot.file.write_all(&self.pixel_buffer))
        {
            log::warn!(
                "[悬浮窗口] 写入候选框共享内存失败，槽位: {}，原因: {}",
                self.current_slot,
                e
            );
            return None;
        }

        Some((width, height, &slot.file))
    }

    /// 将 RGBA/BGRA 像素位图写入匿名内存映射文件（用于跨进程传递给 Wayland 合成器）
    /// 在 Linux 环境优先使用纯内存文件描述符 (memfd_create)，失败时平滑降级至 tempfile
    pub fn create_shm_fd(size: usize) -> std::io::Result<std::fs::File> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{memfd_create, MemfdFlags};
            if let Ok(fd) = memfd_create(
                "waylan-candidate-shm",
                MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
            ) {
                let file = std::fs::File::from(fd);
                file.set_len(size as u64)?;
                return Ok(file);
            }
        }
        let file = tempfile::tempfile()?;
        file.set_len(size as u64)?;
        Ok(file)
    }
}

#[cfg(test)]
#[path = "surface_tests.rs"]
mod tests;
