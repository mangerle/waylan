use serde::{Deserialize, Serialize};


/// 模糊音细粒度配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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
}

impl FuzzyConfig {
    /// 创建开启全部 9 类模糊音规则的配置
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
    }
}

/// 微澜算法引擎全局运行配置
///
/// # 设计原理
/// - **实现初衷**：采用参数对象模式统一收敛算法核心的各项开关与阈值，防止多参数平铺。
/// - **核心优势**：字段明确，具备标准 Default 实现，支持序列化并与配置文件无缝对接。
/// - **代价与局限**：新增细分配置项需扩展结构体，需保持与外层 config.toml 映射一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// 候选词优先级层级（数值越大优先级越高）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum CandidateTier {
    /// 部分前缀匹配或残余拼音
    PrefixPartial = 1,
    /// 次位邻键容错纠错候选
    TypoCorrection = 2,
    /// 首字母缩写简拼（如 bj 匹配 北京，yyds 匹配 永远的神）
    Acronym = 3,
    /// 整句预测（Viterbi 多音节长句拼接）
    SentenceViterbi = 4,
    /// 用户自适应学习词或动态自造词
    UserHistory = 5,
    /// 完整整词匹配（词库中完整匹配的成词，如“我们”、“中国”、“西安”、“先”）
    #[default]
    ExactWord = 6,
    /// 专有名词与受保护实体（如技术专名、中英实体，享有最高置顶优先级）
    ProtectedEntity = 7,
}

/// 算法内部候选词结构
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlgoCandidate {
    /// 候选汉字或词条文本
    pub text: String,
    /// 对应拼音切分或来源编码
    pub pinyin: String,
    /// 综合打分（静态词频 + 动态习惯加权）
    pub score: u32,
    /// 候选附加注释信息（如反查提示、计算结果来源）
    pub comment: Option<String>,
    /// 候选词优先级层级
    pub tier: CandidateTier,
    /// 是否为可展开的二级动态功能项
    pub is_expandable: bool,
    /// 二级子候选列表（如多种展开的日期或时间格式）
    pub sub_candidates: Vec<String>,
}

impl AlgoCandidate {
    /// 创建新的算法候选词（默认 ExactWord 级别）
    pub fn new(text: impl Into<String>, pinyin: impl Into<String>, score: u32) -> Self {
        Self {
            text: text.into(),
            pinyin: pinyin.into(),
            score,
            comment: None,
            tier: CandidateTier::ExactWord,
            is_expandable: false,
            sub_candidates: Vec::new(),
        }
    }

    /// 附加注释说明
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// 指定优先级层级
    pub fn with_tier(mut self, tier: CandidateTier) -> Self {
        self.tier = tier;
        self
    }

    /// 标记为可展开并挂载二级子候选格式列表
    pub fn with_sub_candidates(mut self, sub_candidates: Vec<String>) -> Self {
        self.is_expandable = !sub_candidates.is_empty();
        self.sub_candidates = sub_candidates;
        self
    }

    /// 转换为 Core 层定义的公共候选词结构体
    pub fn to_core_candidate(&self) -> waylan_core::Candidate {
        waylan_core::Candidate::new(&self.text, &self.pinyin, self.score)
            .with_sub_candidates(self.sub_candidates.clone())
    }
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
}
