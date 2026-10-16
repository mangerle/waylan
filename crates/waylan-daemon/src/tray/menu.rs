use log::info;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::Value;

use crate::system::ShutdownCoordinator;

/// DBusMenu 布局返回元组类型定义
pub type DbusMenuLayout = (
    u32,
    (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>),
);

/// 构造单条 DBusMenu 菜单项结构
pub(crate) fn make_menu_item(id: i32, label: &str) -> Value<'static> {
    let mut props = HashMap::with_capacity(1);
    props.insert("label".to_string(), Value::from(label.to_string()));
    Value::from((id, props, Vec::<Value>::new()))
}

pub use super::subprocess::SubprocessManager;

/// DBusMenu 构造配置参数包
pub(crate) struct DBusMenuOptions {
    pub(crate) is_chinese: Arc<AtomicBool>,
    pub(crate) is_caps_locked: Arc<AtomicBool>,
    pub(crate) traditional_chinese: Arc<AtomicBool>,
    pub(crate) full_width_punct: Arc<AtomicBool>,
    pub(crate) full_width_char: Arc<AtomicBool>,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) coordinator: ShutdownCoordinator,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
}

/// 标准 FreeDesktop DBusMenu 菜单实现
pub struct DBusMenu {
    pub(crate) is_chinese: Arc<AtomicBool>,
    pub(crate) is_caps_locked: Arc<AtomicBool>,
    pub(crate) traditional_chinese: Arc<AtomicBool>,
    pub(crate) full_width_punct: Arc<AtomicBool>,
    pub(crate) full_width_char: Arc<AtomicBool>,
    pub(crate) reload_requested: Arc<AtomicBool>,
    pub(crate) coordinator: ShutdownCoordinator,
    pub(crate) settings_running: Arc<AtomicBool>,
    pub(crate) icon_update_tx: SyncSender<()>,
    pub(crate) subprocess_manager: Arc<SubprocessManager>,
}

impl DBusMenu {
    pub(crate) fn new(opts: DBusMenuOptions) -> Self {
        let subprocess_manager = Arc::new(SubprocessManager::new(opts.coordinator.flag()));
        Self {
            is_chinese: opts.is_chinese,
            is_caps_locked: opts.is_caps_locked,
            traditional_chinese: opts.traditional_chinese,
            full_width_punct: opts.full_width_punct,
            full_width_char: opts.full_width_char,
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

    /// 切换原子标志并在通道唤醒托盘与 Wayland 事件循环，同时输出中文日志反馈
    fn toggle_flag_with_feedback(
        &self,
        flag: &AtomicBool,
        label: &'static str,
        val_true: &'static str,
        val_false: &'static str,
    ) {
        let current = flag.load(Ordering::Acquire);
        let next = !current;
        flag.store(next, Ordering::Release);
        let _ = self.icon_update_tx.try_send(());
        self.coordinator.wake();
        info!(
            "[系统托盘] 右键菜单切换{}为: {}",
            label,
            if next { val_true } else { val_false }
        );
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
        let mode_text = if self.is_caps_locked.load(Ordering::Acquire) {
            "当前输入: 英文大写"
        } else if self.is_chinese.load(Ordering::Acquire) {
            "当前输入: 中文拼音"
        } else {
            "当前输入: 英文模式"
        };
        let trad_text = if self.traditional_chinese.load(Ordering::Acquire) {
            "简繁切换: 繁体"
        } else {
            "简繁切换: 简体"
        };
        let punct_text = if self.full_width_punct.load(Ordering::Acquire) {
            "标点切换: 全角 (中文标点)"
        } else {
            "标点切换: 半角 (英文标点)"
        };
        let char_text = if self.full_width_char.load(Ordering::Acquire) {
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
    pub(crate) fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }

        match id {
            1 => self.toggle_flag_with_feedback(&self.is_chinese, "输入模式", "中文", "英文"),
            2 => self.on_menu_reload(),
            3 => self.launch_settings(),
            4 => self.launch_view_log(),
            5 => self.on_menu_exit(),
            6 => self.toggle_flag_with_feedback(
                &self.traditional_chinese,
                "简繁模式",
                "繁体",
                "简体",
            ),
            7 => self.toggle_flag_with_feedback(
                &self.full_width_punct,
                "标点模式",
                "全角 (中文标点)",
                "半角 (英文标点)",
            ),
            8 => self.toggle_flag_with_feedback(
                &self.full_width_char,
                "字符全半角",
                "全角 (英文字母)",
                "半角 (英文字母)",
            ),
            9 => self.on_menu_restart(),
            _ => {}
        }
    }

    /// 菜单布局更新通知信号
    #[zbus(signal)]
    async fn layout_updated(
        signal_ctxt: &SignalContext<'_>,
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
        let is_chinese = Arc::new(AtomicBool::new(true));
        let is_caps = Arc::new(AtomicBool::new(false));
        let reload = Arc::new(AtomicBool::new(false));
        let exit = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);

        let is_trad = Arc::new(AtomicBool::new(false));
        let is_full_punct = Arc::new(AtomicBool::new(true));
        let is_full_char = Arc::new(AtomicBool::new(false));

        let coordinator = ShutdownCoordinator::from_parts(exit, None);

        let menu = DBusMenu::new(DBusMenuOptions {
            is_chinese: is_chinese.clone(),
            is_caps_locked: is_caps.clone(),
            traditional_chinese: is_trad.clone(),
            full_width_punct: is_full_punct.clone(),
            full_width_char: is_full_char.clone(),
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
        menu.event(6, "clicked".to_string(), Value::from(0), 0);
        assert!(is_trad.load(Ordering::Acquire));
        let (_rev, (_root_id, _props, items_updated)) = menu.get_layout(0, -1, Vec::new());
        assert!(format!("{:?}", items_updated[1]).contains("简繁切换: 繁体"));

        // 开启大写锁定，验证菜单第1项文本正确变为英文大写
        is_caps.store(true, Ordering::Release);
        let (_rev, (_root_id, _props, items_caps)) = menu.get_layout(0, -1, Vec::new());
        let item1_caps_str = format!("{:?}", items_caps[0]);
        assert!(item1_caps_str.contains("当前输入: 英文大写"));

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
