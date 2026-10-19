//! IBus 文本与候选词查找表可序列化对象构建
//!
//! # 设计原理
//! - **实现初衷**：为 IBus 输入法引擎提供符合 `libibus-1.0` 二进制规范的文本、预编辑带下划线文本及候选词列表 GVariant 构建。
//! - **核心优势**：纯 Rust 原生构造，零 FFI 与零外部 C 库依赖，性能极高。
//! - **代价与局限**：严格依赖 IBus 1.5 规定的字段布局。

use super::{empty_attachments, empty_variant_vec, IBUS_ORIENTATION_VERTICAL};
use waylan_core::{format_candidate_display, Candidate, CandidateIndexStyle};
use zbus::zvariant::{Error as VariantError, OwnedValue, StructureBuilder, Value};

/// IBus 文本属性类型：下划线 (`IBUS_ATTR_TYPE_UNDERLINE`)
pub const IBUS_ATTR_TYPE_UNDERLINE: u32 = 1;

/// IBus 下划线样式：单实线 (`IBUS_ATTR_UNDERLINE_SINGLE`)
pub const IBUS_ATTR_UNDERLINE_SINGLE: u32 = 1;

/// 构造空的 `IBusAttrList` 变体对象，签名 `(sa{sv}av)`
///
/// # Errors
/// 当 `zvariant` 序列化转换失败时返回错误。
pub fn build_empty_attr_list() -> Result<OwnedValue, VariantError> {
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
pub fn build_ibus_text(text: &str) -> Result<OwnedValue, VariantError> {
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
pub fn build_ibus_preedit_text(text: &str) -> Result<OwnedValue, VariantError> {
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
    index_style: CandidateIndexStyle,
) -> Result<OwnedValue, VariantError> {
    let mut cand_vec = Vec::with_capacity(candidates.len());
    let mut label_vec = Vec::with_capacity(candidates.len());

    for (idx, cand) in candidates.iter().enumerate() {
        let mut display_text = format_candidate_display(index_style, idx, &cand.text, None);
        if cand.is_expandable && orientation != IBUS_ORIENTATION_VERTICAL {
            display_text.push_str(" ⇥ Tab");
        }
        cand_vec.push(build_ibus_text(&display_text)?);
        // 关键隔离防护：仅在语音录入单条目状态下，允许通过 label 传递波形元数据；
        // 普通键盘组词打字时，100% 严禁暴露拼音到 label 中，必须使用零宽空格 (\u{200B}) 阻断原生序号
        let is_voice_session = candidates.len() == 1 && candidates[0].text.starts_with("正在聆听");
        let label_text = if is_voice_session && !cand.pinyin.is_empty() {
            cand.pinyin.as_str()
        } else {
            "\u{200B}"
        };
        label_vec.push(build_ibus_text(label_text)?);
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
