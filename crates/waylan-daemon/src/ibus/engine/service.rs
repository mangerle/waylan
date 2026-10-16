//! IBus 核心引擎服务 D-Bus 接口实现
//!
//! # 设计原理
//! - **实现初衷**：实现 `org.freedesktop.IBus.Engine` 规范，接收按键、选词、焦点事件，并通过 D-Bus 信号驱动客户端。
//! - **核心优势**：纯内存瞬时加锁变更状态并提取不可变快照，所有信号发送在释放互斥锁后异步执行。
//! - **代价与局限**：必须严格遵循 IBus D-Bus 信号契约。

use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use waylan_wayland::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use super::super::keys::{IbusKeyOutcome, IbusSessionCore};
use super::super::types::{build_ibus_prop_list, extract_text_from_ibus_variant};
use super::snapshot::UiSnapshot;
use crate::tray::menu::SubprocessManager;

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
    pub(crate) subprocess_manager: Arc<SubprocessManager>,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) suppress_properties: bool,
    pub(crate) is_password: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl IbusEngineService {
    /// 处理硬件按键按下与释放事件
    async fn process_key_event(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> bool {
        let (outcome, snap, is_caps_locked) = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            guard.active_engine_path = Some(ctxt.path().to_string());
            if self.is_password.load(Ordering::Acquire) {
                guard.is_password_context = true;
            }
            let out = guard.process_key_event(keyval, keycode, state);
            let snap = if out.update_ui {
                Some(UiSnapshot::capture(&guard))
            } else {
                None
            };
            (out, snap, guard.is_caps_locked)
        };

        let consumed = outcome.consumed;
        self.dispatch_outcome(&ctxt, outcome, snap, is_caps_locked)
            .await;
        consumed
    }

    /// 更新光标屏幕几何坐标
    fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    /// 手写输入坐标事件（保持空实现以兼容协议）
    fn process_hand_writing_event(&self, _coordinates: Vec<f64>) {}

    /// 取消手写输入事件（保持空实现以兼容协议）
    fn cancel_hand_writing(&self, _n_strokes: u32) {}

    /// 设置客户端支持的输入法能力位掩码（同步更新密码框感知标记并清除非密码态残留）
    pub(crate) fn set_capabilities(&self, caps: u32) {
        const IBUS_CAP_PASSWORD: u32 = 1 << 4;
        let is_pwd = (caps & IBUS_CAP_PASSWORD) != 0;
        self.is_password.store(is_pwd, Ordering::Release);
        let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
        guard.is_password_context = is_pwd;
        if is_pwd && !guard.engine.raw_input().is_empty() {
            guard.reset_context();
        }
    }

    /// 响应 GNOME / IBus 原生状态栏菜单项激活事件
    async fn property_activate(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        name: &str,
        _state: u32,
    ) {
        match name {
            "InputMode" => self.on_prop_input_mode(&ctxt).await,
            "TraditionalChinese" => self.on_prop_traditional_chinese(&ctxt).await,
            "FullWidthPunct" => self.on_prop_full_width_punct(&ctxt).await,
            "FullWidthChar" => self.on_prop_full_width_char(&ctxt).await,
            "ReloadConfig" => self.on_prop_reload_config(),
            "Setup" => {
                self.subprocess_manager
                    .launch_settings(&self.settings_running);
            }
            "ViewLog" => {
                self.subprocess_manager.launch_view_log();
            }
            _ => {}
        }
    }

    fn property_show(&self, _name: &str) {}

    fn property_hide(&self, _name: &str) {}

    /// 响应用户在 GNOME 候选框上使用鼠标点击候选词事件
    async fn candidate_clicked(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        index: u32,
        _button: u32,
        _state: u32,
    ) {
        let (outcome, snap, is_caps_locked) = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut out = IbusKeyOutcome {
                consumed: true,
                update_ui: true,
                ..Default::default()
            };
            if guard.engine.is_expanded {
                let sel = guard.engine.selected_index;
                if let Some((committed, _)) = guard.engine.select_expanded_item(sel, index as usize)
                {
                    guard.punct_processor.record_commit(&committed);
                    out.commit_text = Some(committed);
                }
            } else if let Some((committed, _)) = guard.engine.select_candidate(index as usize, now)
            {
                guard.punct_processor.record_commit(&committed);
                out.commit_text = Some(committed);
            }
            let snap = UiSnapshot::capture(&guard);
            (out, Some(snap), guard.is_caps_locked)
        };
        self.dispatch_outcome(&ctxt, outcome, snap, is_caps_locked)
            .await;
    }

    /// 焦点进入输入上下文
    async fn focus_in(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let (mode, is_caps, trad, punct, char_full, snap) = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            guard.active_engine_path = Some(ctxt.path().to_string());
            // 焦点进入新输入上下文时同步当前 Engine 实例记录的密码框状态，
            // 既杜绝旧普通窗口残留密码标记，又防止因先触发 SetContentType 后触发 FocusIn 导致密码标记被误冲刷
            guard.is_password_context = self.is_password.load(Ordering::Acquire);
            // 自愈保护：若当前获焦的并非密码框，而内部仍记录为锁屏状态，说明桌面已解锁但锁屏信号丢失，自动解除锁屏直通
            if !guard.is_password_context && guard.is_screen_locked {
                info!("[微澜 IBus] 普通窗口获焦，自动校准并解除残留的锁屏直通保护。");
                guard.is_screen_locked = false;
            }
            let _ = guard.sync_shared_mode();
            guard.sync_shared_settings();
            let trad = guard.engine.config.traditional_chinese;
            let punct = guard.punct_processor.full_width_punct;
            let char_full = guard.is_full_width_char();
            (
                guard.mode,
                guard.is_caps_locked,
                trad,
                punct,
                char_full,
                UiSnapshot::capture(&guard),
            )
        };
        if let Ok(prop_list) = build_ibus_prop_list(mode, is_caps, trad, punct, char_full) {
            let _ = Self::register_properties(&ctxt, Value::from(prop_list)).await;
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
        let snap = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.engine.is_expanded {
                let _ = guard.engine.prev_page();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(&ctxt, &snap).await;
    }

    /// 响应候选框下一页按钮事件
    async fn page_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.engine.is_expanded {
                let _ = guard.engine.next_page();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(&ctxt, &snap).await;
    }

    /// 响应候选框光标上移事件
    async fn cursor_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if guard.engine.is_expanded {
                let sel = guard.engine.selected_index;
                guard.engine.prev_expanded_item(sel);
            } else {
                guard.engine.prev_candidate();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(&ctxt, &snap).await;
    }

    /// 响应候选框光标下移事件
    async fn cursor_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if guard.engine.is_expanded {
                let sel = guard.engine.selected_index;
                guard.engine.next_expanded_item(sel);
            } else {
                guard.engine.next_candidate();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(&ctxt, &snap).await;
    }

    /// 接收客户端同步的光标周围文本
    fn set_surrounding_text(&self, text: Value<'_>, cursor_pos: u32, anchor_pos: u32) {
        if let Some(parsed) = extract_text_from_ibus_variant(&text) {
            let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
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
        self.is_password.store(is_pwd, Ordering::Release);
        let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
        guard.is_password_context = is_pwd;
        if is_pwd && !guard.engine.raw_input().is_empty() {
            guard.reset_context();
        }
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

    #[zbus(signal)]
    pub async fn commit_text(ctxt: &SignalContext<'_>, text: Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: Value<'_>,
        cursor_pos: u32,
        visible: bool,
        mode: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn update_auxiliary_text(
        ctxt: &SignalContext<'_>,
        text: Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn register_properties(
        ctxt: &SignalContext<'_>,
        props: Value<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn update_property(ctxt: &SignalContext<'_>, prop: Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn forward_key_event(
        ctxt: &SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
