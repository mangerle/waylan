//! 候选框悬浮表面交互定位与命中测试扩展
//!
//! # 设计原理
//! - **实现初衷**：将光标位置跟踪、候选框几何位置防溢出计算、显示/隐藏生命周期以及鼠标点击命中测试从表面管理器中剥离。
//! - **核心优势**：职责单一（SRP），消除渲染提交流程与几何交互定位之间的耦合；
//!   便于独立对多显示器边缘碰撞与命中测试进行单元测试。
//! - **代价与局限**：需由表面管理器持有交互状态对象或通过具名方法调用。

use log::debug;
use waylan_ui::CandidateHitTarget;

use super::cursor_rect::PopupPositionParams;
use super::{CandidateSurfaceManager, CursorRect};

/// 光标矩形更新参数
///
/// 承载合成器下发的焦点光标几何信息，用于刷新候选框悬浮锚点位置。
/// 采用参数对象聚合可避免长参数列表在调用处发生参数错位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorRectUpdateParams {
    /// 焦点光标相对窗口左上角的 X 坐标
    pub x: i32,
    /// 焦点光标相对窗口左上角的 Y 坐标
    pub y: i32,
    /// 光标宽度（像素）
    pub width: i32,
    /// 光标高度（像素，通常为字符行高，如 18~24px）
    pub height: i32,
}

impl CandidateSurfaceManager {
    /// 更新当前活动显示器的逻辑分辨率 (宽, 高)
    pub fn update_screen_size(&mut self, width: i32, height: i32) {
        if width > 0 && height > 0 {
            self.screen_size = (width, height);
        }
    }

    /// 基于当前跟踪的光标位置与屏幕逻辑分辨率计算候选框最佳悬浮坐标
    #[inline]
    pub fn compute_popup_position(&self, popup_width: u32, popup_height: u32) -> (i32, i32) {
        self.cursor_rect
            .compute_popup_position_constrained(&PopupPositionParams {
                popup_width,
                popup_height,
                screen_width: self.screen_size.0,
                screen_height: self.screen_size.1,
            })
    }

    /// 更新来自合成器的光标矩形位置
    pub fn update_cursor_rect(&mut self, params: &CursorRectUpdateParams) {
        self.cursor_rect = CursorRect::new(params.x, params.y, params.width, params.height);
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

    /// 对候选框矢量图层执行鼠标点击命中测试（展开折叠、选词、翻页）
    #[inline]
    pub fn hit_test(&self, x: f32, y: f32) -> Option<CandidateHitTarget> {
        self.renderer.hit_test(x, y)
    }
}
