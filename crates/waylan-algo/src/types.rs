use serde::{Deserialize, Serialize};
use waylan_core::config::FuzzyPinyinRules;
use waylan_core::AppConfig;

/// 模糊音细粒度规则配置别名（统一收敛至 core::config::FuzzyPinyinRules，消除重复抽象）
pub type FuzzyConfig = FuzzyPinyinRules;

/// 微澜算法引擎全局运行配置
///
/// # 设计原理
/// - **实现初衷**：采用参数对象模式统一收敛算法核心的各项开关与阈值，防止多参数平铺。
/// - **核心优势**：字段明确，具备标准 Default 实现，支持序列化并与配置文件无缝对接。
/// - **代价与局限**：新增细分配置项需扩展结构体，需保持与外层 config.toml 映射一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlgoConfig {
    /// 单页候选词数量（默认 7）
    pub page_size: usize,
    /// 是否开启 V 模式表达式求值与快捷计算
    pub enable_v_mode: bool,
    /// 是否开启 U 模式拆字与笔画反查
    pub enable_u_mode: bool,
    /// 是否开启 I 模式特殊符号与序号选择
    pub enable_i_mode: bool,
    /// 是否开启繁体中文转换输出
    pub traditional_chinese: bool,
    /// 是否过滤超出常用规范汉字范围的生僻字符
    pub filter_uncommon_charset: bool,
    /// 是否在候选词旁边附加拼音/注释提示
    pub show_pinyin_comment: bool,
    /// 是否开启多路候选词自动去重
    pub auto_dedup: bool,
    /// 模糊音配置
    pub fuzzy: FuzzyConfig,
    /// 是否开启中西文智能空格（盘古之白，在中英文/数字交界处自动补齐半角空格）
    pub auto_pangu_spacing: bool,
}

impl Default for AlgoConfig {
    fn default() -> Self {
        Self {
            page_size: 7,
            enable_v_mode: true,
            enable_u_mode: true,
            enable_i_mode: true,
            traditional_chinese: false,
            filter_uncommon_charset: false,
            show_pinyin_comment: true,
            auto_dedup: true,
            fuzzy: FuzzyConfig::default(),
            auto_pangu_spacing: false,
        }
    }
}

impl From<&AppConfig> for AlgoConfig {
    fn from(cfg: &AppConfig) -> Self {
        Self {
            page_size: cfg.ui.page_size,
            enable_v_mode: cfg.pinyin.v_mode,
            enable_u_mode: cfg.pinyin.enable_u_mode,
            enable_i_mode: cfg.pinyin.enable_i_mode,
            traditional_chinese: cfg.pinyin.traditional_chinese,
            filter_uncommon_charset: cfg.pinyin.filter_uncommon_charset,
            show_pinyin_comment: cfg.ui.show_pinyin_comment,
            auto_dedup: cfg.pinyin.auto_dedup,
            fuzzy: if cfg.pinyin.fuzzy_pinyin {
                cfg.pinyin.fuzzy_rules
            } else {
                FuzzyConfig::default()
            },
            auto_pangu_spacing: cfg.pinyin.auto_pangu_spacing,
        }
    }
}

pub use waylan_core::{Candidate, CandidateTier};

/// 算法内部候选词结构（别名，全链路与 Core 统一契约）
pub type AlgoCandidate = Candidate;

/// 外部词库导入条目数据契约 (文本, 拼音序列, 基础频次权重)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedDictEntry {
    /// 词条文本内容
    pub text: String,
    /// 拼音序列（如 "ni'hao"）
    pub pinyin: String,
    /// 基础频次权重
    pub weight: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AlgoConfig::default();
        assert_eq!(config.page_size, 7);
        assert!(config.enable_v_mode);
        assert!(config.auto_dedup);
        assert!(!config.traditional_chinese);
    }

    #[test]
    fn test_fuzzy_config_all_enabled_and_active_rules() {
        let default_fuzzy = FuzzyConfig::default();
        assert!(!default_fuzzy.has_active_rules());
        assert!(!default_fuzzy.c_ch);
        assert!(!default_fuzzy.r_l);

        let rec = FuzzyConfig::recommended();
        assert!(rec.has_active_rules());
        assert!(rec.c_ch);
        assert!(!rec.r_l);

        let all = FuzzyConfig::all_enabled();
        assert!(all.has_active_rules());
        assert!(all.r_l);
        assert!(all.g_k);
    }
}
