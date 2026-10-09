pub mod builder;
pub mod clipboard;
pub mod composition;
pub mod custom_phrase;
pub mod dict_importer;
pub mod dispatcher;
pub mod error;
pub mod evaluator;
pub mod pipeline;
pub mod preprocessor;
pub mod prism;
pub mod syllables;
pub mod table;
pub mod types;
pub mod user_adaptive;
pub mod viterbi;

use clipboard::ClipboardManager;
use composition::Composition;
pub use custom_phrase::AuxiliaryDict;
use custom_phrase::{CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager};
use dict_importer::ImportedDictEntry;
use dispatcher::ModeDispatcher;
use evaluator::DateTimeEvaluator;
use pipeline::CandidatePipeline;
use prism::PrismEngine;
use table::LexiconTable;
use types::{AlgoCandidate, AlgoConfig};
use user_adaptive::UserAdaptiveModel;
use viterbi::ViterbiDecoder;

/// 微澜统一核心算法引擎门面 (AlgoEngine)
///
/// # 设计原理
/// - **实现初衷**：将模式分流、DAG 拼写图、词典倒排、Viterbi 解码、自适应学习与后处理流水线封装为高内聚门面，
///   向输入法上层提供极简的按键响应与选词接口。
/// - **核心优势**：完全纯 Rust 实现，零 C++ 依赖；全生命周期无锁、无异步竞态；100% 内存安全与可测试。
/// - **代价与局限**：门面结构体聚合了多个子模块状态，但在单进程输入法模型下保证了单一状态真实源（Single Source of Truth）。
#[derive(Debug, Default)]
pub struct AlgoEngine {
    /// 静态系统词典表
    table: LexiconTable,
    /// 用户自适应词频模型
    pub user_model: UserAdaptiveModel,
    /// 剪贴板历史管理器
    clipboard: ClipboardManager,
    /// 自定义短语管理器
    pub custom_phrases: CustomPhraseManager,
    /// 英文单词联想管理器
    english_suggester: EnglishSuggestionManager,
    /// 符号表情联想管理器
    symbol_suggester: SymbolSuggestionManager,
    /// 增量敲击编辑状态机
    composition: Composition,
    /// 全局算法配置
    config: AlgoConfig,
    /// 当前候选词分页索引（0-indexed）
    page_index: usize,
    /// 当前呈现的清洗后候选词缓存
    current_candidates: Vec<AlgoCandidate>,
    /// 二级扩展卡片是否展开
    pub is_expanded: bool,
    /// 二级扩展卡片中当前选中的子项索引（0-indexed）
    pub expanded_selected_index: usize,
}

impl AlgoEngine {
    /// 使用指定词典表与配置创建算法引擎
    pub fn new(table: LexiconTable, config: AlgoConfig) -> Self {
        Self {
            table,
            user_model: UserAdaptiveModel::new(),
            clipboard: ClipboardManager::new(),
            custom_phrases: CustomPhraseManager::new(),
            english_suggester: EnglishSuggestionManager::new(),
            symbol_suggester: SymbolSuggestionManager::new(),
            composition: Composition::new(),
            config,
            page_index: 0,
            current_candidates: Vec::new(),
            is_expanded: false,
            expanded_selected_index: 0,
        }
    }

    /// 使用内置样例数据创建算法引擎（用于测试）
    pub fn with_sample_data(config: AlgoConfig) -> Self {
        Self::new(LexiconTable::with_sample_data(), config)
    }

    /// 载入外部辅助与混输二进制词库（包含英文词库与符号表情库）
    pub fn load_auxiliary_dict(&mut self, dict: AuxiliaryDict) {
        self.english_suggester = EnglishSuggestionManager::from_words(dict.english_words);
        self.symbol_suggester = SymbolSuggestionManager::from_symbols(dict.symbols);
    }

    /// 获取当前生效的高频英文混输词目数量
    pub fn english_words_count(&self) -> usize {
        self.english_suggester.len()
    }

    /// 获取当前生效的符号与表情映射条目数量
    pub fn symbols_count(&self) -> usize {
        self.symbol_suggester.len()
    }

    /// 键入一个字符
    pub fn push_char(&mut self, ch: char) -> &[AlgoCandidate] {
        self.composition.push_char(ch);
        self.page_index = 0;
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 退格删除一个字符
    pub fn pop_char(&mut self) -> &[AlgoCandidate] {
        self.composition.pop_char();
        self.page_index = 0;
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 光标左移
    pub fn move_cursor_left(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_left() {
            self.recompute_candidates();
        }
        &self.current_candidates
    }

    /// 光标右移
    pub fn move_cursor_right(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_right() {
            self.recompute_candidates();
        }
        &self.current_candidates
    }

    /// 候选翻下一页
    pub fn next_page(&mut self) -> &[AlgoCandidate] {
        self.page_index = self.page_index.saturating_add(1);
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 候选翻上一页
    pub fn prev_page(&mut self) -> &[AlgoCandidate] {
        self.page_index = self.page_index.saturating_sub(1);
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 选择指定索引的候选词上屏
    ///
    /// # 返回值
    /// 返回 `Some((上屏文本, 剩余未完成拼音长度))`；若索引无效则返回 `None`。
    pub fn select_candidate(
        &mut self,
        index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        if index >= self.current_candidates.len() {
            return None;
        }

        let selected = self.current_candidates[index].clone();

        // 若选中的是可展开项，且直接按数字键确认（未展开时）：
        // 优先将首个格式化子候选文本（如当前标准日期 2026年10月9日）上屏
        let commit_text = if selected.is_expandable && !selected.sub_candidates.is_empty() {
            selected.sub_candidates[0].clone()
        } else {
            selected.text.clone()
        };

        // 记录用户选词并动态调频
        self.user_model
            .record_commit(&selected.text, &selected.pinyin, current_timestamp);

        // 计算消费的拼音字母长度并执行分段确认（动态功能项消费全部输入）
        let consumed_len = if selected.is_expandable {
            self.composition.len()
        } else {
            Self::compute_consumed_len(self.composition.as_str(), &selected.pinyin)
        };
        self.composition.consume_prefix(consumed_len);

        self.page_index = 0;
        self.recompute_candidates();

        Some((commit_text, self.composition.len()))
    }

    /// 切换二级扩展卡片的展开状态（仅当当前选中的项支持二级展开时有效）
    pub fn toggle_expand(&mut self, selected_candidate_index: usize) -> bool {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if cand.is_expandable && !cand.sub_candidates.is_empty() {
                self.is_expanded = !self.is_expanded;
                self.expanded_selected_index = 0;
                return self.is_expanded;
            }
        }
        self.is_expanded = false;
        false
    }

    /// 在二级展开卡片中向下移动选中项
    pub fn next_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                self.expanded_selected_index =
                    (self.expanded_selected_index + 1) % cand.sub_candidates.len();
            }
        }
    }

    /// 在二级展开卡片中向上移动选中项
    pub fn prev_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                if self.expanded_selected_index == 0 {
                    self.expanded_selected_index = cand.sub_candidates.len().saturating_sub(1);
                } else {
                    self.expanded_selected_index = self.expanded_selected_index.saturating_sub(1);
                }
            }
        }
    }

    /// 在二级展开卡片中选中子项上屏
    pub fn select_expanded_item(
        &mut self,
        main_index: usize,
        sub_index: usize,
    ) -> Option<(String, usize)> {
        let cand = self.current_candidates.get(main_index)?;
        let text = cand.sub_candidates.get(sub_index)?.clone();

        self.composition.clear();
        self.page_index = 0;
        self.recompute_candidates();

        Some((text, 0))
    }

    /// 精确计算用户选词所消费的原始按键长度（含自动跳过隔音符）
    fn compute_consumed_len(raw: &str, pinyin: &str) -> usize {
        if raw.starts_with(pinyin) {
            let mut len = pinyin.len();
            if raw[len..].starts_with('\'') {
                len = len.saturating_add(1);
            }
            return len;
        }

        let clean_pinyin: String = pinyin.chars().filter(|&c| c != '\'').collect();
        let mut pinyin_iter = clean_pinyin.chars();
        let mut target_opt = pinyin_iter.next();
        let mut last_end = 0;

        for (idx, ch) in raw.char_indices() {
            if let Some(target) = target_opt {
                if ch == target {
                    last_end = idx.saturating_add(ch.len_utf8());
                    target_opt = pinyin_iter.next();
                } else if ch == '\'' {
                    last_end = idx.saturating_add(ch.len_utf8());
                }
            } else {
                if ch == '\'' {
                    last_end = idx.saturating_add(ch.len_utf8());
                }
                break;
            }
        }
        last_end
    }

    /// 重新计算当前候选词列表
    fn recompute_candidates(&mut self) {
        self.is_expanded = false;
        self.expanded_selected_index = 0;

        let raw_input = self.composition.as_str();
        if raw_input.is_empty() {
            self.current_candidates.clear();
            return;
        }

        // 1. 尝试前缀模式分流 (V计算器、U拆字、I特殊符号)
        if let Some(dispatched) = ModeDispatcher::try_dispatch(raw_input, &self.config) {
            self.current_candidates =
                CandidatePipeline::process(dispatched, &self.config, self.page_index);
            return;
        }

        // 2. 拼音模式：利用 Prism 构建 DAG 音节路径
        let paths =
            PrismEngine::build_paths(raw_input, &self.config.fuzzy, self.config.double_pinyin);

        let mut raw_candidates = Vec::new();

        for path in &paths {
            let key = path.to_key_string();

            // 词典精确及倒排检索
            let mut matches = self.table.lookup(&key);
            raw_candidates.append(&mut matches);

            // 用户自造词检索
            let mut user_matches = self.user_model.lookup_user_words(&key, 1_700_000_000);
            raw_candidates.append(&mut user_matches);

            // Viterbi 长句动态规划解码（多音节时生效）
            if path.syllables.len() >= 2 {
                if let Some(sentence) =
                    ViterbiDecoder::decode_sentence(&path.syllables, &self.table)
                {
                    raw_candidates.push(sentence);
                }
            }
        }

        // 2.2 检索首字母缩写简拼（如 bj -> 北京，yyds -> 永远的神）
        let mut acronym_matches = self.table.lookup_acronym(raw_input);
        raw_candidates.append(&mut acronym_matches);

        // 3. 动态应用用户习惯提权
        for c in &mut raw_candidates {
            self.user_model.apply_boost(c, 1_700_000_000);
        }

        // 4. 送入后处理流水线（去重、简繁、字符集、注释、分页）
        let mut candidates =
            CandidatePipeline::process(raw_candidates, &self.config, self.page_index);

        // 5. 动态扩展功能（日期、时间、干支等）注入：
        // 若命中动态日期/时间指令，将动态功能项插入到第 2 位（仅次于首选的常规成词）
        if let Some(dynamic_cand) = DateTimeEvaluator::try_evaluate(raw_input) {
            let insert_pos = if candidates.is_empty() { 0 } else { 1 };
            candidates.insert(insert_pos, dynamic_cand);
        }

        // 6. 英文单词联想注入（如输入 rust、github、apple 等在次位提供英文候选）
        let eng_cands = self.english_suggester.lookup(raw_input);
        for eng in eng_cands.into_iter().rev() {
            let pos = if candidates.is_empty() { 0 } else { 1 };
            candidates.insert(pos, eng);
        }

        // 7. 符号与表情联想注入（如输入 dianzan、daxiao 等在次位提供符号候选）
        let sym_cands = self.symbol_suggester.lookup(raw_input);
        for sym in sym_cands.into_iter().rev() {
            let pos = if candidates.is_empty() { 0 } else { 1 };
            candidates.insert(pos, sym);
        }

        // 8. 用户自定义短语注入（如输入 mymail、addr 等置顶首位）
        let phrase_cands = self.custom_phrases.lookup(raw_input);
        for phrase in phrase_cands.into_iter().rev() {
            candidates.insert(0, phrase);
        }

        // 9. 剪贴板历史快捷粘贴候选注入（敲入 v、zt、niantie 时首位提示）
        let clip_cands = self.clipboard.try_eval_candidates(raw_input);
        for clip in clip_cands.into_iter().rev() {
            candidates.insert(0, clip);
        }

        self.current_candidates = candidates;
    }

    /// 批量导入外部词库条目到用户自适应模型
    pub fn import_dict_entries(&mut self, entries: &[ImportedDictEntry]) {
        let now = 1_700_000_000;
        for entry in entries {
            self.user_model.record_commit(&entry.text, &entry.pinyin, now);
        }
    }

    /// 记录外部系统剪贴板复制事件
    pub fn record_clipboard_copy(&mut self, text: impl Into<String>) {
        self.clipboard.record_copy(text);
    }

    /// 获取当前输入缓冲区内容
    pub fn raw_input(&self) -> &str {
        self.composition.as_str()
    }

    /// 获取带智能音节切分隔音符的格式化预编辑文本（由音节语言学切分直接驱动，如 "wo'men", "shan'xi", "xian"）
    pub fn formatted_preedit(&self) -> String {
        let raw = self.composition.as_str();
        if raw.is_empty() {
            return String::new();
        }

        let paths = PrismEngine::build_paths(raw, &self.config.fuzzy, self.config.double_pinyin);
        if let Some(first_path) = paths.first() {
            return first_path.to_key_string();
        }

        raw.to_string()
    }

    /// 获取内部词典表引用
    pub fn table(&self) -> &LexiconTable {
        &self.table
    }

    /// 获取当前光标位置
    pub fn cursor(&self) -> usize {
        self.composition.cursor()
    }

    /// 获取当前呈现的候选词切片
    pub fn candidates(&self) -> &[AlgoCandidate] {
        &self.current_candidates
    }

    /// 清空并重置引擎
    pub fn reset(&mut self) {
        self.composition.clear();
        self.page_index = 0;
        self.current_candidates.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_end_to_end_xian() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "xian".chars() {
            engine.push_char(ch);
        }

        let candidates = engine.candidates();
        assert!(!candidates.is_empty());

        let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
        // 验证多路径召回：同时出现“先”与“西安”
        assert!(texts.contains(&"先"));
        assert!(texts.contains(&"西安"));
    }

    #[test]
    fn test_engine_v_mode() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "v20*5".chars() {
            engine.push_char(ch);
        }

        let candidates = engine.candidates();
        assert!(!candidates.is_empty());
        assert_eq!(candidates[0].text, "100");
    }

    #[test]
    fn test_engine_select_and_partial_commit() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "wo'men".chars() {
            engine.push_char(ch);
        }

        let (text, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选词");
        assert_eq!(text, "我们");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_engine_women_exact_first_and_preedit() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "women".chars() {
            engine.push_char(ch);
        }

        let candidates = engine.candidates();
        assert!(!candidates.is_empty());
        // 整词“我们”必须排在第 1 位
        assert_eq!(candidates[0].text, "我们");
        // 预编辑文本必须智能显示为 "wo'men"
        assert_eq!(engine.formatted_preedit(), "wo'men");
    }

    #[test]
    fn test_engine_dynamic_date_expand_and_select() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        for ch in "riqi".chars() {
            engine.push_char(ch);
        }

        let candidates = engine.candidates();
        assert!(candidates.len() >= 2);
        // 验证第 2 位为可展开的动态日期功能项
        assert_eq!(candidates[1].text, "日期");
        assert!(candidates[1].is_expandable);
        assert_eq!(candidates[1].sub_candidates.len(), 7);

        // 验证展开控制
        assert!(engine.toggle_expand(1));
        assert!(engine.is_expanded);

        // 验证二级卡片选择
        let (chosen_text, remaining) = engine.select_expanded_item(1, 2).expect("选择连字符日期");
        assert!(chosen_text.contains('-'));
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_engine_acronym_matching() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        
        // 1. 测试 bj 简拼召回 北京
        for ch in "bj".chars() {
            engine.push_char(ch);
        }
        let candidates_bj = engine.candidates();
        assert!(!candidates_bj.is_empty());
        assert_eq!(candidates_bj[0].text, "北京");

        // 2. 测试 yyds 简拼召回 永远的神
        engine.reset();
        for ch in "yyds".chars() {
            engine.push_char(ch);
        }
        let candidates_yyds = engine.candidates();
        assert!(!candidates_yyds.is_empty());
        assert_eq!(candidates_yyds[0].text, "永远的神");
    }

    #[test]
    fn test_engine_clipboard_paste_candidate() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine.record_clipboard_copy("微澜输入法新版本");

        // 敲入 v 直接提示最近剪贴板
        engine.push_char('v');
        let candidates_v = engine.candidates();
        assert!(!candidates_v.is_empty());
        assert_eq!(candidates_v[0].text, "微澜输入法新版本");
        assert!(candidates_v[0].comment.as_ref().unwrap().contains("剪贴板"));

        // 选择上屏
        let (committed, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选中剪贴板");
        assert_eq!(committed, "微澜输入法新版本");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_engine_custom_phrase_and_english_and_symbol() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        engine.custom_phrases.add_entry("mymail".to_string(), "user@waylan.org".to_string());

        // 1. 测试自定义短语置顶
        for ch in "mymail".chars() {
            engine.push_char(ch);
        }
        let cands_mail = engine.candidates();
        assert!(!cands_mail.is_empty());
        assert_eq!(cands_mail[0].text, "user@waylan.org");

        // 2. 测试英文单词联想 (rust)
        engine.reset();
        for ch in "rust".chars() {
            engine.push_char(ch);
        }
        let cands_rust = engine.candidates();
        assert!(cands_rust.iter().any(|c| c.text == "rust"));

        // 3. 测试符号联想 (dianzan)
        engine.reset();
        for ch in "dianzan".chars() {
            engine.push_char(ch);
        }
        let cands_zan = engine.candidates();
        assert!(cands_zan.iter().any(|c| c.text == "\u{1F44D}"));
    }
}
