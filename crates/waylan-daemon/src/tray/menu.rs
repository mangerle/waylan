use log::info;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use zbus::Variant;
use zbus::interface;
use zbus::object_server::SignalEmitter;

use crate::system::ShutdownCoordinator;

/// DBusMenu 布局返回元组类型定义
pub type DbusMenuLayout = (
    u32,
    (
        i32,
        HashMap<String, Variant<'static>>,
        Vec<Variant<'static>>,
    ),
);

/// 构造单条 DBusMenu 菜单项结构
pub(crate) fn make_menu_item(id: i32, label: &str) -> Variant<'static> {
    let mut props = HashMap::with_capacity(1);
    props.insert("label".to_string(), Variant::from(label.to_string()));
    Variant::from((id, props, Vec::<Variant>::new()))
}

pub use crate::system::SubprocessManager;
use waylan_core::state::SharedInputFlags;

/// DBusMenu 构造配置参数包
pub(crate) struct DBusMenuOptions {
    pub(crate) shared_flags: SharedInputFlags,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) coordinator: ShutdownCoordinator,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
}

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    pub(crate) shared_flags: SharedInputFlags,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) coordinator: ShutdownCoordinator,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
    pub(crate) subprocess_manager: Arc<SubprocessManager>,
}

impl DBusMenu {
    /// 由托盘菜单装配参数构造 `com.canonical.dbusmenu` 菜单服务对象
    pub(crate) fn new(opts: DBusMenuOptions) -> Self {
        let subprocess_manager = Arc::new(SubprocessManager::new(opts.coordinator.flag()));
        Self {
            shared_flags: opts.shared_flags,
            reload_requested: opts.reload_requested,
            coordinator: opts.coordinator,
            settings_running: opts.settings_running,
            icon_update_tx: opts.icon_update_tx,
            subprocess_manager,
        }
    }

    fn launch_settings(&self) {
        self.subprocess_manager
            .launch_settings(&self.settings_running);
    }

    fn launch_view_log(&self) {
        self.subprocess_manager.launch_view_log();
    }

    /// 唤醒托盘与 Wayland 事件循环，同时输出中文日志反馈
    fn notify_flag_changed(&self, label: &'static str, val_str: &'static str) {
        let _ = self.icon_update_tx.try_send(());
        self.coordinator.wake();
        info!("[系统托盘] 右键菜单切换{}为: {}", label, val_str);
    }

    /// 响应点击菜单“重新加载词库与配置”
    fn on_menu_reload(&self) {
        self.reload_requested.store(true, Ordering::Release);
        self.coordinator.wake();
        info!("[系统托盘] 右键菜单触发：已发出重新加载词库与配置信号...");
    }

    /// 响应点击菜单“退出微澜输入法”
    fn on_menu_exit(&self) {
        info!("[系统托盘] 收到退出菜单指令，正在触发优雅停机与数据落盘流程...");
        self.coordinator.request_shutdown();
        let _ = self.icon_update_tx.try_send(());
    }

    /// 响应点击菜单“重启微澜输入法”
    fn on_menu_restart(&self) {
        info!("[系统托盘] 收到重启菜单指令，正在排队等待旧实例完全释放后拉起新实例...");
        self.coordinator.trigger_graceful_restart();
        let _ = self.icon_update_tx.try_send(());
    }

    /// 响应点击菜单“当前输入”项：切换中英文状态，并在大写锁定时优先解除锁定
    fn on_menu_input_mode(&self) {
        let snap = self.shared_flags.snapshot();
        if snap.is_caps_locked {
            self.shared_flags.set_caps_locked(false);
            self.shared_flags.set_chinese(true);
            self.notify_flag_changed("输入模式", "中文 (解除大写锁定)");
        } else {
            let next = !snap.is_chinese;
            self.shared_flags.set_chinese(next);
            self.notify_flag_changed("输入模式", if next { "中文" } else { "英文" });
        }
    }

    /// 响应点击菜单“简繁切换”项：在简体与繁体输入模式间翻转
    fn on_menu_traditional(&self) {
        let next = !self.shared_flags.is_traditional();
        self.shared_flags.set_traditional(next);
        self.notify_flag_changed("简繁模式", if next { "繁体" } else { "简体" });
    }

    /// 响应点击菜单“标点切换”项：在中文全角标点与英文半角标点间翻转
    fn on_menu_punct_width(&self) {
        let next = !self.shared_flags.is_full_width_punct();
        self.shared_flags.set_full_width_punct(next);
        self.notify_flag_changed(
            "标点模式",
            if next {
                "全角 (中文标点)"
            } else {
                "半角 (英文标点)"
            },
        );
    }

    /// 响应点击菜单“字符切换”项：在全角与半角英文字母间翻转
    fn on_menu_char_width(&self) {
        let next = !self.shared_flags.is_full_width_char();
        self.shared_flags.set_full_width_char(next);
        self.notify_flag_changed(
            "字符全半角",
            if next {
                "全角 (英文字母)"
            } else {
                "半角 (英文字母)"
            },
        );
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl DBusMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "normal".to_string()
    }

    /// 获取托盘右键菜单布局
    pub(crate) fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> DbusMenuLayout {
        let snap = self.shared_flags.snapshot();
        let mode_text = if snap.is_caps_locked {
            "当前输入: 英文大写"
        } else if snap.is_chinese {
            "当前输入: 中文拼音"
        } else {
            "当前输入: 英文模式"
        };
        let trad_text = if snap.traditional_chinese {
            "简繁切换: 繁体"
        } else {
            "简繁切换: 简体"
        };
        let punct_text = if snap.full_width_punct {
            "标点切换: 全角 (中文标点)"
        } else {
            "标点切换: 半角 (英文标点)"
        };
        let char_text = if snap.full_width_char {
            "字符切换: 全角 (英文字母)"
        } else {
            "字符切换: 半角 (英文字母)"
        };
        let menu_items = vec![
            make_menu_item(1, mode_text),
            make_menu_item(6, trad_text),
            make_menu_item(7, punct_text),
            make_menu_item(8, char_text),
            make_menu_item(2, "重载词库与配置"),
            make_menu_item(3, "偏好设置"),
            make_menu_item(4, "查看运行日志"),
            make_menu_item(9, "重启微澜输入法"),
            make_menu_item(5, "退出微澜输入法"),
        ];

        (1, (0, HashMap::new(), menu_items))
    }

    /// 用户在右键菜单中点击特定项触发事件
    ///
    /// # 设计原理
    /// - **实现初衷**：本方法签名严格对应 `com.canonical.dbusmenu` 接口的 `Event` 方法
    ///   （`id: i`、`event_id: s`、`data: v`、`timestamp: u`），由 D-Bus 协议契约强制约束。
    /// - **核心优势**：保持与托盘菜单宿主（GNOME Shell / KDE 等）的协议兼容。
    /// - **代价与局限**：入参个数由外部协议固定，无法收敛为参数对象；
    ///   其中 `data` 与 `timestamp` 当前未被消费，仅为满足 D-Bus 签名而保留。
    pub(crate) fn event(&self, id: i32, event_id: String, _data: Variant<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }

        match id {
            1 => self.on_menu_input_mode(),
            2 => self.on_menu_reload(),
            3 => self.launch_settings(),
            4 => self.launch_view_log(),
            5 => self.on_menu_exit(),
            6 => self.on_menu_traditional(),
            7 => self.on_menu_punct_width(),
            8 => self.on_menu_char_width(),
            9 => self.on_menu_restart(),
            _ => {}
        }
    }

    /// 菜单布局更新通知信号
    #[zbus(signal)]
    async fn layout_updated(
        signal_ctxt: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_menu_layout_contains_dynamic_log_path() {
        let flags = SharedInputFlags::new(true, false, true, false);
        let reload = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);
        let coordinator = ShutdownCoordinator::from_parts(exit, None);

        let menu = DBusMenu::new(DBusMenuOptions {
            shared_flags: flags.clone(),
            reload_requested: reload,
            coordinator,
            settings_running: settings,
            icon_update_tx: tx,
        });
        let (_rev, (_root_id, _props, items)) = menu.get_layout(0, -1, Vec::new());
        assert_eq!(items.len(), 9);

        // 验证默认中文状态文本
        let item1_str = format!("{:?}", items[0]);
        assert!(item1_str.contains("当前输入: 中文拼音"));

        // 验证新添加的 3 个菜单项
        let item2_str = format!("{:?}", items[1]);
        assert!(item2_str.contains("简繁切换: 简体"));
        let item3_str = format!("{:?}", items[2]);
        assert!(item3_str.contains("标点切换: 全角 (中文标点)"));
        let item4_str = format!("{:?}", items[3]);
        assert!(item4_str.contains("字符切换: 半角 (英文字母)"));

        // 测试点击切换简繁
        menu.event(6, "clicked".to_string(), Variant::from(0), 0);
        assert!(flags.is_traditional());
        let (_rev, (_root_id, _props, items_updated)) = menu.get_layout(0, -1, Vec::new());
        assert!(format!("{:?}", items_updated[1]).contains("简繁切换: 繁体"));

        // 开启大写锁定，验证菜单第1项文本正确变为英文大写
        flags.set_caps_locked(true);
        let (_rev, (_root_id, _props, items_caps)) = menu.get_layout(0, -1, Vec::new());
        let item1_caps_str = format!("{:?}", items_caps[0]);
        assert!(item1_caps_str.contains("当前输入: 英文大写"));

        // 点击第1项菜单（当前输入: 英文大写），验证自动解除大写锁定并切回中文
        menu.event(1, "clicked".to_string(), Variant::from(0), 0);
        assert!(
            !flags.is_caps_locked(),
            "点击大写菜单项后应立即解除大写锁定"
        );
        assert!(flags.is_chinese(), "解除大写锁定后应恢复中文输入模式");

        let expected_label = "查看运行日志";

        // 验证第7项菜单（查看运行日志）包含真实文本
        let item7 = &items[6];
        let serialized = format!("{:?}", item7);
        assert!(
            serialized.contains(expected_label),
            "菜单项中应包含实际日志绝对路径"
        );
    }
}
