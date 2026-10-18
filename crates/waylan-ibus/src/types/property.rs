//! IBus 菜单项与状态指示属性（IBusProperty / IBusPropList）变体构建
//!
//! # 设计原理
//! - **实现初衷**：为 GNOME Shell 面板及各类 IBus 前端构建中英文状态、全半角标点、简繁切换及操作菜单项。
//! - **核心优势**：纯栈内类型安全的变体包装，精确匹配 `(sa{sv}suvsvbbuvv)` 签名。
//! - **代价与局限**：严格遵循 IBus 属性列表协议规范。

use super::component::resolve_mode_icon;
use super::text::build_ibus_text;
use super::{empty_attachments, empty_variant_vec};
use waylan_core::InputMode;
use zbus::zvariant::{Error as VariantError, OwnedValue, StructureBuilder, Value};

/// IBus 属性类型：普通按钮项 (`PROP_TYPE_NORMAL`)
const IBUS_PROP_TYPE_NORMAL: u32 = 0;

/// IBus 属性状态：未选中 (`PROP_STATE_UNCHECKED`)
const IBUS_PROP_STATE_UNCHECKED: u32 = 0;

/// 构造单个通用 `IBusProperty` 菜单项变体对象，签名 `(sa{sv}suvsvbbuvv)`
///
/// # Errors
/// 当属性结构体序列化失败时返回错误。
pub fn build_action_property(
    key: &str,
    label: &str,
    icon: &str,
    tooltip: &str,
    symbol: &str,
) -> Result<OwnedValue, VariantError> {
    let empty_sub_struct = StructureBuilder::new()
        .add_field("IBusPropList")
        .add_field(empty_attachments())
        .add_field(empty_variant_vec())
        .build();
    let empty_sub = OwnedValue::try_from(Value::from(empty_sub_struct))?;

    let s = StructureBuilder::new()
        .add_field("IBusProperty")
        .add_field(empty_attachments())
        .add_field(key.to_string())
        .add_field(IBUS_PROP_TYPE_NORMAL)
        .add_field(build_ibus_text(label)?)
        .add_field(icon)
        .add_field(build_ibus_text(tooltip)?)
        .add_field(true)
        .add_field(true)
        .add_field(IBUS_PROP_STATE_UNCHECKED)
        .add_field(empty_sub)
        .add_field(build_ibus_text(symbol)?)
        .build();
    OwnedValue::try_from(Value::from(s))
}

/// 构造中英文状态指示属性 (`InputMode`) 的 `IBusProperty` 变体对象，签名 `(sa{sv}suvsvbbuvv)`
///
/// # Errors
/// 当属性结构体序列化失败时返回错误。
pub fn build_input_mode_property(
    mode: InputMode,
    is_caps_locked: bool,
) -> Result<OwnedValue, VariantError> {
    let is_zh = matches!(mode, InputMode::Chinese | InputMode::VMode);
    let symbol_str = if is_caps_locked {
        "A"
    } else if is_zh {
        "中"
    } else {
        "英"
    };
    let label_str = if is_caps_locked {
        "当前输入: 英文大写"
    } else if is_zh {
        "当前输入: 中文拼音"
    } else {
        "当前输入: 英文模式"
    };
    let icon_path = resolve_mode_icon(mode, is_caps_locked);
    build_action_property(
        "InputMode",
        label_str,
        &icon_path,
        "点击或按切换键更改中英文输入状态",
        symbol_str,
    )
}

/// 构造简繁切换状态属性 (`TraditionalChinese`) 的 `IBusProperty` 变体对象
pub fn build_traditional_property(traditional_chinese: bool) -> Result<OwnedValue, VariantError> {
    let label = if traditional_chinese {
        "简繁切换: 繁体"
    } else {
        "简繁切换: 简体"
    };
    build_action_property(
        "TraditionalChinese",
        label,
        "character-set",
        "点击切换简体或繁体输入模式",
        "",
    )
}

/// 构造标点全角/半角属性 (`FullWidthPunct`) 的 `IBusProperty` 变体对象
pub fn build_full_width_punct_property(full_width_punct: bool) -> Result<OwnedValue, VariantError> {
    let label = if full_width_punct {
        "标点切换: 全角 (中文标点)"
    } else {
        "标点切换: 半角 (英文标点)"
    };
    build_action_property(
        "FullWidthPunct",
        label,
        "accessories-character-map",
        "点击切换全角或半角标点符号",
        "",
    )
}

/// 构造英文字符全角/半角属性 (`FullWidthChar`) 的 `IBusProperty` 变体对象
pub fn build_full_width_char_property(full_width_char: bool) -> Result<OwnedValue, VariantError> {
    let label = if full_width_char {
        "字符切换: 全角 (英文字母)"
    } else {
        "字符切换: 半角 (英文字母)"
    };
    build_action_property(
        "FullWidthChar",
        label,
        "format-text-bold",
        "点击切换全角或半角英文字母",
        "",
    )
}

/// 构造包含微澜状态与操作菜单的 `IBusPropList` 变体对象（用于 `RegisterProperties`），签名 `(sa{sv}av)`
///
/// # Errors
/// 当属性列表序列化失败时返回错误。
pub fn build_ibus_prop_list(
    mode: InputMode,
    is_caps_locked: bool,
    traditional_chinese: bool,
    full_width_punct: bool,
    full_width_char: bool,
) -> Result<OwnedValue, VariantError> {
    let props = vec![
        build_input_mode_property(mode, is_caps_locked)?,
        build_traditional_property(traditional_chinese)?,
        build_full_width_punct_property(full_width_punct)?,
        build_full_width_char_property(full_width_char)?,
        build_action_property(
            "ReloadConfig",
            "重载词库与配置",
            "view-refresh",
            "立即热重载微澜配置文件与用户词库",
            "",
        )?,
        build_action_property(
            "Setup",
            "偏好设置",
            "preferences-system",
            "打开微澜图形化偏好设置中心",
            "",
        )?,
        build_action_property(
            "ViewLog",
            "查看运行日志",
            "text-x-generic",
            "使用系统默认程序打开微澜运行日志",
            "",
        )?,
    ];
    let s = StructureBuilder::new()
        .add_field("IBusPropList")
        .add_field(empty_attachments())
        .add_field(props)
        .build();
    OwnedValue::try_from(Value::from(s))
}
