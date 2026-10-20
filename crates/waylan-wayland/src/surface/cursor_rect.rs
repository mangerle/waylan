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

/// 候选框悬浮位置计算参数
///
/// 承载候选框自身尺寸与屏幕可用区域约束，供悬浮锚点坐标计算使用。
/// 采用参数对象聚合可避免长参数列表在调用处发生参数错位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupPositionParams {
    /// 候选框宽度（像素）
    pub popup_width: u32,
    /// 候选框高度（像素）
    pub popup_height: u32,
    /// 屏幕逻辑宽度（像素；非正值表示不约束水平方向）
    pub screen_width: i32,
    /// 屏幕逻辑高度（像素；非正值表示不约束垂直方向）
    pub screen_height: i32,
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
        self.compute_popup_position_constrained(&PopupPositionParams {
            popup_width,
            popup_height,
            screen_width: 0,
            screen_height,
        })
    }

    /// 在指定屏幕宽度与高度约束下计算候选框悬浮坐标（自动执行水平右边缘防溢出平移与垂直下边缘向上翻转）
    pub fn compute_popup_position_constrained(&self, params: &PopupPositionParams) -> (i32, i32) {
        let PopupPositionParams {
            popup_width,
            popup_height,
            screen_width,
            screen_height,
        } = *params;
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
