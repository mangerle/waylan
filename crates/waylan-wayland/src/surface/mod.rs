//! 候选框透明悬浮表面与渲染管线模块
//!
//! 负责 Wayland 候选框图层位置计算、多缓冲共享内存管理与矢量位图提交。

pub mod cursor_rect;
pub mod interaction;
pub mod shm_slot;
pub mod state_builder;

pub use cursor_rect::CursorRect;
pub(crate) use shm_slot::ShmSlot;
pub use state_builder::*;

use log::{debug, warn};
use std::fs::File;
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
    pub(crate) shm_slots: [Option<ShmSlot>; 4],
    /// 各槽位绑定的 Wayland 活跃缓冲区对象（等待 release 回调）
    pub(crate) slot_buffers: [Option<WlBuffer>; 4],
    /// 当前活跃槽位索引
    pub(crate) current_slot: usize,
    /// 目标显示器逻辑分辨率 (宽, 高)，用于贴边防溢出平移与向上翻转
    pub screen_size: (i32, i32),
    /// 待合成器 release 的退役缓冲区列表（附带所属槽位索引，防止隐藏表面后 is_busy 永久锁死）
    pub(crate) pending_release_buffers: Vec<(usize, WlBuffer)>,
}

impl Default for CandidateSurfaceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 待合成器 release 的退役缓冲区队列最大容量上限（防止异常合成器丢事件引发内存与对象 ID 泄漏）
const MAX_PENDING_RELEASE_BUFFERS: usize = 16;

impl CandidateSurfaceManager {
    /// 安全暂存等待 release 的退役缓冲区，达到上限时主动销毁最旧的代理对象防止泄漏
    fn push_pending_release(&mut self, slot_idx: usize, buffer: WlBuffer) {
        if self.pending_release_buffers.len() >= MAX_PENDING_RELEASE_BUFFERS {
            let (old_slot, oldest_buf) = self.pending_release_buffers.remove(0);
            warn!(
                "[悬浮窗口] 退役缓冲区队列达到上限 ({})，主动销毁最旧退役缓冲（槽位: {}）。",
                MAX_PENDING_RELEASE_BUFFERS, old_slot
            );
            oldest_buf.destroy();
        }
        self.pending_release_buffers.push((slot_idx, buffer));
    }

    /// 创建默认候选框悬浮表面管理器实例
    ///
    /// # 设计原理
    /// - **实现初衷**：统一管理 Wayland 输入法候选框的矢量栅格化、光标跟随、防溢出与多缓冲共享内存交换。
    /// - **核心优势**：预分配 4 缓冲环形槽位配合延迟按需分配，零开销冷启动，且杜绝每帧重复创建文件描述符与内存映射。
    /// - **代价与局限**：维护多缓冲生命周期状态机，需精确跟踪合成器下发的 release 事件与容量扩容。
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

    /// 清理绑定到旧 Wayland 连接的池与缓冲区代理资源（保留底层 memfd 文件与渲染器缓存，供会话重连或降级复用）
    pub fn clear_wayland_resources(&mut self) {
        for slot in self.shm_slots.iter_mut().flatten() {
            slot.is_busy = false;
            if let Some((pool, _)) = slot.pool.take() {
                pool.destroy();
            }
        }
        for buf in self.slot_buffers.iter_mut() {
            if let Some(b) = buf.take() {
                b.destroy();
            }
        }
        for (_, b) in self.pending_release_buffers.drain(..) {
            b.destroy();
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
        for idx in 0..self.slot_buffers.len() {
            let is_busy = self.shm_slots[idx]
                .as_ref()
                .map(|s| s.is_busy)
                .unwrap_or(false);
            if !is_busy {
                if let Some(buf) = self.slot_buffers[idx].take() {
                    buf.destroy();
                }
            } else if let Some(buf) = self.slot_buffers[idx].take() {
                self.push_pending_release(idx, buf);
            }
        }
    }

    /// 从指定 SHM 槽位持久复用的 WlShmPool 中分配或扩容创建 WlBuffer
    fn acquire_slot_buffer<S>(
        slot: &mut ShmSlot,
        shm: &WlShm,
        qh: &QueueHandle<S>,
        dims: (u32, u32, i32),
    ) -> WlBuffer
    where
        S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let (aligned_w, aligned_h, stride) = dims;
        let min_pool_bytes = (stride.saturating_mul(aligned_h as i32)).max(1) as usize;
        let target_pool_size = slot.capacity.max(min_pool_bytes);
        match slot.pool {
            Some((ref pool, ref mut mapped_size)) => {
                if *mapped_size < target_pool_size {
                    pool.resize(target_pool_size as i32);
                    *mapped_size = target_pool_size;
                }
                pool.create_buffer(
                    0,
                    aligned_w as i32,
                    aligned_h as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                    qh,
                    (),
                )
            }
            None => {
                let pool = shm.create_pool(slot.file.as_fd(), target_pool_size as i32, qh, ());
                let buf = pool.create_buffer(
                    0,
                    aligned_w as i32,
                    aligned_h as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                    qh,
                    (),
                );
                slot.pool = Some((pool, target_pool_size));
                buf
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
        let Some((width, height, _)) = self.render_state(state) else {
            self.hide_wayland_surface(surface, active_buffer);
            return;
        };

        if let (Some(shm), Some(surf), Some(qh)) = (shm, surface, qhandle) {
            let aligned_w = (width / scale_u).max(1).saturating_mul(scale_u);
            let aligned_h = (height / scale_u).max(1).saturating_mul(scale_u);
            let stride = (width as i32).saturating_mul(4);

            let Some(ref mut slot) = self.shm_slots[self.current_slot] else {
                return;
            };
            let is_slot_busy = slot.is_busy;
            let buffer = Self::acquire_slot_buffer(slot, shm, qh, (aligned_w, aligned_h, stride));
            slot.is_busy = true;

            if let Some(old) = self.slot_buffers[self.current_slot].take() {
                if !is_slot_busy {
                    old.destroy();
                } else {
                    self.push_pending_release(self.current_slot, old);
                }
            }
            self.slot_buffers[self.current_slot] = Some(buffer.clone());

            surf.set_buffer_scale(scale);
            surf.attach(Some(&buffer), 0, 0);
            let logical_w = (aligned_w / scale_u) as i32;
            let logical_h = (aligned_h / scale_u) as i32;
            surf.damage(0, 0, logical_w, logical_h);
            surf.commit();
            *active_buffer = Some(buffer);
        }
    }

    /// 轮转选取可用于本次渲染的共享内存槽位下标（优先选取非 busy 或尚未分配的槽位）
    fn select_available_slot(&self) -> usize {
        for i in 0..self.shm_slots.len() {
            let candidate = (self.current_slot + 1 + i) % self.shm_slots.len();
            match self.shm_slots[candidate].as_ref() {
                Some(slot) if slot.is_busy => continue,
                _ => return candidate,
            }
        }
        debug!(
            "[悬浮窗口] 合成器未及时下发 release 事件，轮转复用最旧槽位保证候选框画面实时刷新。"
        );
        (self.current_slot + 1) % self.shm_slots.len()
    }

    /// 确保目标槽位可用于本次渲染：必要时重新分配独立槽位并完成容量扩容
    ///
    /// # 设计原理
    /// - **实现初衷**：合成器读取共享内存期间槽位处于 busy 状态，若直接覆写会导致画面撕裂。
    /// - **核心优势**：将「避让忙槽位」与「容量扩容」收敛于同一处，渲染主流程保持线性可读。
    /// - **代价与局限**：忙槽位需等待 release 事件才能真正释放，此处仅做规避与重分配。
    fn prepare_slot_for_render(&mut self, next_slot: usize, required_bytes: usize) -> bool {
        // 若所选槽位仍被合成器占用，为避免覆盖正在被合成读取的共享内存引发撕裂，重新分配独立槽位
        let is_slot_busy = self.shm_slots[next_slot]
            .as_ref()
            .map(|s| s.is_busy)
            .unwrap_or(false);
        if is_slot_busy {
            debug!(
                "[悬浮窗口] 槽位 {} 仍被合成器占用，重新分配独立共享内存槽位以杜绝数据竞争撕裂。",
                next_slot
            );
            // 将正在被合成器占用的旧缓冲移入待释放队列，等待 release 事件后再安全销毁
            if let Some(old_buffer) = self.slot_buffers[next_slot].take() {
                self.push_pending_release(next_slot, old_buffer);
            }
            if let Ok(new_slot) = ShmSlot::new(required_bytes.max(64 * 1024)) {
                self.shm_slots[next_slot] = Some(new_slot);
            }
        }

        if let Err(e) = self.ensure_slot(next_slot, required_bytes) {
            warn!(
                "[悬浮窗口] 扩容共享内存槽位失败，槽位: {}，原因: {}",
                next_slot, e
            );
            return false;
        }
        self.current_slot = next_slot;
        true
    }

    /// 根据当前候选框状态渲染位图并复用共享内存槽位准备提交
    pub fn render_state(&mut self, state: &CandidateBoxState) -> Option<(u32, u32, &File)> {
        if !state.visible || state.candidates.is_empty() {
            self.on_input_inactive();
            return None;
        }

        // 1. 调用矢量渲染器绘制候选框位图（复用渲染器内部像素缓冲，消除跨帧大块内存重分配）
        let (width, height, _) = self.renderer.render_rgba(state)?;
        self.on_input_active(width, height);

        let required_bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);

        // 2. 轮转选取空闲的共享内存槽位并完成容量准备
        let next_slot = self.select_available_slot();
        if !self.prepare_slot_for_render(next_slot, required_bytes) {
            return None;
        }

        // 3. 复用持久映射句柄零拷贝写入像素数据，消除中间堆内存中转与重复 seek+write_all 内核拷贝
        let rgba = self.renderer.last_rgba_slice(required_bytes)?;
        let slot = self.shm_slots[self.current_slot].as_mut()?;
        if Self::write_rgba_to_slot(slot, rgba).is_none() {
            warn!(
                "[悬浮窗口] 写入候选框共享内存失败，槽位: {}",
                self.current_slot
            );
            return None;
        }

        Some((width, height, &slot.file))
    }

    /// 将 RGBA 像素流直接写入槽位的共享内存映射中（委托至 ShmSlot）
    #[inline]
    pub(crate) fn write_rgba_to_slot(slot: &mut ShmSlot, rgba: &[u8]) -> Option<()> {
        slot.write_rgba(rgba)
    }

    /// 创建匿名共享内存文件（委托至 shm_slot 模块）
    ///
    /// # 设计原理
    /// - **实现初衷**：为上层应用或测试提供创建兼容 Wayland wl_shm 协议的标准匿名内存描述符工具方法。
    /// - **核心优势**：自动适配 Linux `memfd_create` 纯内存文件与跨平台临时文件回退。
    /// - **代价与局限**：创建并返回未映射的原始系统文件描述符，调用方需自行维护其生命周期。
    ///
    /// # Errors
    /// 当创建或调整文件大小失败时返回系统 I/O 错误。
    pub fn create_shm_fd(size: usize) -> std::io::Result<File> {
        shm_slot::create_shm_fd(size)
    }
}

#[cfg(test)]
#[path = "../surface_tests.rs"]
mod tests;
