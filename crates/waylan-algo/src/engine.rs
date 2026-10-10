use crate::clipboard::ClipboardManager;
use crate::composition::Composition;
use crate::custom_phrase::{
    AuxiliaryDict, CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager,
};
use crate::dict_importer::ImportedDictEntry;
use crate::dispatcher::ModeDispatcher;
use crate::entity::EntityProtectionManager;
use crate::evaluator::DateTimeEvaluator;
use crate::opencc::OpenCcDict;
use crate::pipeline::CandidatePipeline;
use crate::prism::PrismEngine;
use crate::syllables;
use crate::table::LexiconTable;
use crate::types::{self, AlgoCandidate, AlgoConfig, CandidateTier};
use crate::user_adaptive::UserAdaptiveModel;
use crate::viterbi::{BigramLanguageModel, ViterbiDecoder};

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
    pub table: LexiconTable,
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
    /// 简繁转换两级词库引擎（STPhrases + STCharacters）
    pub opencc_dict: std::sync::Arc<OpenCcDict>,
    /// 增量敲击编辑状态机
    composition: Composition,
    /// 全局算法配置
    pub config: AlgoConfig,
    /// 当前候选词分页索引（0-indexed）
    page_index: usize,
    /// 当前页内高亮选中的候选词索引（0-indexed）
    pub selected_index: usize,
    /// 当前呈现的清洗后候选词缓存
    current_candidates: Vec<AlgoCandidate>,
    /// 上一次选词上屏的历史上下文词条（用于跨句 Bigram 语言模型联想预测）
    pub last_commit_context: Option<String>,
    /// 二级扩展卡片是否展开
    pub is_expanded: bool,
    /// 二级扩展卡片中当前选中的子项索引（0-indexed）
    pub expanded_selected_index: usize,
    /// 用户自适应词库 WAL 持久化文件路径（若设置则每次选词自动追加写盘）
    pub user_dict_path: Option<std::path::PathBuf>,
    /// 单次连续拼音输入过程中的分段选词累积器 (词条文本, 拼音片段)，用于整串消费完毕时自动合成用户自造新词
    partial_commit_segments: Vec<(String, String)>,
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
            opencc_dict: OpenCcDict::global(),
            composition: Composition::new(),
            config,
            page_index: 0,
            selected_index: 0,
            current_candidates: Vec::new(),
            last_commit_context: None,
            is_expanded: false,
            expanded_selected_index: 0,
            user_dict_path: None,
            partial_commit_segments: Vec::new(),
        }
    }

    /// 设置用户自适应词库持久化文件路径并自动重放加载已有 WAL 记录
    pub fn set_user_dict_path(&mut self, path: impl Into<std::path::PathBuf>) {
        let p = path.into();
        if p.exists() {
            if let Ok(model) = UserAdaptiveModel::load_or_replay_wal(&p) {
                self.user_model = model;
            }
        }
        self.user_dict_path = Some(p);
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

    /// 载入外部简繁转换二进制词库（STPhrases + STCharacters）
    pub fn load_opencc_dict(&mut self, dict: OpenCcDict) {
        self.opencc_dict = std::sync::Arc::new(dict);
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
        // 若在光标非末尾处插入字符，说明破坏了原有连续分段拼音流，清空自造词累积器
        if self.composition.cursor() < self.composition.len() {
            self.partial_commit_segments.clear();
        }
        self.composition.push_char(ch);
        self.page_index = 0;
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 退格删除一个字符（修改输入时清空分段自造词累积器，防止拼装错误脏词）
    pub fn pop_char(&mut self) -> &[AlgoCandidate] {
        self.composition.pop_char();
        self.partial_commit_segments.clear();
        self.page_index = 0;
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 向后删除光标右侧的一个字符（Delete 键行为，并清空分段自造词累积器）
    pub fn delete_char_at_cursor(&mut self) -> &[AlgoCandidate] {
        self.composition.delete_char_at_cursor();
        self.partial_commit_segments.clear();
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

    /// 候选翻下一页（若已处于末页则保持当前页不产生空白页）
    pub fn next_page(&mut self) -> &[AlgoCandidate] {
        self.page_index = self.page_index.saturating_add(1);
        self.recompute_candidates();
        if self.current_candidates.is_empty() && self.page_index > 0 {
            self.page_index -= 1;
            self.recompute_candidates();
        }
        &self.current_candidates
    }

    /// 候选翻上一页
    pub fn prev_page(&mut self) -> &[AlgoCandidate] {
        self.page_index = self.page_index.saturating_sub(1);
        self.recompute_candidates();
        &self.current_candidates
    }

    /// 在当前页候选词中向后（向下/向右）循环移动高亮选中项
    pub fn next_candidate(&mut self) {
        if !self.current_candidates.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.current_candidates.len();
            self.is_expanded = false;
            self.expanded_selected_index = 0;
        }
    }

    /// 在当前页候选词中向前（向上/向左）循环移动高亮选中项
    pub fn prev_candidate(&mut self) {
        if !self.current_candidates.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.current_candidates.len().saturating_sub(1);
            } else {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            self.is_expanded = false;
            self.expanded_selected_index = 0;
        }
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

        // 记录用户选词并动态调频（若配置了持久化路径则同步追加至 WAL 日志）
        if let Some(ref path) = self.user_dict_path {
            let _ = self.user_model.append_to_wal(
                path,
                &selected.text,
                &selected.pinyin,
                current_timestamp,
            );
        } else {
            self.user_model
                .record_commit(&selected.text, &selected.pinyin, current_timestamp);
        }

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
            self.compute_consumed_len_for_candidate(&selected)
        };

        if !selected.is_expandable && selected.tier != CandidateTier::TypoCorrection && !selected.pinyin.is_empty() {
            self.partial_commit_segments
                .push((selected.text.clone(), selected.pinyin.clone()));
        } else {
            self.partial_commit_segments.clear();
        }

        self.composition.consume_prefix(consumed_len);

        // 若当前整串拼音已全部消费完毕，且通过 2 次及以上分段选词完成，自动合成为用户自造新词
        if self.composition.is_empty() {
            if self.partial_commit_segments.len() >= 2 {
                let full_text: String = self
                    .partial_commit_segments
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect();
                let full_pinyin: String = self
                    .partial_commit_segments
                    .iter()
                    .map(|(_, p)| p.as_str())
                    .collect::<Vec<_>>()
                    .join("'");
                let char_len = full_text.chars().count();
                if (2..=8).contains(&char_len) {
                    if let Some(ref path) = self.user_dict_path {
                        let _ = self.user_model.append_to_wal(
                            path,
                            &full_text,
                            &full_pinyin,
                            current_timestamp,
                        );
                    } else {
                        self.user_model
                            .record_commit(&full_text, &full_pinyin, current_timestamp);
                    }
                }
            }
            self.partial_commit_segments.clear();
        }

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

    /// 结合当前双拼/全拼模式与音节图计算选中候选词所消费的原始按键字节长度
    fn compute_consumed_len_for_candidate(&self, selected: &AlgoCandidate) -> usize {
        let raw = self.composition.as_str();
        let pinyin = selected.pinyin.as_str();
        if raw.is_empty() {
            return 0;
        }

        if raw.starts_with(pinyin) && !pinyin.is_empty() {
            let mut len = pinyin.len();
            if raw[len..].starts_with('\'') {
                len = len.saturating_add(1);
            }
            return len;
        }

        // 双拼模式：每个完整音节固定消费 2 个原始双拼按键字符
        if self.config.double_pinyin.is_some() {
            let syl_count = pinyin.split('\'').filter(|s| !s.is_empty()).count();
            if syl_count == 0 {
                return raw.len();
            }
            let target_chars = syl_count.saturating_mul(2);
            let mut seen_non_quote = 0usize;
            let mut last_end = 0usize;
            for (idx, ch) in raw.char_indices() {
                last_end = idx.saturating_add(ch.len_utf8());
                if ch != '\'' {
                    seen_non_quote += 1;
                    if seen_non_quote >= target_chars {
                        if raw[last_end..].starts_with('\'') {
                            last_end = last_end.saturating_add(1);
                        }
                        break;
                    }
                }
            }
            return last_end.min(raw.len());
        }

        // 全拼与简拼混合模式：优先按候选音节逐段对齐原始输入或最优切分路径单元
        let cand_syls: Vec<&str> = pinyin.split('\'').filter(|s| !s.is_empty()).collect();
        let paths = PrismEngine::build_paths(raw, &self.config.fuzzy, None);
        let mut best_units: Vec<String> = Vec::new();
        if let Some(best) = paths.first() {
            best_units.extend(best.syllables.iter().cloned());
            if !best.remaining.is_empty() {
                best_units.push(best.remaining.clone());
            }
        }

        let mut pos = 0usize;
        for (i, &cs) in cand_syls.iter().enumerate() {
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
            if pos >= raw.len() {
                break;
            }
            if raw[pos..].starts_with(cs) {
                pos += cs.len();
            } else if i < best_units.len() && raw[pos..].starts_with(best_units[i].as_str()) {
                pos += best_units[i].len();
            } else {
                let first_len = cs.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if raw[pos..].starts_with(&cs[..first_len]) {
                    pos += first_len;
                } else {
                    return Self::compute_consumed_len(raw, pinyin);
                }
            }
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
        }

        if pos > 0 {
            pos.min(raw.len())
        } else {
            Self::compute_consumed_len(raw, pinyin)
        }
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
        self.selected_index = 0;
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

        let eval_ts = self.user_model.latest_timestamp();

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
            let mut user_matches = self.user_model.lookup_user_words(&key, eval_ts);
            if path.is_typo {
                for m in &mut user_matches {
                    m.tier = CandidateTier::TypoCorrection;
                    m.comment = Some(format!("[纠错: {}]", key));
                }
            }
            raw_candidates.append(&mut user_matches);

            // 模糊音等价查询键联合检索（将候选词 pinyin 映射回原始输入切分键以保证选词消费长度一致）
            if self.config.fuzzy.has_active_rules() && !path.is_typo {
                for fuzzy_key in PrismEngine::expand_fuzzy_keys(path, &self.config.fuzzy) {
                    let mut fuzzy_matches = self.table.lookup(&fuzzy_key);
                    for fm in &mut fuzzy_matches {
                        fm.pinyin = key.clone();
                        fm.score = fm.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_matches);

                    let mut fuzzy_user_matches =
                        self.user_model.lookup_user_words(&fuzzy_key, eval_ts);
                    for fum in &mut fuzzy_user_matches {
                        fum.pinyin = key.clone();
                        fum.score = fum.score.saturating_sub(50);
                    }
                    raw_candidates.append(&mut fuzzy_user_matches);
                }
            }
        }

        // 2.1 Viterbi 长句动态规划解码（遵循 Best Path Only 原则）：
        // 仅针对 Prism 中代价最低的最优主路径执行整句求解，彻底杜绝次级碎切路径产生劣质假整句；
        // 同时传入末尾残存拼音 remaining 与上屏历史上下文，支持跨句二元转移预测
        let min_units = if self.last_commit_context.is_some() { 1 } else { 2 };
        if let Some(path) = paths.first() {
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
                    // 融合切分路径拓扑代价值：扣除对应的拓扑惩罚
                    sentence.score = sentence.score.saturating_sub(path.cost.saturating_mul(100));
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

                // 2.2.1 跨词边界短语前缀合成检索（解决诸如 "womendeshij" 召回“我们的时间”、“我们的世界”等）
                let mut phrase_matches = self
                    .synthesize_prefix_phrases(&best_path.syllables, &best_path.remaining);
                raw_candidates.append(&mut phrase_matches);

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

        // 2.3 若多音节切分未能命中整词，将首段已完成音节或首声母单字作为保真降级备选（如 "ni" -> "你", "r" -> "人"）
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
                    if self.config.fuzzy.has_active_rules() {
                        for alt_syl in PrismEngine::expand_fuzzy_syllables(first_syl, &self.config.fuzzy) {
                            let mut alt_matches = self.table.lookup(&alt_syl);
                            for am in &mut alt_matches {
                                am.pinyin = first_syl.clone();
                                am.score = am.score.saturating_sub(50);
                            }
                            first_matches.append(&mut alt_matches);
                        }
                    }
                    if first_matches.is_empty() {
                        if let Some(initial_entries) = self.table.get_initial_raw_entries(first_syl) {
                            for (text, score) in initial_entries {
                                first_matches.push(
                                    AlgoCandidate::new(text, first_syl.clone(), score)
                                        .with_tier(CandidateTier::PrefixPartial),
                                );
                            }
                        }
                    } else {
                        for fm in &mut first_matches {
                            fm.tier = CandidateTier::PrefixPartial;
                        }
                    }
                    raw_candidates.append(&mut first_matches);
                }
            }
        }

        // 3. 动态应用用户习惯提权与上屏历史上下文亲和度加权
        for c in &mut raw_candidates {
            self.user_model.apply_boost(c, eval_ts);
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
        let mut candidates = CandidatePipeline::process_with_opencc(
            raw_candidates,
            &self.config,
            self.page_index,
            Some(&self.opencc_dict),
        );

        if self.page_index == 0 {
            // 5. 状态机感知与容错门禁控制：
            // 若当前输入处于正常的合法前缀打字态（已成功检索出 ExactWord 级别的完整成词或前缀展开短语），
            // 绝对免受激进物理空间容错打扰；仅在无任何合法成词/前缀展开命中时（如输入 nihso, cehng），才激活次位容错。
            let has_exact_or_prefix_words = candidates
                .iter()
                .any(|c| c.tier == CandidateTier::ExactWord || c.tier == CandidateTier::ProtectedEntity);

            if !has_exact_or_prefix_words {
                let typo_cands = Self::generate_spatial_typo_candidates(raw_input, &self.table);
                for (i, mut typo_cand) in typo_cands.into_iter().take(2).enumerate() {
                    if self.config.traditional_chinese {
                        typo_cand.text = self.opencc_dict.convert_text(&typo_cand.text);
                    }
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

            // 7. 英文单词联想注入（如输入 rust、github、apple 等在次位提供英文候选，若已存在相同专名则避免重复）
            let eng_cands = self.english_suggester.lookup(raw_input);
            for eng in eng_cands.into_iter().rev() {
                if !candidates.iter().any(|c| c.text.eq_ignore_ascii_case(&eng.text)) {
                    let pos = if candidates.is_empty() { 0 } else { 1 };
                    candidates.insert(pos, eng);
                }
            }

            // 8. 符号与表情联想注入（如输入 dianzan、daxiao 等在次位提供符号候选）
            let sym_cands = self.symbol_suggester.lookup(raw_input);
            for sym in sym_cands.into_iter().rev() {
                let pos = if candidates.is_empty() { 0 } else { 1 };
                candidates.insert(pos, sym);
            }

            // 9. 用户自定义短语注入（如输入 mymail、addr 等置顶首位）
            let phrase_cands = self.custom_phrases.lookup(raw_input);
            for phrase in phrase_cands.into_iter().rev() {
                candidates.insert(0, phrase);
            }

            // 10. 剪贴板历史快捷粘贴候选注入（敲入 v、zt、niantie 时首位提示）
            let clip_cands = self.clipboard.try_eval_candidates(raw_input);
            for clip in clip_cands.into_iter().rev() {
                candidates.insert(0, clip);
            }

            // 11. 前缀高频短词混排注入 (Prefix Segment Candidate Injection)：
            // 当处于首未翻页状态 (page_index == 0) 且有效拼音达到 3 个及以上音节时，
            // 自动提取前 3 个音节（如 wo'men'de -> 我们的）与前 2 个音节（如 wo'men -> 我们，或简拼 r'g -> 如果/入股）的高频成词，
            // 紧随全长成词之后注入，支持分段确认 (Partial Commit)
            if let Some(best_path) = paths.first() {
                let total_syls = best_path.syllables.len()
                    + if best_path.remaining.is_empty() { 0 } else { 1 };
                if total_syls >= 3 && best_path.syllables.len() >= 2 {
                    let mut prefix_items = Vec::new();

                    // 11.1 若总音节 >= 4 且已完成音节 >= 3，优先提取前 3 音节全拼高频词（如 wo'men'de -> 我们的）
                    if total_syls >= 4 && best_path.syllables.len() >= 3 {
                        let key_3 = best_path.syllables[..3].join("'");
                        let matches_3 = self.table.lookup(&key_3);
                        for m in matches_3 {
                            if m.pinyin == key_3 {
                                let mut cand = m.clone();
                                if self.config.traditional_chinese {
                                    cand.text = self.opencc_dict.convert_text(&cand.text);
                                }
                                cand.tier = CandidateTier::PrefixPartial;
                                prefix_items.push(cand);
                                break;
                            }
                        }
                    }

                    // 11.2 提取前 2 音节高频词（优先全拼如 wo'men -> 我们；若为声母简拼如 r'g，则回退查简拼召回 Top-2 二字高频词如 如果、入股）
                    let key_2 = best_path.syllables[..2].join("'");
                    let matches_2 = self.table.lookup(&key_2);
                    let mut found_exact_2 = false;
                    for m in matches_2 {
                        if m.pinyin == key_2 {
                            let mut cand = m.clone();
                            if self.config.traditional_chinese {
                                cand.text = self.opencc_dict.convert_text(&cand.text);
                            }
                            cand.tier = CandidateTier::PrefixPartial;
                            prefix_items.push(cand);
                            found_exact_2 = true;
                            break;
                        }
                    }
                    if !found_exact_2 {
                        let acronym_2 = best_path.syllables[..2].concat();
                        let acro_matches_2 = self.table.lookup_acronym(&acronym_2);
                        for m in acro_matches_2.into_iter().take(2) {
                            let mut cand = m;
                            if self.config.traditional_chinese {
                                cand.text = self.opencc_dict.convert_text(&cand.text);
                            }
                            cand.pinyin = key_2.clone();
                            cand.tier = CandidateTier::PrefixPartial;
                            prefix_items.push(cand);
                        }
                    }

                    // 11.3 混排注入：动态保留前序全长成词（2 至 3 个），随后注入前缀短词并保持去重
                    if !prefix_items.is_empty() {
                        let mut injected = Vec::with_capacity(candidates.len() + prefix_items.len());
                        let mut seen = std::collections::HashSet::new();

                        // 统计排在列表头部的全长匹配成词数量（如 rgzn 命中的 人工智能、人过中年、弱冠之年）
                        let full_len_count = candidates
                            .iter()
                            .take_while(|c| {
                                c.tier != CandidateTier::PrefixPartial
                                    && c.text.chars().count() >= total_syls
                            })
                            .count();
                        let keep_count = full_len_count.clamp(2, 3).min(candidates.len());

                        for c in candidates.drain(..keep_count) {
                            if seen.insert(c.text.clone()) {
                                injected.push(c);
                            }
                        }

                        // 注入前缀短词
                        for pi in prefix_items {
                            if seen.insert(pi.text.clone()) {
                                injected.push(pi);
                            }
                        }

                        // 拼接剩余备选词
                        for c in candidates.drain(..) {
                            if seen.insert(c.text.clone()) {
                                injected.push(c);
                            }
                        }

                        candidates = injected;
                    }
                }
            }
        }

        // 统一强制截断至单页容量上限，防止扩展项注入导致单页候选溢出
        candidates.truncate(self.config.page_size.max(1));

        self.current_candidates = candidates;
    }

    /// 通用跨词边界短语前缀与简拼链合成机制 (Unified Phrase Synthesis)
    ///
    /// # 设计原理
    /// - **统一声母链模型 ($k \ge 1$)**：
    ///   动态向前回溯切分流，提取所有连续的未闭合声母序列。
    ///   - 当 $k = 1$ 时（单声母）：与倒数第 1 个音节结合，展开全拼词（如 shi + j -> 时间/世界）；
    ///   - 当 $k \ge 2$ 时（多声母简拼链）：通过简拼索引展开对应长度的成词（如 gj -> 感觉，gwy -> 国务院，rgzn -> 人工智能）；
    /// - **前驱与尾部短语合成**：
    ///   与前驱 Viterbi 最优短语无缝拼接，并引入长度激励与 Bigram 语言模型亲和度加权。
    fn synthesize_prefix_phrases(
        &self,
        syllables: &[String],
        remaining: &str,
    ) -> Vec<AlgoCandidate> {
        let n = syllables.len();
        if n == 0 || remaining.is_empty() {
            return Vec::new();
        }

        // 1. 动态向前回溯提取尾部声母链 initial_chain
        let mut initial_chain = vec![remaining.to_string()];
        let mut split_idx = n;

        while split_idx > 0 {
            let prev_syl = &syllables[split_idx - 1];
            // 若倒数音节不是合法的完整标准音节（即为单字母声母，如 g, w, r, z），吸收进声母链
            if !crate::syllables::is_valid_syllable(prev_syl) {
                initial_chain.insert(0, prev_syl.clone());
                split_idx -= 1;
            } else {
                break;
            }
        }

        let k = initial_chain.len();
        let head_syls = &syllables[..split_idx];

        // 2. 尾部展开候选词集合 (tail_cands)
        let tail_cands: Vec<AlgoCandidate> = if k == 1 {
            // k = 1 场景：单声母未完成态，与前驱最后 1 个完整音节结合展开全拼词 (如 shi + j* -> 时间/世界)
            if split_idx == 0 {
                return Vec::new();
            }
            let last_syl_idx = split_idx - 1;
            let tail_syl = &syllables[last_syl_idx..split_idx];
            self.table.lookup_prefix_extension(tail_syl, remaining)
        } else {
            // k >= 2 场景：连续声母简拼链，直接在简拼索引树上遍历 (如 gj -> 感觉, gwy -> 国务院, rgzn -> 人工智能)
            let acronym_key = initial_chain.concat();
            self.table.lookup_acronym(&acronym_key)
        };

        if tail_cands.is_empty() {
            return Vec::new();
        }

        // 3. 确定真正的前驱短语音节切片 (effective_head_syls)
        let effective_head_syls = if k == 1 {
            // k = 1 时，最后 1 个完整音节已经被 tail 消耗，前驱为 split_idx - 1
            if split_idx == 0 {
                return Vec::new();
            }
            &syllables[..split_idx - 1]
        } else {
            // k >= 2 时，所有声母都被 tail 消耗，完整音节全部留作前驱
            head_syls
        };

        if effective_head_syls.is_empty() {
            return tail_cands;
        }

        // 4. 求解前驱段文本与得分 (如 wo'men'gong'tong'de -> 我们共同的)
        let head_key = effective_head_syls.join("'");
        let mut head_options: Vec<(String, String, u32)> = Vec::new();

        if let Some(sentence) = ViterbiDecoder::decode_sentence_with_context(
            effective_head_syls,
            "",
            &self.table,
            self.last_commit_context.as_deref(),
        ) {
            head_options.push((sentence.text, head_key.clone(), sentence.score));
        }

        let exact_matches = self.table.lookup(&head_key);
        if let Some(best_exact) = exact_matches.first() {
            if !head_options.iter().any(|(t, _, _)| t == &best_exact.text) {
                head_options.push((best_exact.text.clone(), head_key.clone(), best_exact.score));
            }
        }

        // 5. 跨词短语笛卡尔积合成与打分
        let mut results = Vec::new();
        for (head_text, head_pinyin, head_score) in &head_options {
            let head_last_char = head_text.chars().last().map(|c| c.to_string()).unwrap_or_default();
            for tail in tail_cands.iter().take(5) {
                let phrase_text = format!("{}{}", head_text, tail.text);
                let phrase_pinyin = format!("{}'{}", head_pinyin, tail.pinyin);

                let char_len = phrase_text.chars().count() as u32;
                let length_boost = char_len.saturating_sub(1).saturating_mul(600_000);

                // 获取二元搭配亲和度奖励（优先全词转移，次选显式高阶语义规则，再回退末字助词泛化转移）
                let bigram_bonus = self
                    .table
                    .get_bigram_affinity(head_text, &tail.text)
                    .unwrap_or_else(|| {
                        let rule_score = BigramLanguageModel::estimate_bigram_affinity(head_text, &tail.text);
                        if rule_score > 0 {
                            rule_score
                        } else {
                            self.table.get_bigram_affinity(&head_last_char, &tail.text).unwrap_or(0)
                        }
                    });

                let final_score = head_score
                    .saturating_add(tail.score)
                    .saturating_add(length_boost)
                    .saturating_add((bigram_bonus.max(0) as u32).min(50_000));

                let cand = AlgoCandidate::new(phrase_text, phrase_pinyin, final_score)
                    .with_tier(CandidateTier::ExactWord);
                results.push(cand);
            }
        }

        results.sort_by_key(|c| std::cmp::Reverse(c.score));
        results.truncate(10);
        results
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

    /// 计算当前光标在带隔音符的 `formatted_preedit()` 字符串中的精确 UTF-8 字节偏移
    pub fn formatted_cursor_byte_offset(&self) -> usize {
        let raw = self.composition.as_str();
        let raw_cursor = self.composition.cursor().min(raw.len());
        if raw.is_empty() || raw_cursor == 0 {
            return 0;
        }

        let formatted = self.formatted_preedit();
        if raw_cursor >= raw.len() {
            return formatted.len();
        }

        let raw_prefix = &raw[..raw_cursor];
        let mut raw_iter = raw_prefix.chars();
        let mut target_opt = raw_iter.next();
        let mut offset = 0;

        for (idx, ch) in formatted.char_indices() {
            if let Some(target) = target_opt {
                if ch == target {
                    offset = idx + ch.len_utf8();
                    target_opt = raw_iter.next();
                } else if ch == '\'' {
                    continue;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if target_opt.is_none() {
            return offset.min(formatted.len());
        }

        // 双拼展开场景：基于前缀切分路径长度计算字节偏移
        let prefix_paths =
            PrismEngine::build_paths(raw_prefix, &self.config.fuzzy, self.config.double_pinyin);
        if let Some(first) = prefix_paths.first() {
            let key_len = first.to_key_string().len().min(formatted.len());
            if formatted.is_char_boundary(key_len) {
                return key_len;
            }
        }

        formatted.len()
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
        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;
        self.current_candidates.clear();
        self.last_commit_context = None;
        self.partial_commit_segments.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DoublePinyinSchema;

    #[test]
    fn test_double_pinyin_partial_commit_consumed_len() {
        let mut config = AlgoConfig::default();
        config.double_pinyin = Some(DoublePinyinSchema::XiaoHe);
        let mut engine = AlgoEngine::with_sample_data(config);

        // 小鹤双拼输入 womfvsgo -> wo'men'zhong'guo (我们中国)
        for ch in "womfvsgo".chars() {
            engine.push_char(ch);
        }
        let texts: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        let idx = texts.iter().position(|t| t == "我们").expect("候选列表应包含前缀短词 我们");
        let (committed1, rem1) = engine.select_candidate(idx, 100).unwrap();
        assert_eq!(committed1, "我们");
        assert_eq!(rem1, 4, "双拼模式下选中二字词应精确消费 4 个双拼字符，剩余 vsgo");
        assert_eq!(engine.raw_input(), "vsgo");

        // 接着选中剩余 vsgo 对应的 中国
        let texts2: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        let idx2 = texts2.iter().position(|t| t == "中国").expect("剩余双拼 vsgo 应召回 中国");
        let (committed2, rem2) = engine.select_candidate(idx2, 101).unwrap();
        assert_eq!(committed2, "中国");
        assert_eq!(rem2, 0);
    }

    #[test]
    fn test_pagination_truncate_and_last_page_guard() {
        let mut config = AlgoConfig::default();
        config.page_size = 5;
        let mut engine = AlgoEngine::with_sample_data(config);

        for ch in "riqi".chars() {
            engine.push_char(ch);
        }
        assert!(
            !engine.candidates().is_empty() && engine.candidates().len() <= 5,
            "注入日期动态扩展项后单页候选数不得超过 page_size"
        );

        // 连续向后翻页，验证处于末页时不会翻入空白页
        for _ in 0..5 {
            let page_cands = engine.next_page();
            assert!(
                !page_cands.is_empty(),
                "末页向后翻页应触发越界保护，始终保持非空候选列表"
            );
            assert!(page_cands.len() <= 5);
        }
    }

    #[test]
    fn test_partial_commit_synthesizes_user_word() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

        // 输入 bjxian -> 分段依次选中 北京 (bei'jing) 与 西安 (xi'an)
        for ch in "beijingxian".chars() {
            engine.push_char(ch);
        }
        let texts1: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        let idx1 = texts1.iter().position(|t| t == "北京").expect("应包含前缀词 北京");
        let (c1, rem1) = engine.select_candidate(idx1, 200).unwrap();
        assert_eq!(c1, "北京");
        assert_eq!(rem1, 4);

        let texts2: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        let idx2 = texts2.iter().position(|t| t == "西安").expect("剩余 xian 应包含 西安");
        let (c2, rem2) = engine.select_candidate(idx2, 201).unwrap();
        assert_eq!(c2, "西安");
        assert_eq!(rem2, 0);

        // 再次输入 beijingxian，验证分段合成的自造新词 北京西安 已沉淀到用户模型并直接召回
        let user_words = engine.user_model.lookup_user_words("bei'jing'xi'an", 202);
        assert!(
            user_words.iter().any(|w| w.text == "北京西安"),
            "分段选词消费完整个拼音序列后应自动合成用户自造新词 北京西安"
        );
    }

    #[test]
    fn test_partial_commit_segments_cleared_on_edit() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());

        for ch in "beijingxian".chars() {
            engine.push_char(ch);
        }
        let texts1: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        let idx1 = texts1.iter().position(|t| t == "北京").expect("应包含前缀词 北京");
        let (c1, rem1) = engine.select_candidate(idx1, 200).unwrap();
        assert_eq!(c1, "北京");
        assert_eq!(rem1, 4);
        assert_eq!(engine.partial_commit_segments.len(), 1);

        // 用户在剩余的拼音中按下退格键修改拼音
        engine.pop_char();
        assert!(
            engine.partial_commit_segments.is_empty(),
            "修改剩余拼音时必须立即清空自造词累积器，防止拼音错位合成脏词"
        );
    }
}

