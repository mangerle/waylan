//! 微澜输入法前端显示服务与协议解耦抽象契约
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1、Wayland V2 与 IBus 等多显示服务协议对核心输入法会话状态机的双向耦合。
//! - **核心优势**：依赖倒置原则（DIP）；协议适配层只需实现本契约接口，即可零成本接入微澜核心算法与语音交互，
//!   实现多协议端按键分发与界面刷新行为的 100% 绝对一致。
//! - **代价与局限**：协议层需要轻量桥接具体的上屏或候选框刷新指令。

use waylan_core::{CandidateBoxState, InputMode};

/// 微澜输入法前端显示服务契约接口 (Input Method Frontend)
pub trait InputMethodFrontend {
    /// 向当前获得焦点的客户端应用程序提交上屏文本
    fn commit_text(&mut self, text: &str);

    /// 更新客户端文本框内的预编辑文本与光标字节偏移
    fn update_preedit(&mut self, text: &str, cursor_byte_idx: usize);

    /// 刷新并呈现候选词浮动窗口/气泡状态
    fn update_candidates(&mut self, state: &CandidateBoxState);

    /// 隐藏候选词窗口表面
    fn hide_candidates(&mut self);

    /// 模拟方向键或向合成器请求移动文本光标相对偏移（负数向左，正数向右）
    fn move_cursor(&mut self, offset: i32);

    /// 通知前端中英文输入模式发生了切换
    fn sync_input_mode(&mut self, mode: InputMode);
}
