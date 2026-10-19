use serde::{Deserialize, Serialize};
use waylan_core::config::FuzzyPinyinRules;
use waylan_core::AppConfig;

/// 模糊音细粒度配置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FuzzyConfig {
    /// 平翘舌: z <=> zh
    pub z_zh: bool,
    /// 平翘舌: c <=> ch
    pub c_ch: bool,
    /// 平翘舌: s <=> sh
    pub s_sh: bool,
    /// 前后鼻音: an <=> ang
    pub an_ang: bool,
    /// 前后鼻音: en <=> eng
    pub en_eng: bool,
    /// 前后鼻音: in <=> ing
    pub in_ing: bool,
    /// 边鼻音: l <=> n
    pub l_n: bool,
    /// 唇齿与舌根擦音: f <=> h
    #[serde(default)]
    pub f_h: bool,
    /// 卷舌与边音: r <=> l
    #[serde(default)]
    pub r_l: bool,
    /// 舌根塞音: g <=> k
    #[serde(default)]
    pub g_k: bool,
}

impl FuzzyConfig {
    /// 创建开启全部 10 类模糊音规则的配置
    pub fn all_enabled() -> Self {
        Self {
            z_zh: true,
            c_ch: true,
            s_sh: true,
            an_ang: true,
            en_eng: true,
            in_ing: true,
            l_n: true,
            f_h: true,
            r_l: true,
            g_k: true,
        }
    }

    /// 检查是否存在任一激活的模糊音规则
    pub fn has_active_rules(&self) -> bool {
        self.z_zh
            || self.c_ch
            || self.s_sh
            || self.an_ang
            || self.en_eng
            || self.in_ing
            || self.l_n
            || self.f_h
            || self.r_l
            || self.g_k
    }
}

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

impl From<FuzzyPinyinRules> for FuzzyConfig {
    fn from(r: FuzzyPinyinRules) -> Self {
        Self {
            z_zh: r.z_zh,
            c_ch: r.c_ch,
            s_sh: r.s_sh,
            an_ang: r.an_ang,
            en_eng: r.en_eng,
            in_ing: r.in_ing,
            l_n: r.l_n,
            f_h: r.f_h,
            r_l: r.r_l,
            g_k: r.g_k,
        }
    }
}

impl From<&FuzzyPinyinRules> for FuzzyConfig {
    fn from(r: &FuzzyPinyinRules) -> Self {
        Self::from(*r)
    }
}

impl From<FuzzyConfig> for FuzzyPinyinRules {
    fn from(c: FuzzyConfig) -> Self {
        Self {
            c_ch: c.c_ch,
            z_zh: c.z_zh,
            s_sh: c.s_sh,
            an_ang: c.an_ang,
            en_eng: c.en_eng,
            in_ing: c.in_ing,
            l_n: c.l_n,
            f_h: c.f_h,
            r_l: c.r_l,
            g_k: c.g_k,
        }
    }
}

impl From<&FuzzyConfig> for FuzzyPinyinRules {
    fn from(c: &FuzzyConfig) -> Self {
        Self::from(*c)
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
                FuzzyConfig::from(cfg.pinyin.fuzzy_rules)
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
    fn test_fuzzy_config_conversion() {
        let rules = FuzzyPinyinRules {
            c_ch: true,
            z_zh: false,
            s_sh: true,
            an_ang: false,
            en_eng: true,
            in_ing: false,
            l_n: true,
            f_h: false,
            r_l: true,
            g_k: false,
        };

        let fuzzy_cfg = FuzzyConfig::from(rules);
        assert!(fuzzy_cfg.c_ch);
        assert!(!fuzzy_cfg.z_zh);
        assert!(fuzzy_cfg.s_sh);
        assert!(!fuzzy_cfg.an_ang);
        assert!(fuzzy_cfg.en_eng);
        assert!(!fuzzy_cfg.in_ing);
        assert!(fuzzy_cfg.l_n);
        assert!(!fuzzy_cfg.f_h);
        assert!(fuzzy_cfg.r_l);
        assert!(!fuzzy_cfg.g_k);

        let converted_back = FuzzyPinyinRules::from(fuzzy_cfg);
        assert_eq!(converted_back, rules);

        let from_ref = FuzzyConfig::from(&rules);
        assert_eq!(from_ref, fuzzy_cfg);
    }
}
