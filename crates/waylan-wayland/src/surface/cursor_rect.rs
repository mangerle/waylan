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
