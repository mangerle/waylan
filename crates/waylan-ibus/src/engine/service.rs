//! IBus 核心引擎服务 D-Bus 接口实现
//!
//! # 设计原理
//! - **实现初衷**：实现 `org.freedesktop.IBus.Engine` 规范，接收按键、选词、焦点事件，并通过 D-Bus 信号驱动客户端。
//! - **核心优势**：纯内存瞬时加锁变更状态并提取不可变快照，所有信号发送在释放互斥锁后异步执行。
//! - **代价与局限**：必须严格遵循 IBus D-Bus 信号契约。

use log::{debug, info};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use waylan_sys::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use super::super::keys::IbusSessionCore;
use super::super::screensaver::ScreenLockHealer;
use super::super::types::{
    build_ibus_prop_list, extract_text_from_ibus_variant, IbusPropListParams,
};
use super::emitter::OutcomeDispatchParams;
use super::snapshot::UiSnapshot;
use crate::IbusMenuActionHandler;

/// IBus 输入框用途常量：密码框 (`IBUS_INPUT_PURPOSE_PASSWORD`)
pub(crate) const IBUS_INPUT_PURPOSE_PASSWORD: u32 = 2;

/// IBus 输入框用途常量：PIN 码框 (`IBUS_INPUT_PURPOSE_PIN`)
pub(crate) const IBUS_INPUT_PURPOSE_PIN: u32 = 3;

/// `org.freedesktop.IBus.Engine` 核心接口实现
#[derive(Clone)]
pub struct IbusEngineService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
    pub(crate) reload_requested: Option<Arc<AtomicBool>>,
    pub(crate) loop_wakeup: Option<LoopWakeup>,
    pub(crate) menu_handler: Option<Arc<dyn IbusMenuActionHandler>>,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) suppress_properties: bool,
    pub(crate) is_password: Arc<AtomicBool>,
    pub(crate) lock_healer: Arc<ScreenLockHealer>,
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl IbusEngineService {
    /// 处理硬件按键按下与释放事件
    ///
    /// 入参由 `org.freedesktop.IBus.Engine.ProcessKeyEvent(keyval, keycode, state)` 协议契约固定，
    /// `ctxt` 为 zbus 信号上下文注入项，均不可收敛为参数对象。
    async fn process_key_event(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> bool {
        let (outcome, snap, is_caps_locked) = {
            let mut guard = self.core.lock();
            guard.active_engine_path = Some(ctxt.path().to_string());
            guard.is_password_context = self.is_password.load(Ordering::Acquire);

            // 按键级锁屏直通残留即时自愈
            self.lock_healer.maybe_heal_on_key(
                guard.is_password_context,
                &mut guard.is_screen_locked,
                self.loop_wakeup.as_ref(),
            );

            let prev_voice = guard.is_voice_active();
            let out = guard.process_key_event(keyval, keycode, state);
            // 语音状态变迁或输入模式切换时即刻唤醒事件循环，杜绝图标与属性刷新的事件延迟
            if guard.is_voice_active() != prev_voice || out.mode_changed.is_some() {
                if let Some(ref wakeup) = self.loop_wakeup {
                    wakeup.wake();
                }
            }
            let snap = if out.update_ui {
                Some(UiSnapshot::capture(&guard))
            } else {
                None
            };
            (out, snap, guard.is_caps_locked())
        };

        let consumed = outcome.consumed;
        self.dispatch_outcome(OutcomeDispatchParams {
            ctxt: &ctxt,
            outcome,
            snap,
            is_caps_locked,
        })
        .await;
        consumed
    }

    /// 更新光标屏幕几何坐标
    ///
    /// 入参由 `org.freedesktop.IBus.Engine.SetCursorLocation(x, y, w, h)` 协议契约固定，不可收敛为参数对象。
    fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    /// 手写输入坐标事件（保持空实现以兼容协议）
    fn process_hand_writing_event(&self, _coordinates: Vec<f64>) {}

    /// 取消手写输入事件（保持空实现以兼容协议）
    fn cancel_hand_writing(&self, _n_strokes: u32) {}

    /// 设置客户端支持的输入法能力位掩码（IBus 协议方法，自动同步密码框感知标记）
    pub(crate) fn set_capabilities(&self, caps: u32) {
        const IBUS_CAP_PASSWORD: u32 = 1 << 4;
        let is_pwd = (caps & IBUS_CAP_PASSWORD) != 0;
        self.update_password_state(is_pwd);
    }

    /// 响应 GNOME / IBus 原生状态栏菜单项激活事件
    async fn property_activate(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        name: &str,
        _state: u32,
    ) {
        self.handle_property_activate(&ctxt, name).await;
    }

    fn property_show(&self, _name: &str) {}

    fn property_hide(&self, _name: &str) {}

    /// 响应用户在 GNOME 候选框上使用鼠标点击候选词事件
    ///
    /// 入参由 `org.freedesktop.IBus.Engine.CandidateClicked(index, button, state)` 协议契约固定，
    /// 不可收敛为参数对象。
    async fn candidate_clicked(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        index: u32,
        _button: u32,
        _state: u32,
    ) {
        self.handle_candidate_clicked(&ctxt, index).await;
    }

    /// 焦点进入输入上下文
    async fn focus_in(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        debug!(
            "[微澜 IBus] 输入上下文获得焦点 (FocusIn): 路径={}",
            ctxt.path()
        );
        let (mode, is_caps, trad, punct, char_full, snap) = {
            let mut guard = self.core.lock();
            guard.active_engine_path = Some(ctxt.path().to_string());
            // 焦点进入新输入上下文时同步当前 Engine 实例记录的密码框状态，
            // 既杜绝旧普通窗口残留密码标记，又防止因先触发 SetContentType 后触发 FocusIn 导致密码标记被误冲刷
            guard.is_password_context = self.is_password.load(Ordering::Acquire);
            // 自愈保护：若当前获焦的并非密码框，而内部仍记录为锁屏状态，自动解除锁屏直通
            self.lock_healer.heal_on_focus(
                guard.is_password_context,
                &mut guard.is_screen_locked,
                self.loop_wakeup.as_ref(),
            );
            let _ = guard.sync_shared_mode();
            let trad = guard.runtime.core.engine.config.traditional_chinese;
            let punct = guard.runtime.core.is_full_width_punct();
            let char_full = guard.is_full_width_char();
            (
                guard.runtime.core.input_mode(),
                guard.is_caps_locked(),
                trad,
                punct,
                char_full,
                UiSnapshot::capture(&guard),
            )
        };
        if !self.suppress_properties {
            if let Ok(prop_list) = build_ibus_prop_list(&IbusPropListParams {
                mode,
                is_caps_locked: is_caps,
                traditional_chinese: trad,
                full_width_punct: punct,
                full_width_char: char_full,
            }) {
                let _ = Self::register_properties(&ctxt, Value::from(prop_list)).await;
            }
        }
        Self::emit_ui_snapshot(&ctxt, &snap).await;
    }

    /// 带对象标识的焦点进入事件
    async fn focus_in_id(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        _object_path: &str,
        _client: &str,
    ) {
        self.focus_in(ctxt).await;
    }

    /// 焦点离开输入上下文
    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        debug!(
            "[微澜 IBus] 输入上下文失去焦点 (FocusOut): 路径={}",
            ctxt.path()
        );
        self.reset_and_hide_ui(&ctxt).await;
    }

    /// 带对象标识的焦点离开事件
    async fn focus_out_id(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        _object_path: &str,
    ) {
        self.reset_and_hide_ui(&ctxt).await;
    }

    /// 重置输入上下文
    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.reset_and_hide_ui(&ctxt).await;
    }

    /// 启用微澜输入法引擎
    async fn enable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.focus_in(ctxt).await;
    }

    /// 禁用微澜输入法引擎
    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.reset_and_hide_ui(&ctxt).await;
    }

    /// 响应候选框上一页按钮事件
    async fn page_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.handle_page_up(&ctxt).await;
    }

    /// 响应候选框下一页按钮事件
    async fn page_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.handle_page_down(&ctxt).await;
    }

    /// 响应候选框光标上移事件
    async fn cursor_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.handle_cursor_up(&ctxt).await;
    }

    /// 响应候选框光标下移事件
    async fn cursor_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.handle_cursor_down(&ctxt).await;
    }

    /// 接收客户端同步的光标周围文本
    fn set_surrounding_text(&self, text: Value<'_>, cursor_pos: u32, anchor_pos: u32) {
        if let Some(parsed) = extract_text_from_ibus_variant(&text) {
            let mut guard = self.core.lock();
            guard.surrounding_text = Some((parsed, cursor_pos, anchor_pos));
        }
    }

    fn panel_extension_received(&self, _event: Value<'_>) {}

    fn panel_extension_register_keys(&self, _data: Value<'_>) {}

    /// 获取当前输入框内容类型属性
    #[zbus(property)]
    fn content_type(&self) -> (u32, u32) {
        (0, 0)
    }

    /// 接收客户端输入框内容类型（自动识别密码与 PIN 输入框并直通按键）
    #[zbus(property)]
    pub(crate) fn set_content_type(&self, content_type: (u32, u32)) {
        let (purpose, _hints) = content_type;
        let is_pwd = purpose == IBUS_INPUT_PURPOSE_PASSWORD || purpose == IBUS_INPUT_PURPOSE_PIN;
        self.update_password_state(is_pwd);
    }

    /// 声明是否启用带焦点路径标识的 `FocusInId` / `FocusOutId`
    #[zbus(property)]
    fn focus_id(&self) -> (bool,) {
        (false,)
    }

    /// 声明是否启用周围文本感知
    #[zbus(property)]
    fn active_surrounding_text(&self) -> (bool,) {
        (true,)
    }

    // 以下均为 `org.freedesktop.IBus.Engine` 规范定义的 D-Bus 信号，
    // 入参列表与顺序由协议契约固定，不适用参数对象收敛规则。

    /// 发射已上屏文本提交信号 `CommitText(text)`
    #[zbus(signal)]
    pub async fn commit_text(ctxt: &SignalContext<'_>, text: Value<'_>) -> zbus::Result<()>;

    /// 发射预编辑文本更新信号 `UpdatePreeditText(text, cursor_pos, visible, mode)`
    #[zbus(signal)]
    pub async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: Value<'_>,
        cursor_pos: u32,
        visible: bool,
        mode: u32,
    ) -> zbus::Result<()>;

    /// 发射预编辑文本隐藏信号 `HidePreeditText`
    #[zbus(signal)]
    pub async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    /// 发射辅助文本（候选拼音提示）更新信号 `UpdateAuxiliaryText(text, visible)`
    #[zbus(signal)]
    pub async fn update_auxiliary_text(
        ctxt: &SignalContext<'_>,
        text: Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    /// 发射候选词表更新信号 `UpdateLookupTable(table, visible)`
    #[zbus(signal)]
    pub async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    /// 发射候选词表隐藏信号 `HideLookupTable`
    #[zbus(signal)]
    pub async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    /// 发射引擎属性批量注册信号 `RegisterProperties(props)`，驱动 GNOME 状态栏指示器重建
    #[zbus(signal)]
    pub async fn register_properties(
        ctxt: &SignalContext<'_>,
        props: Value<'_>,
    ) -> zbus::Result<()>;

    /// 发射单个引擎属性更新信号 `UpdateProperty(prop)`
    #[zbus(signal)]
    pub async fn update_property(ctxt: &SignalContext<'_>, prop: Value<'_>) -> zbus::Result<()>;

    /// 发射按键回传信号 `ForwardKeyEvent(keyval, keycode, state)`，用于成对标点的光标补偿
    #[zbus(signal)]
    pub async fn forward_key_event(
        ctxt: &SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> zbus::Result<()>;
}

impl IbusEngineService {
    /// 更新当前引擎实例与核心会话的密码框感知标记（内部辅助方法，不暴露为 D-Bus 方法）
    pub(crate) fn update_password_state(&self, is_pwd: bool) {
        let prev = self.is_password.swap(is_pwd, Ordering::AcqRel);
        if prev != is_pwd {
            info!("[微澜 IBus] 输入框密码保护状态变迁: {} -> {}", prev, is_pwd);
        }
        let mut guard = self.core.lock();
        guard.is_password_context = is_pwd;
        if is_pwd && !guard.runtime.core.engine.raw_input().is_empty() {
            guard.reset_context();
        }
    }

    /// 清除密码输入感知标记（内部辅助方法，不暴露为 D-Bus 方法）
    pub(crate) fn clear_password_state(&self) {
        let prev = self.is_password.swap(false, Ordering::AcqRel);
        if prev {
            info!("[微澜 IBus] 输入框密码保护状态已清除并复位为普通模式");
        }
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
