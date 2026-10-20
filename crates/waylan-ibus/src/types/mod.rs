//! IBus D-Bus 协议可序列化对象（IBusSerializable）构建与解析模块
//!
//! # 设计原理
//! - **实现初衷**：IBus 守护进程 (`ibus-daemon`) 与 GNOME Shell 面板之间的 D-Bus 通信大量使用自描述的
//!   `GVariant` 包装结构（`IBusText`、`IBusLookupTable`、`IBusPropList`、`IBusComponent` 等）。
//!   本模块直接基于项目已有的 `zbus::zvariant::StructureBuilder` 构建符合 `libibus-1.0` 二进制规范的变体数据，无需引入 C 语言 `libibus` 绑定。
//! - **核心优势**：纯 Rust 零 FFI 开销，模块化划分为 text、property、component 三大子领域，各司其职。
//! - **代价与局限**：需严格遵循 IBus 1.5 规范的字段顺序。

pub mod component;
pub mod property;
pub mod text;

pub use component::*;
pub use property::*;
pub use text::*;

use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

/// IBus 预编辑失焦行为：清空预编辑 (`IBUS_ENGINE_PREEDIT_CLEAR`)
pub const IBUS_ENGINE_PREEDIT_CLEAR: u32 = 0;

/// IBus 候选框排列方向：水平排列 (`IBUS_ORIENTATION_HORIZONTAL`)
pub const IBUS_ORIENTATION_HORIZONTAL: i32 = 0;

/// IBus 候选框排列方向：垂直纵向排列 (`IBUS_ORIENTATION_VERTICAL`)
pub const IBUS_ORIENTATION_VERTICAL: i32 = 1;

/// 构造空的附件字典 `a{sv}`
pub(crate) fn empty_attachments() -> HashMap<String, OwnedValue> {
    HashMap::with_capacity(0)
}

/// 构造空的变体列表 `av`
pub(crate) fn empty_variant_vec() -> Vec<OwnedValue> {
    Vec::with_capacity(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::{Candidate, CandidateIndexStyle, InputMode};

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
            Candidate::new("微澜", "wei'lan", 1000),
            Candidate::new("未来", "wei'lai", 900),
        ];
        let table = build_ibus_lookup_table(&IbusLookupTableParams {
            candidates: &cands,
            page_size: 5,
            selected_index: 0,
            orientation: IBUS_ORIENTATION_HORIZONTAL,
            index_style: CandidateIndexStyle::ModernSpace,
        })
        .expect("序列化 IBusLookupTable 失败");
        assert_eq!(table.value_signature().as_str(), "(sa{sv}uubbiavav)");
    }

    #[test]
    fn test_ibus_prop_list_keys_match_activatable_set() {
        let prop_list = build_ibus_prop_list(&IbusPropListParams {
            mode: InputMode::Chinese,
            is_caps_locked: false,
            traditional_chinese: false,
            full_width_punct: true,
            full_width_char: false,
        })
        .expect("序列化 IBusPropList 失败");

        let keys = extract_prop_keys(&prop_list).expect("属性列表结构解析失败");
        assert_eq!(
            keys,
            IBUS_ACTIVATABLE_KEYS.to_vec(),
            "属性树键集合必须与激活分派表共用同一常量，顺序与数量都需一致"
        );
    }

    /// 逐层解开 zvariant 装箱，取出 `IBusProperty` 结构体的键名字段（第 3 个字段）
    fn unwrap_structure<'a>(
        val: &'a zbus::zvariant::Value<'a>,
    ) -> Option<&'a [zbus::zvariant::Value<'a>]> {
        let mut inner = val;
        while let zbus::zvariant::Value::Value(v) = inner {
            inner = v.as_ref();
        }
        match inner {
            zbus::zvariant::Value::Structure(s) => Some(s.fields()),
            _ => None,
        }
    }

    /// 从 `IBusPropList` 变体中按序提取全部属性键名
    fn extract_prop_keys(prop_list: &zbus::zvariant::OwnedValue) -> Option<Vec<String>> {
        let fields = unwrap_structure(prop_list)?;
        let props_value = &fields[2];
        let mut inner = props_value;
        while let zbus::zvariant::Value::Value(v) = inner {
            inner = v.as_ref();
        }
        let array = match inner {
            zbus::zvariant::Value::Array(arr) => arr,
            _ => return None,
        };
        let mut keys = Vec::with_capacity(array.len());
        for item in array.iter() {
            let prop_fields = unwrap_structure(item)?;
            match &prop_fields[2] {
                zbus::zvariant::Value::Str(text) => keys.push(text.as_str().to_string()),
                _ => return None,
            }
        }
        Some(keys)
    }

    #[test]
    fn test_ibus_prop_and_component_signature() {
        let prop =
            build_input_mode_property(InputMode::Chinese, false).expect("序列化 IBusProperty 失败");
        assert_eq!(prop.value_signature().as_str(), "(sa{sv}suvsvbbuvv)");

        let prop_list = build_ibus_prop_list(&IbusPropListParams {
            mode: InputMode::Chinese,
            is_caps_locked: false,
            traditional_chinese: false,
            full_width_punct: true,
            full_width_char: false,
        })
        .expect("序列化 IBusPropList 失败");
        assert_eq!(prop_list.value_signature().as_str(), "(sa{sv}av)");

        let comp =
            build_waylan_ibus_component("/usr/bin/waylan --ibus", "/usr/bin/waylan settings")
                .expect("序列化 IBusComponent 失败");
        assert_eq!(comp.value_signature().as_str(), "(sa{sv}ssssssssavav)");
    }
}
