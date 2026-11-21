//! 静态只读词典与语言学规则资源包
//!
//! # 设计原理
//! - **实现初衷**：将只读词典表、简繁字典、辅助词库与实体保护管理器聚合为独立资源包，
//!   解耦静态无状态资源与单次输入会话的动态状态机。
//! - **核心优势**：职责清晰独立，为多会话共享只读词典或零拷贝加载打下类型基础；
//!   简繁转换词库采用按需懒加载，未启用繁体输出时不占用守护进程启动内存与解析耗时。
//! - **代价与局限**：外部更新辅助词库时需经由资源包中继；首次触发繁体转换时存在一次性词库载入延迟。

use std::sync::{Arc, OnceLock};

use crate::custom_phrase::{
    AuxiliaryDict, CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager,
};
use crate::entity::EntityProtectionManager;
use crate::opencc::OpenCcDict;
use crate::table::LexiconTable;

/// 算法引擎静态词典与语言学规则资源包
#[derive(Debug, Default)]
pub struct LexiconResources {
    /// 静态系统词典表
    pub table: LexiconTable,
    /// 自定义短语管理器
    pub custom_phrases: CustomPhraseManager,
    /// 英文单词联想管理器
    pub english_suggester: Arc<EnglishSuggestionManager>,
    /// 符号表情联想管理器
    pub symbol_suggester: Arc<SymbolSuggestionManager>,
    /// 专有名词与实体保护管理器
    pub entity_manager: EntityProtectionManager,
    /// 简繁转换两级词库引擎（STPhrases + STCharacters，按需懒加载）
    opencc_dict: OnceLock<Arc<OpenCcDict>>,
}

impl LexiconResources {
    /// 使用指定词典表构建资源包
    pub fn new(table: LexiconTable) -> Self {
        Self {
            table,
            custom_phrases: CustomPhraseManager::new(),
            english_suggester: EnglishSuggestionManager::default_global(),
            symbol_suggester: SymbolSuggestionManager::default_global(),
            entity_manager: EntityProtectionManager::new(),
            opencc_dict: OnceLock::new(),
        }
    }

    /// 使用内置样例数据创建资源包（用于测试）
    pub fn with_sample_data() -> Self {
        Self::new(LexiconTable::with_sample_data())
    }

    /// 载入外部辅助与混输二进制词库（包含英文词库与符号表情库）
    pub fn load_auxiliary_dict(&mut self, dict: AuxiliaryDict) {
        let eng = Arc::new(EnglishSuggestionManager::from_words(dict.english_words));
        EnglishSuggestionManager::set_active_global(eng.clone());
        self.english_suggester = eng;
        self.symbol_suggester = Arc::new(SymbolSuggestionManager::from_symbols(dict.symbols));
    }

    /// 设置外部预构建并以 Arc 共享的英文与符号建议管理器（零拷贝直接复用已分配内存）
    pub fn set_suggestion_managers(
        &mut self,
        english: Arc<EnglishSuggestionManager>,
        symbols: Arc<SymbolSuggestionManager>,
    ) {
        EnglishSuggestionManager::set_active_global(english.clone());
        self.english_suggester = english;
        self.symbol_suggester = symbols;
    }

    /// 获取简繁转换词典（按需懒加载，未启用繁体输出时不占用启动内存与解析耗时）
    pub fn opencc_dict(&self) -> &OpenCcDict {
        self.opencc_dict.get_or_init(OpenCcDict::global)
    }

    /// 获取当前生效的高频英文混输词目数量
    #[inline]
    pub fn english_words_count(&self) -> usize {
        self.english_suggester.len()
    }

    /// 获取当前生效的符号与表情映射条目数量
    #[inline]
    pub fn symbols_count(&self) -> usize {
        self.symbol_suggester.len()
    }

    /// 动态注册新的受保护专有名词与实体
    #[inline]
    pub fn register_entity(&mut self, entity: &str) {
        self.entity_manager.register_entity(entity);
    }
}
