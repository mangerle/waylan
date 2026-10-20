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

/// IBus 通用操作属性项构建参数包
///
/// # 设计原理
/// - **实现初衷**：单个 `IBusProperty` 菜单项需要同时携带键名、显示文本、图标、悬浮提示与状态符号，
///   平铺五个字符串参数极易在调用点发生位置错配，故收敛为专用的参数结构体。
/// - **核心优势**：以具名字段取代位置参数，键名与提示文本不会互相串位，可读性与可维护性显著提升。
/// - **代价与局限**：调用点需显式书写字段名，代码量略增，但换来长期可维护性。
pub struct ActionPropertyParams<'a> {
    /// 属性键名（如 `InputMode`）
    pub key: &'a str,
    /// 属性显示文本
    pub label: &'a str,
    /// 属性图标名称
    pub icon: &'a str,
    /// 属性悬浮提示文本
    pub tooltip: &'a str,
    /// 属性状态标记符号
    pub symbol: &'a str,
}

/// 构造单个通用 `IBusProperty` 菜单项变体对象，签名 `(sa{sv}suvsvbbuvv)`
///
/// # Errors
/// 当属性结构体序列化失败时返回错误。
pub fn build_action_property(
    params: &ActionPropertyParams<'_>,
) -> Result<OwnedValue, VariantError> {
    let ActionPropertyParams {
        key,
        label,
        icon,
        tooltip,
        symbol,
    } = *params;
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
    build_action_property(&ActionPropertyParams {
        key: "InputMode",
        label: label_str,
        icon: &icon_path,
        tooltip: "点击或按切换键更改中英文输入状态",
        symbol: symbol_str,
    })
}

/// 构造简繁切换状态属性 (`TraditionalChinese`) 的 `IBusProperty` 变体对象
pub fn build_traditional_property(traditional_chinese: bool) -> Result<OwnedValue, VariantError> {
    let label = if traditional_chinese {
        "简繁切换: 繁体"
    } else {
        "简繁切换: 简体"
    };
    build_action_property(&ActionPropertyParams {
        key: "TraditionalChinese",
        label,
        icon: "character-set",
        tooltip: "点击切换简体或繁体输入模式",
        symbol: "",
    })
}

/// 构造标点全角/半角属性 (`FullWidthPunct`) 的 `IBusProperty` 变体对象
pub fn build_full_width_punct_property(full_width_punct: bool) -> Result<OwnedValue, VariantError> {
    let label = if full_width_punct {
        "标点切换: 全角 (中文标点)"
    } else {
        "标点切换: 半角 (英文标点)"
    };
    build_action_property(&ActionPropertyParams {
        key: "FullWidthPunct",
        label,
        icon: "accessories-character-map",
        tooltip: "点击切换全角或半角标点符号",
        symbol: "",
    })
}

/// 构造英文字符全角/半角属性 (`FullWidthChar`) 的 `IBusProperty` 变体对象
pub fn build_full_width_char_property(full_width_char: bool) -> Result<OwnedValue, VariantError> {
    let label = if full_width_char {
        "字符切换: 全角 (英文字母)"
    } else {
        "字符切换: 半角 (英文字母)"
    };
    build_action_property(&ActionPropertyParams {
        key: "FullWidthChar",
        label,
        icon: "format-text-bold",
        tooltip: "点击切换全角或半角英文字母",
        symbol: "",
    })
}

/// IBus 原生属性列表构建参数包
///
/// # 设计原理
/// - **实现初衷**：原生属性列表需汇聚输入模式、大写锁定、简繁与全半角共五项状态，平铺布尔参数极易误传。
/// - **核心优势**：具名字段让每个状态开关的归属一目了然，杜绝位置错配导致的静默行为偏差。
/// - **代价与局限**：调用点需显式书写字段名，代码量略增但语义更清晰。
pub struct IbusPropListParams {
    /// 当前输入模式（中文 / 英文 / V 模式）
    pub mode: InputMode,
    /// 是否处于大写锁定状态
    pub is_caps_locked: bool,
    /// 是否启用繁体中文输入
    pub traditional_chinese: bool,
    /// 是否使用全角标点
    pub full_width_punct: bool,
    /// 是否使用全角英文字符
    pub full_width_char: bool,
}

/// 构造包含微澜状态与操作菜单的 `IBusPropList` 变体对象（用于 `RegisterProperties`），签名 `(sa{sv}av)`
///
/// # Errors
/// 当属性列表序列化失败时返回错误。
pub fn build_ibus_prop_list(params: &IbusPropListParams) -> Result<OwnedValue, VariantError> {
    let IbusPropListParams {
        mode,
        is_caps_locked,
        traditional_chinese,
        full_width_punct,
        full_width_char,
    } = *params;
    let props = vec![
        build_input_mode_property(mode, is_caps_locked)?,
        build_traditional_property(traditional_chinese)?,
        build_full_width_punct_property(full_width_punct)?,
        build_full_width_char_property(full_width_char)?,
        build_action_property(&ActionPropertyParams {
            key: "ReloadConfig",
            label: "重载词库与配置",
            icon: "view-refresh",
            tooltip: "立即热重载微澜配置文件与用户词库",
            symbol: "",
        })?,
        build_action_property(&ActionPropertyParams {
            key: "Setup",
            label: "偏好设置",
            icon: "preferences-system",
            tooltip: "打开微澜图形化偏好设置中心",
            symbol: "",
        })?,
        build_action_property(&ActionPropertyParams {
            key: "ViewLog",
            label: "查看运行日志",
            icon: "text-x-generic",
            tooltip: "使用系统默认程序打开微澜运行日志",
            symbol: "",
        })?,
    ];
    let s = StructureBuilder::new()
        .add_field("IBusPropList")
        .add_field(empty_attachments())
        .add_field(props)
        .build();
    OwnedValue::try_from(Value::from(s))
}
