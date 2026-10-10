pub mod bigram;
pub mod builder;
pub mod clipboard;
pub mod composition;
pub mod custom_phrase;
pub mod dat;
pub mod dict_importer;
pub mod dispatcher;
pub mod entity;
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
pub mod vocab;

use clipboard::ClipboardManager;
use composition::Composition;
pub use custom_phrase::AuxiliaryDict;
pub use bigram::CompactMmapBigram;
pub use vocab::Vocabulary;
use custom_phrase::{CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager};
use dict_importer::ImportedDictEntry;
use dispatcher::ModeDispatcher;
pub use entity::EntityProtectionManager;
use evaluator::DateTimeEvaluator;
use pipeline::CandidatePipeline;
use prism::PrismEngine;
use table::LexiconTable;
use types::{AlgoCandidate, AlgoConfig, CandidateTier};
use user_adaptive::UserAdaptiveModel;
use viterbi::{BigramLanguageModel, ViterbiDecoder};

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
    /// 专有名词与实体保护管理器
    pub entity_manager: EntityProtectionManager,
    /// 增量敲击编辑状态机
    composition: Composition,
    /// 全局算法配置
    pub config: AlgoConfig,
    /// 当前候选词分页索引（0-indexed）
    page_index: usize,
    /// 当前呈现的清洗后候选词缓存
    current_candidates: Vec<AlgoCandidate>,
    /// 上一次选词上屏的历史上下文词条（用于跨句 Bigram 语言模型联想预测）
    pub last_commit_context: Option<String>,
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
            entity_manager: EntityProtectionManager::new(),
            composition: Composition::new(),
            config,
            page_index: 0,
            current_candidates: Vec::new(),
            last_commit_context: None,
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

    /// 动态注册新的受保护专有名词与实体
    pub fn register_entity(&mut self, entity: &str) {
        self.entity_manager.register_entity(entity);
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
        let raw_commit = if selected.is_expandable && !selected.sub_candidates.is_empty() {
            selected.sub_candidates[0].clone()
        } else {
            selected.text.clone()
        };

        // 记录用户选词并动态调频
        self.user_model
            .record_commit(&selected.text, &selected.pinyin, current_timestamp);

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = &self.last_commit_context {
                if Self::should_insert_pangu_space(prev, &raw_commit) {
                    format!(" {}", raw_commit)
                } else {
                    raw_commit.clone()
                }
            } else {
                raw_commit.clone()
            }
        } else {
            raw_commit.clone()
        };

        // 记录上一次上屏词条作为跨句历史上下文（记录去除前后空格的纯净词条，避免干扰语言模型转移概率）
        let trimmed_commit = raw_commit.trim();
        if !trimmed_commit.is_empty() {
            self.last_commit_context = Some(trimmed_commit.to_string());
        }

        // 计算消费的拼音字母长度并执行分段确认（动态功能项与容错纠错项消费全部输入）
        let consumed_len = if selected.is_expandable || selected.tier == CandidateTier::TypoCorrection {
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
        let raw_text = cand.sub_candidates.get(sub_index)?.clone();

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = &self.last_commit_context {
                if Self::should_insert_pangu_space(prev, &raw_text) {
                    format!(" {}", raw_text)
                } else {
                    raw_text.clone()
                }
            } else {
                raw_text.clone()
            }
        } else {
            raw_text.clone()
        };

        let trimmed_text = raw_text.trim();
        if !trimmed_text.is_empty() {
            self.last_commit_context = Some(trimmed_text.to_string());
        }

        self.composition.clear();
        self.page_index = 0;
        self.recompute_candidates();

        Some((commit_text, 0))
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

    /// 检查两个相邻文本之间是否需要插入盘古之白（中西文智能空格）
    pub fn should_insert_pangu_space(prev_context: &str, curr_commit: &str) -> bool {
        let prev_text = prev_context.trim();
        let curr_text = curr_commit.trim();

        if prev_text.is_empty() || curr_text.is_empty() {
            return false;
        }

        let prev_ch = match prev_text.chars().last() {
            Some(c) => c,
            None => return false,
        };

        let curr_ch = match curr_text.chars().next() {
            Some(c) => c,
            None => return false,
        };

        // 若前驱结尾或当前开头已有空格，则不重复插入
        if prev_context.ends_with(' ') || curr_commit.starts_with(' ') {
            return false;
        }

        let is_cjk = |c: char| -> bool {
            matches!(c,
                '\u{4e00}'..='\u{9fff}' |
                '\u{3400}'..='\u{4dbf}' |
                '\u{20000}'..='\u{2a6df}' |
                '\u{f900}'..='\u{faff}'
            )
        };

        // 场景 1：中文汉字接英文单词或数字（如："测试" + "Rust" => "测试 Rust"；"第" + "10" => "第 10"）
        if is_cjk(prev_ch) && (curr_ch.is_ascii_alphabetic() || curr_ch.is_ascii_digit()) {
            return true;
        }

        // 场景 2：英文单词接中文汉字（如："Rust" + "代码" => "Rust 代码"）
        if prev_ch.is_ascii_alphabetic() && is_cjk(curr_ch) {
            return true;
        }

        // 场景 3：数字接中文汉字（如："100" + "苹果" => "100 苹果"）
        // 豁免情况：若当前词为中文量词或单位（如："100" + "米" => "100米"），则不插入空格
        if prev_ch.is_ascii_digit() && is_cjk(curr_ch) {
            if Self::is_chinese_classifier(curr_text) {
                return false;
            }
            return true;
        }

        false
    }

    /// 判断是否属于常见中文量词或单位（用于数字与中文紧邻时的空格豁免）
    pub fn is_chinese_classifier(word: &str) -> bool {
        const CLASSIFIERS: &[&str] = &[
            // 常见单字量词
            "个", "只", "支", "枝", "张", "条", "头", "匹", "尾", "块", "片", "粒", "颗", "本",
            "册", "部", "首", "篇", "幅", "座", "栋", "扇", "套", "件", "双", "对", "副", "打",
            "群", "把", "口", "根", "排", "行", "段", "节", "项", "类", "种", "束", "串", "包",
            "盒", "箱", "袋", "瓶", "罐", "杯", "碗", "盘", "锅", "桶", "盆", "页", "枚", "尊",
            "通", "场", "顿", "番", "次", "回", "趟", "遍", "遭", "阵", "门", "桩", "任", "届",
            // 度量衡单位
            "米", "分米", "厘米", "毫米", "微米", "纳米", "千米", "公里", "里", "尺", "寸", "丈",
            "克", "千克", "公斤", "斤", "两", "钱", "吨", "磅", "盎司",
            "升", "毫升", "立方米", "平方米", "平方", "公顷", "亩",
            // 时间与日期
            "秒", "分", "分钟", "刻", "时", "小时", "天", "日", "周", "月", "年", "岁", "代", "季", "度",
            // 货币与比率
            "元", "角", "块", "毛", "镑", "刀",
            "折", "倍", "成", "点", "级", "百分点", "万", "亿", "兆",
        ];

        CLASSIFIERS.contains(&word)
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

        // 2.0 专有名词与实体保护机制 (NER 原型)：精确命中受保护实体时注入置顶候选
        if let Some(entity) = self.entity_manager.match_exact(raw_input) {
            let mut cand = AlgoCandidate::new(entity, raw_input, 999_999);
            cand.tier = CandidateTier::ProtectedEntity;
            cand.comment = Some("专名".to_string());
            raw_candidates.push(cand);
        }

        for path in &paths {
            let key = path.to_key_string();

            // 词典精确及倒排检索
            let mut matches = self.table.lookup(&key);
            if path.is_typo {
                for m in &mut matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            }
            raw_candidates.append(&mut matches);

            // 用户自造词检索
            let mut user_matches = self.user_model.lookup_user_words(&key, 1_700_000_000);
            if path.is_typo {
                for m in &mut user_matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            }
            raw_candidates.append(&mut user_matches);
        }

        // 2.1 Viterbi 长句动态规划解码：
        // 仅针对 Prism 中代价最低的最优主路径执行整句求解，避免多路径重复计算与次级碎切噪音；
        // 同时传入末尾残存拼音 remaining 与上屏历史上下文，支持跨句二元转移预测
        let min_units = if self.last_commit_context.is_some() { 1 } else { 2 };
        for path in paths.iter().take(2) {
            let total_units = path.syllables.len()
                + if path.remaining.is_empty() {
                    0
                } else {
                    1
                };
            if total_units >= min_units {
                if let Some(mut sentence) = ViterbiDecoder::decode_sentence_with_context(
                    &path.syllables,
                    &path.remaining,
                    &self.table,
                    self.last_commit_context.as_deref(),
                ) {
                    // 融合切分路径拓扑代价值：代价值越大（如包含借音碎切），扣除对应的似然惩罚，防止劣质路径反超优质整句
                    sentence.score = sentence.score.saturating_sub((path.cost as u32).saturating_mul(100));
                    raw_candidates.push(sentence);
                }
            }
        }

        if let Some(best_path) = paths.first() {

            // 2.2 前缀残存拼音与声母补全检索（解决诸如 "shux" 召回“熟悉”、“数学”、“属性”等）
            if !best_path.remaining.is_empty() {
                let mut prefix_matches = self
                    .table
                    .lookup_prefix_extension(&best_path.syllables, &best_path.remaining);
                raw_candidates.append(&mut prefix_matches);

                // 若前序已完成音节非空，同时将首段已完成音节的候选词作为降级备选（如“数”、“书”、“树”）
                if !best_path.syllables.is_empty() {
                    let base_key = best_path.syllables.join("'");
                    let mut base_matches = self.table.lookup(&base_key);
                    for bm in &mut base_matches {
                        bm.tier = CandidateTier::PrefixPartial;
                    }
                    raw_candidates.append(&mut base_matches);
                }
            }
        }

        // 2.2 检索首字母缩写简拼（如 bj -> 北京，yyds -> 永远的神）
        let mut acronym_matches = self.table.lookup_acronym(raw_input);
        raw_candidates.append(&mut acronym_matches);

        // 2.3 若多音节切分未能命中整词，将首段已完成合法音节作为保真降级备选（如 "ni" -> "你"）
        let clean_raw_len = raw_input.chars().filter(|c| *c != '\'').count();
        let has_full_exact_match = raw_candidates.iter().any(|c| {
            c.tier == CandidateTier::ExactWord
                && c.pinyin.chars().filter(|ch| *ch != '\'').count() == clean_raw_len
        });
        if !has_full_exact_match {
            if let Some(first_path) = paths.first() {
                if !first_path.syllables.is_empty() {
                    let first_syl = &first_path.syllables[0];
                    let mut first_matches = self.table.lookup(first_syl);
                    for fm in &mut first_matches {
                        fm.tier = CandidateTier::PrefixPartial;
                    }
                    raw_candidates.append(&mut first_matches);
                }
            }
        }

        // 3. 动态应用用户习惯提权与上屏历史上下文亲和度加权
        for c in &mut raw_candidates {
            self.user_model.apply_boost(c, 1_700_000_000);
            if let Some(ref prev) = self.last_commit_context {
                let affinity = self
                    .table
                    .get_bigram_affinity(prev, &c.text)
                    .unwrap_or_else(|| BigramLanguageModel::estimate_bigram_affinity(prev, &c.text));
                if affinity > 0 {
                    // 将语言模型亲和度映射为候选词得分增益（大幅提高首位命中率）
                    c.score = c.score.saturating_add((affinity as u32).min(50_000));
                }
            }
        }

        // 4. 送入后处理流水线（去重、简繁、字符集、注释、分页）
        let mut candidates =
            CandidatePipeline::process(raw_candidates, &self.config, self.page_index);

        // 5. 次位容错备选注入：
        // 保持切分音节 100% 原始字母绝对保真；
        // 仅在主词库未命中完整成词时，在候选词第 2~3 位提供带有 [纠错] 标识的邻键候选词，避免盲打全盘皆空
        if !has_full_exact_match {
            let typo_cands = Self::generate_spatial_typo_candidates(raw_input, &self.table);
            for (i, typo_cand) in typo_cands.into_iter().take(2).enumerate() {
                // 若列表中已有同名候选但非容错词（例如 Viterbi 产生的不完全前缀预测），先将其剔除
                candidates.retain(|c| c.text != typo_cand.text);
                let target_pos = if candidates.is_empty() {
                    0
                } else {
                    (1 + i).min(candidates.len())
                };
                candidates.insert(target_pos, typo_cand);
            }
        }

        // 6. 动态扩展功能（日期、时间、干支等）注入：
        // 若命中动态日期/时间指令，将动态功能项插入到第 2 位（仅次于首选的常规成词）
        if let Some(dynamic_cand) = DateTimeEvaluator::try_evaluate(raw_input) {
            let insert_pos = if candidates.is_empty() { 0 } else { 1 };
            candidates.insert(insert_pos, dynamic_cand);
        }

        // 6. 英文单词联想注入（如输入 rust、github、apple 等在次位提供英文候选，若已存在相同专名则避免重复）
        let eng_cands = self.english_suggester.lookup(raw_input);
        for eng in eng_cands.into_iter().rev() {
            if !candidates.iter().any(|c| c.text.eq_ignore_ascii_case(&eng.text)) {
                let pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(pos, eng);
            }
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

    /// 生成基于 QWERTY 物理键盘邻键误触与相邻倒置的次位容错候选
    fn generate_spatial_typo_candidates(
        raw_input: &str,
        table: &LexiconTable,
    ) -> Vec<AlgoCandidate> {
        if raw_input.len() < 3 || raw_input.len() > 20 {
            return Vec::new();
        }

        let mut candidates: Vec<(AlgoCandidate, u32)> = Vec::new();
        let chars: Vec<char> = raw_input.chars().collect();
        let n = chars.len();

        // 1. 单字符邻键空间替换 (Single-char spatial substitution)
        for i in 0..n {
            let orig_c = chars[i];
            if !orig_c.is_ascii_lowercase() {
                continue;
            }

            for alt_b in b'a'..=b'z' {
                let alt_c = alt_b as char;
                if alt_c == orig_c {
                    continue;
                }
                let dist = syllables::key_spatial_distance(orig_c, alt_c);
                if dist > 1.25 {
                    continue; // 仅探索几何紧邻按键
                }

                let mut mutated_chars = chars.clone();
                mutated_chars[i] = alt_c;
                let mutated: String = mutated_chars.into_iter().collect();

                let paths =
                    PrismEngine::build_paths(&mutated, &types::FuzzyConfig::default(), None);
                if let Some(best) = paths.first() {
                    if best.remaining.is_empty() && !best.syllables.is_empty() {
                        let key = best.to_key_string();
                        if let Some(entries) = table.get_raw_entries(&key) {
                            let cost = syllables::char_substitution_cost(orig_c, alt_c);
                            for (text, raw_score) in entries.iter().take(2) {
                                let mut cand = AlgoCandidate::new(text, &key, *raw_score);
                                cand.tier = CandidateTier::TypoCorrection;
                                cand.comment = Some(format!("[纠错: {}]", key));
                                candidates.push((cand, cost));
                            }
                        }
                    }
                }
            }
        }

        // 2. 邻近字母倒置 (Adjacent transpose, 如 cehng -> cheng, neg -> eng)
        for i in 0..n.saturating_sub(1) {
            let mut swapped_chars = chars.clone();
            swapped_chars.swap(i, i + 1);
            let swapped: String = swapped_chars.into_iter().collect();

            let paths =
                PrismEngine::build_paths(&swapped, &types::FuzzyConfig::default(), None);
            if let Some(best) = paths.first() {
                if best.remaining.is_empty() && !best.syllables.is_empty() {
                    let key = best.to_key_string();
                    if let Some(entries) = table.get_raw_entries(&key) {
                        let cost = 16;
                        for (text, raw_score) in entries.iter().take(2) {
                            let mut cand = AlgoCandidate::new(text, &key, *raw_score);
                            cand.tier = CandidateTier::TypoCorrection;
                            cand.comment = Some(format!("[纠错: {}]", key));
                            candidates.push((cand, cost));
                        }
                    }
                }
            }
        }

        // 3. 排序与去重：优先代价低，次优先词频高
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.score.cmp(&a.0.score)));

        let mut deduped = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (cand, _) in candidates {
            if seen.insert(cand.text.clone()) {
                deduped.push(cand);
                if deduped.len() >= 3 {
                    break;
                }
            }
        }

        deduped
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

    /// 清空上屏历史上下文缓存
    pub fn clear_commit_context(&mut self) {
        self.last_commit_context = None;
    }

    /// 清空并重置引擎
    pub fn reset(&mut self) {
        self.composition.clear();
        self.page_index = 0;
        self.current_candidates.clear();
        self.last_commit_context = None;
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

    #[test]
    fn test_engine_entity_protection() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

        // 1. 测试内置实体置顶推荐 (wayland)
        for ch in "wayland".chars() {
            engine.push_char(ch);
        }
        let cands = engine.candidates();
        assert!(!cands.is_empty(), "候选列表不应为空");
        assert_eq!(cands[0].text, "wayland");
        assert_eq!(cands[0].comment.as_deref(), Some("专名"));

        // 2. 测试动态注册实体 (deepmind)
        engine.reset();
        engine.register_entity("DeepMind");
        for ch in "deepmind".chars() {
            engine.push_char(ch);
        }
        let dm_cands = engine.candidates();
        assert!(!dm_cands.is_empty());
        assert_eq!(dm_cands[0].text, "deepmind");
        assert_eq!(dm_cands[0].comment.as_deref(), Some("专名"));
    }

    #[test]
    fn test_engine_commit_context_association() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        // 插入具有同音竞争的词条："shi'jian" => "事件"（静态分 14,000）与 "时间"（静态分 10,000）
        engine.table.insert("shi'jian", "事件", 14_000);
        engine.table.insert("shi'jian", "时间", 10_000);

        // 1. 在无上屏历史上下文时，输入 shijian 优先命中静态分更高的“事件”
        for ch in "shijian".chars() {
            engine.push_char(ch);
        }
        assert_eq!(engine.candidates()[0].text, "事件");
        engine.reset();
        assert!(engine.last_commit_context.is_none());

        // 2. 用户先输入 beijing 并选择“北京”上屏
        for ch in "beijing".chars() {
            engine.push_char(ch);
        }
        let (committed, remaining) = engine.select_candidate(0, 1_700_000_000).expect("选中北京");
        assert_eq!(committed, "北京");
        assert_eq!(remaining, 0);
        assert_eq!(engine.last_commit_context.as_deref(), Some("北京"));

        // 3. 用户紧接着输入 shijian：跨句历史上下文生效，由于“北京时间”强亲和度，“时间”逆袭首选！
        for ch in "shijian".chars() {
            engine.push_char(ch);
        }
        let candidates = engine.candidates();
        assert!(!candidates.is_empty());
        assert_eq!(
            candidates[0].text, "时间",
            "在跨句上下文‘北京’的驱动下，‘时间’应成功逆袭置顶首位"
        );

        // 4. 用户按 Esc/reset，上下文重置清空
        engine.reset();
        assert!(engine.last_commit_context.is_none());
    }

    #[test]
    fn test_engine_spatial_typo_tolerance_nihso() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        // 模拟用户键入 nihso（其中 s 为 a 的 QWERTY 键盘紧邻误触按键，原本意图为 nihao 你好）
        for ch in "nihso".chars() {
            engine.push_char(ch);
        }

        // 验证 1：原始击键字母绝对保真（绝不篡改底层输入缓冲区）
        assert_eq!(engine.raw_input(), "nihso");

        // 验证 2：预编辑格式化文本绝不被改写为 nihao
        assert_ne!(engine.formatted_preedit(), "ni'hao");

        let candidates = engine.candidates();
        assert!(candidates.len() >= 2, "应至少包含保真降级候选与次位容错备选");

        // 验证 3：第 2 位（次位）提供带有 [纠错] 标识的邻键候选词“你好”
        let typo_cand = &candidates[1];
        assert_eq!(typo_cand.text, "你好");
        assert_eq!(typo_cand.tier, CandidateTier::TypoCorrection);
        assert!(
            typo_cand.comment.as_ref().unwrap().contains("[纠错: ni'hao]"),
            "注释应包含纠错提示"
        );

        // 验证 4：选中次位容错候选上屏，完全消费掉全部 5 个误触字符并清空输入
        let (committed, remaining) = engine.select_candidate(1, 1_700_000_000).expect("选容错词");
        assert_eq!(committed, "你好");
        assert_eq!(remaining, 0, "选中纠错候选应完全清空输入缓冲区");
        assert_eq!(engine.raw_input(), "");
    }

    #[test]
    fn test_engine_typo_adjacent_transpose_cehng() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        // 词库准备目标词条："cheng" => "成"
        engine.table.insert("cheng", "成", 10_000);

        // 模拟用户键入 cehng（eh 为相邻字母颠倒误敲，原本意图为 cheng）
        for ch in "cehng".chars() {
            engine.push_char(ch);
        }

        let candidates = engine.candidates();
        assert!(!candidates.is_empty(), "候选列表不应为空");
        // 验证包含 cheng 纠错的候选
        assert!(candidates.iter().any(|c| {
            c.tier == CandidateTier::TypoCorrection
                && c.comment.as_ref().map_or(false, |comm| comm.contains("[纠错: cheng]"))
        }));
    }

    #[test]
    fn test_pangu_spacing_helper_rules() {
        // 1. 中文汉字接英文单词或数字
        assert!(AlgoEngine::should_insert_pangu_space("测试", "Rust"));
        assert!(AlgoEngine::should_insert_pangu_space("第", "10"));

        // 2. 英文单词接中文汉字
        assert!(AlgoEngine::should_insert_pangu_space("Rust", "语言"));
        assert!(AlgoEngine::should_insert_pangu_space("Waylan", "输入法"));

        // 3. 数字接量词豁免 vs 数字接普通汉字
        assert!(!AlgoEngine::should_insert_pangu_space("100", "米"));
        assert!(!AlgoEngine::should_insert_pangu_space("20", "个"));
        assert!(!AlgoEngine::should_insert_pangu_space("5", "折"));
        assert!(!AlgoEngine::should_insert_pangu_space("2026", "年"));
        assert!(AlgoEngine::should_insert_pangu_space("100", "苹果"));
        assert!(AlgoEngine::should_insert_pangu_space("2026", "你好"));

        // 4. 同类别不插入空格
        assert!(!AlgoEngine::should_insert_pangu_space("北京", "时间"));
        assert!(!AlgoEngine::should_insert_pangu_space("hello", "world"));

        // 5. 标点符号与已有空格不重复插入
        assert!(!AlgoEngine::should_insert_pangu_space("你好", "，"));
        assert!(!AlgoEngine::should_insert_pangu_space("测试 ", "Rust"));
        assert!(!AlgoEngine::should_insert_pangu_space("测试", " Rust"));
    }

    #[test]
    fn test_engine_auto_pangu_spacing_integration() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        // 注入目标词条与英文短语（带音节分词符）
        engine.table.insert("ping'guo", "苹果", 10_000);
        engine.table.insert("ge", "个", 10_000);
        engine.custom_phrases.add_entry("rust".to_string(), "Rust".to_string());
        engine.custom_phrases.add_entry("100".to_string(), "100".to_string());

        // 1. 默认情况下，auto_pangu_spacing 为 false，不自动追加空格
        assert!(!engine.config.auto_pangu_spacing);
        for ch in "women".chars() {
            engine.push_char(ch);
        }
        let (comm1, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
        assert_eq!(comm1, "我们");

        for ch in "rust".chars() {
            engine.push_char(ch);
        }
        let (comm2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中Rust");
        assert_eq!(comm2, "Rust", "未开启智能空格时不应包含前置空格");

        // 2. 开启智能空格配置
        engine.reset();
        engine.config.auto_pangu_spacing = true;

        // 步骤 A：中文接英文（我们 -> Rust）
        for ch in "women".chars() {
            engine.push_char(ch);
        }
        let (c1, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
        assert_eq!(c1, "我们");

        for ch in "rust".chars() {
            engine.push_char(ch);
        }
        let (c2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中Rust");
        assert_eq!(c2, " Rust", "中文接英文应自动补齐半角空格");
        assert_eq!(engine.last_commit_context.as_deref(), Some("Rust"), "历史上下文应保存去除空格后的纯净词");

        // 步骤 B：英文接中文（Rust -> 我们）
        for ch in "women".chars() {
            engine.push_char(ch);
        }
        let (c3, _) = engine.select_candidate(0, 1_700_000_000).expect("选中我们");
        assert_eq!(c3, " 我们", "英文接中文应自动补齐半角空格");

        // 步骤 C：数字接量词白名单豁免（100 -> 个）
        engine.reset();
        for ch in "100".chars() {
            engine.push_char(ch);
        }
        let (num_c, _) = engine.select_candidate(0, 1_700_000_000).expect("选中100");
        assert_eq!(num_c, "100");

        for ch in "ge".chars() {
            engine.push_char(ch);
        }
        let (clf_c, _) = engine.select_candidate(0, 1_700_000_000).expect("选中量词个");
        assert_eq!(clf_c, "个", "数字接中文量词白名单应豁免，不加空格");

        // 步骤 D：数字接普通中文词（100 -> 苹果）
        engine.reset();
        for ch in "100".chars() {
            engine.push_char(ch);
        }
        let (num_c2, _) = engine.select_candidate(0, 1_700_000_000).expect("选中100");
        assert_eq!(num_c2, "100");

        for ch in "pingguo".chars() {
            engine.push_char(ch);
        }
        let (noun_c, _) = engine.select_candidate(0, 1_700_000_000).expect("选中苹果");
        assert_eq!(noun_c, " 苹果", "数字接普通中文词应自动补齐半角空格");
    }
}
