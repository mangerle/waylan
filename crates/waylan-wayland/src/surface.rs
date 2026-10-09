use log::info;

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
    pub fn compute_popup_position(&self, _popup_width: u32, popup_height: u32, screen_height: i32) -> (i32, i32) {
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
}
