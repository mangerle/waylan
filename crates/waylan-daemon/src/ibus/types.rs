//! IBus D-Bus 协议可序列化对象（IBusSerializable）构建与解析模块
//!
//! # 设计原理
//! - **实现初衷**：IBus 守护进程 (`ibus-daemon`) 与 GNOME Shell 面板之间的 D-Bus 通信大量使用自描述的
//!   `GVariant` 包装结构（`IBusText`、`IBusLookupTable`、`IBusPropList`、`IBusComponent` 等）。
//!   本模块直接基于项目已有的 `zbus::zvariant::StructureBuilder` 构建符合 `libibus-1.0` 二进制规范的变体数据，无需引入 C 语言 `libibus` 绑定。
//! - **核心优势**：纯 Rust 零 FFI 开销，生成的 `OwnedValue` 严格匹配 IBus 1.5 的 D-Bus 签名（如 `(sa{sv}uubbiavav)`）。
//! - **代价与局限**：需严格遵循 IBus 1.5 规范的字段顺序，若未来 IBus 2.0 变更底层序列化布局需同步调整字段序列。

use std::collections::HashMap;

use waylan_core::{Candidate, InputMode};
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

/// IBus 文本属性类型：下划线 (`IBUS_ATTR_TYPE_UNDERLINE`)
pub const IBUS_ATTR_TYPE_UNDERLINE: u32 = 1;

/// IBus 下划线样式：单实线 (`IBUS_ATTR_UNDERLINE_SINGLE`)
pub const IBUS_ATTR_UNDERLINE_SINGLE: u32 = 1;

/// IBus 预编辑失焦行为：清空预编辑 (`IBUS_ENGINE_PREEDIT_CLEAR`)
pub const IBUS_ENGINE_PREEDIT_CLEAR: u32 = 0;

/// IBus 候选框排列方向：水平排列 (`IBUS_ORIENTATION_HORIZONTAL`)
pub const IBUS_ORIENTATION_HORIZONTAL: i32 = 0;

/// IBus 候选框排列方向：垂直纵向排列 (`IBUS_ORIENTATION_VERTICAL`)
pub const IBUS_ORIENTATION_VERTICAL: i32 = 1;

/// IBus 属性类型：普通按钮项 (`PROP_TYPE_NORMAL`)
const IBUS_PROP_TYPE_NORMAL: u32 = 0;

/// IBus 属性状态：未选中 (`PROP_STATE_UNCHECKED`)
const IBUS_PROP_STATE_UNCHECKED: u32 = 0;

/// 构造空的附件字典 `a{sv}`
fn empty_attachments() -> HashMap<String, OwnedValue> {
    HashMap::with_capacity(0)
}

/// 构造空的变体列表 `av`
fn empty_variant_vec() -> Vec<OwnedValue> {
    Vec::with_capacity(0)
}

/// 构造空的 `IBusAttrList` 变体对象，签名 `(sa{sv}av)`
///
/// # Errors
/// 当 `zvariant` 序列化转换失败时返回错误。
pub fn build_empty_attr_list() -> Result<OwnedValue, zbus::zvariant::Error> {
    let s = StructureBuilder::new()
        .add_field("IBusAttrList")
        .add_field(empty_attachments())
        .add_field(empty_variant_vec())
        .build();
    OwnedValue::try_from(Value::from(s))
}

/// 构造不带额外样式属性的纯文本 `IBusText` 变体对象，签名 `(sa{sv}sv)`
///
/// # Errors
/// 当 `zvariant` 序列化转换失败时返回错误。
pub fn build_ibus_text(text: &str) -> Result<OwnedValue, zbus::zvariant::Error> {
    let attr_list = build_empty_attr_list()?;
    let s = StructureBuilder::new()
        .add_field("IBusText")
        .add_field(empty_attachments())
        .add_field(text.to_string())
        .add_field(attr_list)
        .build();
    OwnedValue::try_from(Value::from(s))
}

/// 构造带单下划线装饰的预编辑 `IBusText` 变体对象，签名 `(sa{sv}sv)`
///
/// # Errors
/// 当 `zvariant` 序列化转换失败时返回错误。
pub fn build_ibus_preedit_text(text: &str) -> Result<OwnedValue, zbus::zvariant::Error> {
    let char_len = text.chars().count() as u32;
    let mut attrs = Vec::with_capacity(1);
    if char_len > 0 {
        let attr_struct = StructureBuilder::new()
            .add_field("IBusAttribute")
            .add_field(empty_attachments())
            .add_field(IBUS_ATTR_TYPE_UNDERLINE)
            .add_field(IBUS_ATTR_UNDERLINE_SINGLE)
            .add_field(0u32)
            .add_field(char_len)
            .build();
        attrs.push(OwnedValue::try_from(Value::from(attr_struct))?);
    }

    let attr_list_struct = StructureBuilder::new()
        .add_field("IBusAttrList")
        .add_field(empty_attachments())
        .add_field(attrs)
        .build();
    let attr_list = OwnedValue::try_from(Value::from(attr_list_struct))?;

    let s = StructureBuilder::new()
        .add_field("IBusText")
        .add_field(empty_attachments())
        .add_field(text.to_string())
        .add_field(attr_list)
        .build();
    OwnedValue::try_from(Value::from(s))
}

/// 构造包含当前页候选词列表的 `IBusLookupTable` 变体对象，签名 `(sa{sv}uubbiavav)`
///
/// # Errors
/// 当候选词文本或表格序列化失败时返回错误。
pub fn build_ibus_lookup_table(
    candidates: &[Candidate],
    page_size: usize,
    selected_index: usize,
    orientation: i32,
) -> Result<OwnedValue, zbus::zvariant::Error> {
    let mut cand_vec = Vec::with_capacity(candidates.len());
    let mut label_vec = Vec::with_capacity(candidates.len());

    for (idx, cand) in candidates.iter().enumerate() {
        cand_vec.push(build_ibus_text(&cand.text)?);
        let label_num = (idx + 1) % 10;
        label_vec.push(build_ibus_text(&format!("{}.", label_num))?);
    }

    let clamped_page_size = page_size.clamp(1, 10) as u32;
    let cursor_pos = if candidates.is_empty() {
        0
    } else {
        selected_index.min(candidates.len().saturating_sub(1)) as u32
    };

    let s = StructureBuilder::new()
        .add_field("IBusLookupTable")
        .add_field(empty_attachments())
        .add_field(clamped_page_size)
        .add_field(cursor_pos)
        .add_field(!candidates.is_empty())
        .add_field(false)
        .add_field(orientation)
        .add_field(cand_vec)
        .add_field(label_vec)
        .build();
    OwnedValue::try_from(Value::from(s))
}

/// 解析输入模式对应的微澜矢量图标（优先规范绝对路径以确保 GNOME 原生面板直接加载）
pub fn resolve_mode_icon(mode: InputMode, is_caps_locked: bool) -> String {
    let base_name = if is_caps_locked {
        "tray-en-caps"
    } else if matches!(mode, InputMode::Chinese | InputMode::VMode) {
        "tray-zh"
    } else {
        "tray-en"
    };
    if let Some(assets) = crate::tray::item::detect_default_assets_path() {
        crate::tray::item::resolve_icon_path_or_name(&assets, base_name)
    } else {
        base_name.to_string()
    }
}

/// 解析微澜引擎主图标（优先规范绝对路径）
pub fn resolve_engine_icon() -> String {
    if let Some(assets) = crate::tray::item::detect_default_assets_path() {
        crate::tray::item::resolve_icon_path_or_name(&assets, "waylan")
    } else {
        "waylan".to_string()
    }
}

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
) -> Result<OwnedValue, zbus::zvariant::Error> {
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
) -> Result<OwnedValue, zbus::zvariant::Error> {
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
pub fn build_traditional_property(
    traditional_chinese: bool,
) -> Result<OwnedValue, zbus::zvariant::Error> {
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
pub fn build_full_width_punct_property(
    full_width_punct: bool,
) -> Result<OwnedValue, zbus::zvariant::Error> {
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
pub fn build_full_width_char_property(
    full_width_char: bool,
) -> Result<OwnedValue, zbus::zvariant::Error> {
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
) -> Result<OwnedValue, zbus::zvariant::Error> {
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

/// 构造微澜输入法的 `IBusComponent` 变体对象（用于运行时动态向 `ibus-daemon` 注册引擎），签名 `(sa{sv}ssssssssavav)`
///
/// # Errors
/// 当组件或引擎描述序列化失败时返回错误。
pub fn build_waylan_ibus_component(
    exec_cmd: &str,
    setup_cmd: &str,
) -> Result<OwnedValue, zbus::zvariant::Error> {
    let version = env!("CARGO_PKG_VERSION");
    let engine_icon = resolve_engine_icon();
    let engine_desc = StructureBuilder::new()
        .add_field("IBusEngineDesc")
        .add_field(empty_attachments())
        .add_field("waylan")
        .add_field("微澜 (WayLan)")
        .add_field("微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法")
        .add_field("zh_CN")
        .add_field("MIT OR Apache-2.0")
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
        .add_field("MIT OR Apache-2.0")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ibus_text_and_preedit_signature() {
        let text_val = build_ibus_text("你好").expect("序列化 IBusText 失败");
        assert_eq!(text_val.value_signature().as_str(), "(sa{sv}sv)");

        let preedit_val = build_ibus_preedit_text("ni'hao").expect("序列化预编辑 IBusText 失败");
        assert_eq!(preedit_val.value_signature().as_str(), "(sa{sv}sv)");

        let extracted = extract_text_from_ibus_variant(&text_val);
        assert_eq!(extracted.as_deref(), Some("你好"));
    }

    #[test]
    fn test_ibus_lookup_table_signature() {
        let cands = vec![
            Candidate {
                text: "微澜".to_string(),
                pinyin: "wei'lan".to_string(),
                score: 1000,
                is_expandable: false,
                sub_candidates: Vec::new(),
            },
            Candidate {
                text: "未来".to_string(),
                pinyin: "wei'lai".to_string(),
                score: 900,
                is_expandable: false,
                sub_candidates: Vec::new(),
            },
        ];
        let table = build_ibus_lookup_table(&cands, 7, 0, IBUS_ORIENTATION_HORIZONTAL)
            .expect("序列化 IBusLookupTable 失败");
        assert_eq!(table.value_signature().as_str(), "(sa{sv}uubbiavav)");
    }

    #[test]
    fn test_ibus_prop_and_component_signature() {
        let prop =
            build_input_mode_property(InputMode::Chinese, false).expect("序列化 IBusProperty 失败");
        assert_eq!(prop.value_signature().as_str(), "(sa{sv}suvsvbbuvv)");

        let prop_list = build_ibus_prop_list(InputMode::Chinese, false, false, true, false)
            .expect("序列化 IBusPropList 失败");
        assert_eq!(prop_list.value_signature().as_str(), "(sa{sv}av)");

        let comp =
            build_waylan_ibus_component("/usr/bin/waylan --ibus", "/usr/bin/waylan settings")
                .expect("序列化 IBusComponent 失败");
        assert_eq!(comp.value_signature().as_str(), "(sa{sv}ssssssssavav)");
    }
}
