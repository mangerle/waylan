//! IBus 组件与引擎描述变体构建与解析
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法守护进程动态向 `ibus-daemon` 注册输入法组件 (`IBusComponent`) 与引擎描述 (`IBusEngineDesc`)。
//! - **核心优势**：自动解析系统图标规范路径，提供快速文本提取工具，签名严格匹配 `(sa{sv}ssssssssavav)`。
//! - **代价与局限**：引擎字段顺序严格绑定 IBus 1.5 组件协议规范。

use super::{empty_attachments, empty_variant_vec};
use waylan_core::InputMode;
use zbus::zvariant::{Error as VariantError, OwnedValue, StructureBuilder, Value};

/// 解析输入模式对应的微澜矢量图标（优先规范绝对路径以确保 GNOME 原生面板直接加载）
pub fn resolve_mode_icon(mode: InputMode, is_caps_locked: bool) -> String {
    let base_name = if is_caps_locked {
        "tray-en-caps"
    } else if matches!(mode, InputMode::Chinese | InputMode::VMode) {
        "tray-zh"
    } else {
        "tray-en"
    };
    if let Some(assets) = waylan_core::assets::detect_default_assets_path() {
        waylan_core::assets::resolve_icon_path_or_name(&assets, base_name)
    } else {
        base_name.to_string()
    }
}

/// 解析微澜引擎主图标（优先规范绝对路径）
pub fn resolve_engine_icon() -> String {
    if let Some(assets) = waylan_core::assets::detect_default_assets_path() {
        waylan_core::assets::resolve_icon_path_or_name(&assets, "waylan")
    } else {
        "waylan".to_string()
    }
}

/// 构造微澜输入法的 `IBusComponent` 变体对象（用于运行时动态向 `ibus-daemon` 注册引擎），签名 `(sa{sv}ssssssssavav)`
///
/// # Errors
/// 当组件或引擎描述序列化失败时返回错误。
pub fn build_waylan_ibus_component(
    exec_cmd: &str,
    setup_cmd: &str,
) -> Result<OwnedValue, VariantError> {
    let version = env!("CARGO_PKG_VERSION");
    let engine_icon = resolve_engine_icon();
    let engine_desc = StructureBuilder::new()
        .add_field("IBusEngineDesc")
        .add_field(empty_attachments())
        .add_field("waylan")
        .add_field("微澜 (WayLan)")
        .add_field("微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法")
        .add_field("zh_CN")
        .add_field("Apache-2.0")
        .add_field("WayLan Contributors")
        .add_field(engine_icon.as_str())
        .add_field("us")
        .add_field(99u32)
        .add_field("")
        .add_field("澜")
        .add_field(setup_cmd)
        .add_field("")
        .add_field("")
        .add_field(version)
        .add_field("waylan")
        .add_field("InputMode")
        .build();
    let engine_val = OwnedValue::try_from(Value::from(engine_desc))?;

    let comp = StructureBuilder::new()
        .add_field("IBusComponent")
        .add_field(empty_attachments())
        .add_field("org.freedesktop.IBus.Waylan")
        .add_field("WayLan Chinese Input Method Component")
        .add_field(version)
        .add_field("Apache-2.0")
        .add_field("WayLan Contributors")
        .add_field("https://github.com/waylan")
        .add_field(exec_cmd)
        .add_field("waylan")
        .add_field(empty_variant_vec())
        .add_field(vec![engine_val])
        .build();
    OwnedValue::try_from(Value::from(comp))
}

/// 从 `IBusText` 或 `IBusEngineDesc` Variant 中安全提取第 3 个字段（文本或引擎名称）
pub fn extract_text_from_ibus_variant(val: &Value<'_>) -> Option<String> {
    let mut inner = val;
    while let Value::Value(v) = inner {
        inner = v.as_ref();
    }
    if let Value::Structure(s) = inner {
        let fields = s.fields();
        if fields.len() >= 3 {
            if let Value::Str(text) = &fields[2] {
                return Some(text.as_str().to_string());
            }
        }
    }
    None
}
