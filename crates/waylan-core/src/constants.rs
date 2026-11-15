//! 微澜核心系统级与资源安全容量规格上限常量
//!
//! # 设计原理
//! - **实现初衷**：在核心库统一定义系统资源安全边界常量，避免各个业务子模块各自硬编码引发规格分歧。
//! - **核心优势**：为外部非受信任文件解析与反序列化提供统一的安全防爆内存护盾，消除 OOM 隐患。
//! - **代价与局限**：若用户确实需要导入超大规模词库，需在调用层分批或扩展常量上限。

/// 外部词库文件（搜狗细胞词库 .scel、Rime YAML、纯文本）导入的最大允许文件体积上限（64 MB）
pub const MAX_IMPORT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 用户自适应历史词库持久化文件最大允许读取上限（64 MB）
pub const MAX_USER_DICT_BYTES: u64 = 64 * 1024 * 1024;

/// 主配置文件 (`config.toml`) 最大允许读取上限（4 MB）
///
/// 配置文件为纯人工书写的偏好声明，正常体积在数十 KB 量级；
/// 超出该上限即视为异常膨胀或损坏，必须拒绝整体读入内存。
pub const MAX_CONFIG_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// 用户自定义短语文件 (`phrases.txt`) 最大允许读取上限（8 MB）
pub const MAX_PHRASE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// 简繁转换词库（紧凑二进制与 OpenCC 标准 TSV 源）最大允许读取上限（64 MB）
pub const MAX_CONVERT_DICT_BYTES: u64 = 64 * 1024 * 1024;

/// 微澜 GNOME Shell 集成扩展 UUID
pub const WAYLAN_GNOME_EXTENSION_UUID: &str = "waylan-hide-input-source@waylan.org";

/// 微澜在 GNOME `org.gnome.desktop.input-sources` 中的输入源项标识
pub const WAYLAN_IBUS_SOURCE_ENTRY: &str = "('ibus', 'waylan')";

/// 微澜在 IBus 守护进程中的引擎名称
pub const WAYLAN_IBUS_ENGINE_NAME: &str = "waylan";

/// 微澜 GNOME Shell 扩展启用意图
///
/// # 设计原理
/// - **实现初衷**：配套 GNOME 扩展同时承担“隐藏顶栏原生输入源指示器”与“接管渲染微澜自定义候选框”两项职责，
///   任一职责启用即须加载扩展，两项设置互不蕴含、互不牵连。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GnomeExtensionIntent {
    /// 是否需要隐藏顶栏原生输入源指示器（对应独占输入源模式）
    pub hide_indicator: bool,
    /// 是否需要由扩展渲染微澜自定义候选框（对应候选框模式为微澜专属）
    pub custom_candidate_view: bool,
}

impl GnomeExtensionIntent {
    /// 扩展是否应当处于加载状态：两职任一需要即须加载
    #[must_use]
    pub fn should_enable(self) -> bool {
        self.hide_indicator || self.custom_candidate_view
    }
}
