//! IBus `org.freedesktop.IBus.Engine`、`Factory` 与 `Service` D-Bus 接口服务实现
//!
//! # 设计原理
//! - **实现初衷**：对接 GNOME (Mutter) 与 `ibus-daemon` 的 D-Bus 通信协议，将系统键盘输入、鼠标选词、
//!   焦点切换与属性栏交互转发至 `IbusSessionCore`，并通过标准信号驱动 GNOME Shell 原生候选框与客户端内联预编辑。
//! - **核心优势**：所有状态变更均在微秒级纯内存同步临界区内完成，信号序列化与异步 I/O 发送全部位于锁作用域之外，
//!   严格杜绝跨 `.await` 持有互斥锁，保证高并发按键流下的极低延迟。
//! - **代价与局限**：依赖 `ibus-daemon` 转发键盘与候选面板事件。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{debug, info, warn};
use waylan_core::{Candidate, InputMode};
use waylan_wayland::input_constants::{KEY_LEFT, KEY_RIGHT};
use waylan_wayland::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::{OwnedObjectPath, Value};

use super::keys::{
    IbusKeyOutcome, IbusSessionCore, IBUS_KEYVAL_LEFT, IBUS_KEYVAL_RIGHT, IBUS_RELEASE_MASK,
};
use super::types::{
    build_ibus_lookup_table, build_ibus_preedit_text, build_ibus_prop_list, build_ibus_text,
    build_input_mode_property, extract_text_from_ibus_variant, IBUS_ENGINE_PREEDIT_CLEAR,
};
use crate::tray::menu::SubprocessManager;

/// 微澜 IBus 引擎固定导出的 D-Bus 对象路径
pub const WAYLAN_ENGINE_OBJECT_PATH: &str = "/org/freedesktop/IBus/Engine/1";

/// 微澜 IBus 工厂固定导出的 D-Bus 对象路径
pub const WAYLAN_FACTORY_OBJECT_PATH: &str = "/org/freedesktop/IBus/Factory";

/// IBus 输入框用途常量：密码框 (`IBUS_INPUT_PURPOSE_PASSWORD`)
const IBUS_INPUT_PURPOSE_PASSWORD: u32 = 2;

/// IBus 输入框用途常量：PIN 码框 (`IBUS_INPUT_PURPOSE_PIN`)
const IBUS_INPUT_PURPOSE_PIN: u32 = 3;

/// 锁外发射 UI 刷新信号所需的轻量状态快照
struct UiSnapshot {
    preedit: String,
    candidates: Vec<Candidate>,
    page_size: usize,
    selected_index: usize,
    orientation: i32,
}

impl UiSnapshot {
    /// 从当前会话核心提取 UI 快照
    fn capture(core: &IbusSessionCore) -> Self {
        let is_v_mode = core.engine.raw_input().starts_with('v');
        let preedit = if core.engine.raw_input().is_empty() {
            String::new()
        } else if is_v_mode {
            core.engine.raw_input().to_string()
        } else {
            core.engine.formatted_preedit()
        };
        let is_expanded = core.engine.is_expanded;
        let candidates = core.current_candidates();
        let (page_size, selected_index, orientation) = if is_expanded {
            (
                candidates.len().max(1),
                core.engine.expanded_selected_index,
                super::types::IBUS_ORIENTATION_VERTICAL,
            )
        } else {
            (
                core.engine.config.page_size,
                core.engine.selected_index,
                super::types::IBUS_ORIENTATION_HORIZONTAL,
            )
        };
        Self {
            preedit,
            candidates,
            page_size,
            selected_index,
            orientation,
        }
    }
}

/// `org.freedesktop.IBus.Factory` 接口实现
pub struct IbusFactoryService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl IbusFactoryService {
    /// 响应 `ibus-daemon` 的引擎实例创建请求，返回预注册的微澜引擎对象路径
    fn create_engine(&self, name: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        debug!("[微澜 IBus] 收到 CreateEngine 请求，引擎名: {}", name);
        if let Ok(mut guard) = self.core.lock() {
            guard.reset_context();
        }
        OwnedObjectPath::try_from(WAYLAN_ENGINE_OBJECT_PATH)
            .map_err(|e| zbus::fdo::Error::Failed(format!("构造引擎对象路径失败: {}", e)))
    }
}

/// `org.freedesktop.IBus.Service` 通用生命周期接口实现
pub struct IbusGenericService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
}

#[interface(name = "org.freedesktop.IBus.Service")]
impl IbusGenericService {
    /// 响应服务对象销毁请求，安全重置内部预编辑与按键状态
    fn destroy(&self) {
        debug!("[微澜 IBus] 收到 Service.Destroy 请求，重置当前输入法上下文。");
        if let Ok(mut guard) = self.core.lock() {
            guard.reset_context();
        }
    }
}

/// `org.freedesktop.IBus.Engine` 核心接口实现
pub struct IbusEngineService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
    pub(crate) reload_requested: Option<Arc<AtomicBool>>,
    pub(crate) loop_wakeup: Option<LoopWakeup>,
    pub(crate) subprocess_manager: Arc<SubprocessManager>,
    pub(crate) settings_running: Arc<AtomicBool>,
}

impl IbusEngineService {
    /// 在锁作用域之外根据快照向 `ibus-daemon` 发射预编辑与候选词表更新信号
    async fn emit_ui_snapshot(ctxt: &SignalContext<'_>, snap: &UiSnapshot) {
        let visible = !snap.preedit.is_empty();
        if let Ok(preedit_val) = build_ibus_preedit_text(&snap.preedit) {
            let char_len = snap.preedit.chars().count() as u32;
            let _ = Self::update_preedit_text(
                ctxt,
                Value::from(preedit_val),
                char_len,
                visible,
                IBUS_ENGINE_PREEDIT_CLEAR,
            )
            .await;
        }

        let table_visible = visible && !snap.candidates.is_empty();
        if let Ok(table_val) = build_ibus_lookup_table(
            &snap.candidates,
            snap.page_size,
            snap.selected_index,
            snap.orientation,
        ) {
            let _ = Self::update_lookup_table(ctxt, Value::from(table_val), table_visible).await;
        }
    }

    /// 在锁作用域之外统一派发 `IbusKeyOutcome` 包含的所有 D-Bus 信号
    async fn dispatch_outcome(
        ctxt: &SignalContext<'_>,
        outcome: IbusKeyOutcome,
        snap: Option<UiSnapshot>,
    ) {
        if let Some(ref text) = outcome.commit_text {
            if let Ok(text_val) = build_ibus_text(text) {
                let _ = Self::commit_text(ctxt, Value::from(text_val)).await;
            }
        }

        if outcome.cursor_offset != 0 {
            let (keyval, keycode) = if outcome.cursor_offset < 0 {
                (IBUS_KEYVAL_LEFT, KEY_LEFT)
            } else {
                (IBUS_KEYVAL_RIGHT, KEY_RIGHT)
            };
            for _ in 0..outcome.cursor_offset.unsigned_abs() {
                let _ = Self::forward_key_event(ctxt, keyval, keycode, 0).await;
                let _ = Self::forward_key_event(ctxt, keyval, keycode, IBUS_RELEASE_MASK).await;
            }
        }

        if let Some(ref s) = snap {
            Self::emit_ui_snapshot(ctxt, s).await;
        }

        if let Some(new_mode) = outcome.mode_changed {
            if let Ok(prop_val) = build_input_mode_property(new_mode) {
                let _ = Self::update_property(ctxt, Value::from(prop_val)).await;
            }
        }
    }

    /// 重置会话并向 `ibus-daemon` 发送隐藏预编辑与候选面板信号
    async fn reset_and_hide_ui(&self, ctxt: &SignalContext<'_>) {
        let snap = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            guard.reset_context();
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }
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
        let (outcome, snap) = {
            let Ok(mut guard) = self.core.lock() else {
                return false;
            };
            let out = guard.process_key_event(keyval, keycode, state);
            let snap = if out.update_ui {
                Some(UiSnapshot::capture(&guard))
            } else {
                None
            };
            (out, snap)
        };

        let consumed = outcome.consumed;
        Self::dispatch_outcome(&ctxt, outcome, snap).await;
        consumed
    }

    /// 更新光标屏幕几何坐标
    fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    /// 手写输入坐标事件（保持空实现以兼容协议）
    fn process_hand_writing_event(&self, _coordinates: Vec<f64>) {}

    /// 取消手写输入事件（保持空实现以兼容协议）
    fn cancel_hand_writing(&self, _n_strokes: u32) {}

    /// 设置客户端支持的输入法能力位掩码
    fn set_capabilities(&self, _caps: u32) {}

    /// 响应 GNOME / IBus 原生状态栏菜单项激活事件
    async fn property_activate(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        name: &str,
        _state: u32,
    ) {
        match name {
            "InputMode" => {
                let new_mode = {
                    let Ok(mut guard) = self.core.lock() else {
                        return;
                    };
                    let next = match guard.mode {
                        InputMode::Chinese | InputMode::VMode => InputMode::English,
                        InputMode::English => InputMode::Chinese,
                    };
                    guard.mode = next;
                    if let Some(ref zh_flag) = guard.shared_is_chinese {
                        zh_flag.store(matches!(next, InputMode::Chinese), Ordering::Release);
                    }
                    if let Some(ref tx) = guard.mode_notify_tx {
                        let _ = tx.try_send(());
                    }
                    next
                };
                if let Ok(prop_val) = build_input_mode_property(new_mode) {
                    let _ = Self::update_property(&ctxt, Value::from(prop_val)).await;
                }
            }
            "ReloadConfig" => {
                if let Some(ref flag) = self.reload_requested {
                    flag.store(true, Ordering::Release);
                }
                if let Some(ref wakeup) = self.loop_wakeup {
                    wakeup.wake();
                }
                info!("[微澜 IBus] 收到原生状态栏菜单重载指令，已触发词库与配置热重载。");
            }
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
        let (outcome, snap) = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
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
            (out, Some(snap))
        };
        Self::dispatch_outcome(&ctxt, outcome, snap).await;
    }

    /// 焦点进入输入上下文
    async fn focus_in(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let (mode, snap) = {
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
            let _ = guard.sync_shared_mode();
            (guard.mode, UiSnapshot::capture(&guard))
        };
        if let Ok(prop_list) = build_ibus_prop_list(mode) {
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
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
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
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
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
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
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
            let Ok(mut guard) = self.core.lock() else {
                return;
            };
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
            if let Ok(mut guard) = self.core.lock() {
                guard.surrounding_text = Some((parsed, cursor_pos, anchor_pos));
            }
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
    fn set_content_type(&self, content_type: (u32, u32)) {
        let (purpose, _hints) = content_type;
        let is_pwd = purpose == IBUS_INPUT_PURPOSE_PASSWORD || purpose == IBUS_INPUT_PURPOSE_PIN;
        if let Ok(mut guard) = self.core.lock() {
            guard.is_password_context = is_pwd;
            if is_pwd && !guard.engine.raw_input().is_empty() {
                guard.reset_context();
            }
        } else {
            warn!("[微澜 IBus] 更新输入框 ContentType 属性时获取会话锁失败");
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
