use crate::config::{AppConfig, PageKeys, SwitchKey, ThemeMode};
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use waylan_algo::types::DoublePinyinSchema;

/// 双拼方案界面选择辅助枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinyinSchemaChoice {
    /// 标准全拼
    QuanPin,
    /// 小鹤双拼
    XiaoHe,
    /// 自然码双拼
    ZiRanMa,
}

impl From<Option<DoublePinyinSchema>> for PinyinSchemaChoice {
    fn from(schema: Option<DoublePinyinSchema>) -> Self {
        match schema {
            None => PinyinSchemaChoice::QuanPin,
            Some(DoublePinyinSchema::XiaoHe) => PinyinSchemaChoice::XiaoHe,
            Some(DoublePinyinSchema::ZiRanMa) => PinyinSchemaChoice::ZiRanMa,
        }
    }
}

impl From<PinyinSchemaChoice> for Option<DoublePinyinSchema> {
    fn from(choice: PinyinSchemaChoice) -> Self {
        match choice {
            PinyinSchemaChoice::QuanPin => None,
            PinyinSchemaChoice::XiaoHe => Some(DoublePinyinSchema::XiaoHe),
            PinyinSchemaChoice::ZiRanMa => Some(DoublePinyinSchema::ZiRanMa),
        }
    }
}

/// 偏好设置中心导航 Tab 标签页枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// 常规设置（输入方案、候选外观、快捷键、标点特性）
    General,
    /// 汉字与拼音（基础单字库、核心词库、扩展词库统计与导入）
    Pinyin,
    /// 中英混输（英文单词库、混输特性、外部英文导入）
    English,
    /// 符号与表情（Emoji 符号、特殊符号与颜文字预览与导入）
    Symbols,
    /// 自定义短语（用户快捷短语增删改查、自造词统计与清空）
    CustomPhrases,
}

/// 微澜图形化偏好设置应用状态
pub struct SettingsApp {
    /// 当前选中的导航 Tab
    pub current_tab: SettingsTab,
    /// 内存中编辑的配置副本
    pub config: AppConfig,
    /// 当前选择的拼音方案
    pub schema_choice: PinyinSchemaChoice,
    /// 全局操作反馈提示信息
    pub status_message: Option<String>,
    /// 汉语拼音词库文件导入路径
    pub import_file_path: String,
    /// 汉语拼音词库导入反馈提示
    pub import_status_message: Option<String>,

    /// 辅助与混输词库（包含英文词库与符号表情库）
    pub aux_dict: waylan_algo::AuxiliaryDict,
    /// 外部英文词库导入路径
    pub english_import_path: String,
    /// 英文词库导入反馈提示
    pub english_status_message: Option<String>,
    /// 外部符号词库导入路径
    pub symbols_import_path: String,
    /// 符号词库导入反馈提示
    pub symbols_status_message: Option<String>,

    /// 当前已配置的自定义快捷短语列表 (快捷码, 展开内容)
    pub custom_phrases: Vec<(String, String)>,
    /// 新增短语的快捷代码输入
    pub new_phrase_code: String,
    /// 新增短语的展开文本输入
    pub new_phrase_text: String,
    /// 短语操作状态提示
    pub phrase_status_message: Option<String>,
    /// 用户自适应自造词库当前词条总数
    pub user_dict_count: usize,
    /// 用户自造词库操作状态提示
    pub user_dict_status_message: Option<String>,
}

impl SettingsApp {
    pub fn new() -> Self {
        let config = AppConfig::load_or_create();
        let schema_choice = PinyinSchemaChoice::from(config.double_pinyin);

        // 加载现有自定义短语
        let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
        let phrase_manager = waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
        let custom_phrases = phrase_manager.all_entries();

        // 加载用户自造词统计
        let user_dict_path = AppConfig::user_dict_path();
        let user_dict_count = waylan_algo::user_adaptive::UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
            .map(|m| m.total_entries())
            .unwrap_or(0);

        // 加载辅助与混输词库
        let mut aux_dict = waylan_algo::AuxiliaryDict::with_default_data();
        for path in AppConfig::aux_dict_search_paths() {
            if path.exists() {
                if let Ok(loaded) = waylan_algo::AuxiliaryDict::load_from_file(&path) {
                    aux_dict = loaded;
                    break;
                }
            }
        }

        Self {
            current_tab: SettingsTab::General,
            config,
            schema_choice,
            status_message: None,
            import_file_path: String::new(),
            import_status_message: None,
            aux_dict,
            english_import_path: String::new(),
            english_status_message: None,
            symbols_import_path: String::new(),
            symbols_status_message: None,
            custom_phrases,
            new_phrase_code: String::new(),
            new_phrase_text: String::new(),
            phrase_status_message: None,
            user_dict_count,
            user_dict_status_message: None,
        }
    }

    /// 将当前的自定义短语同步保存写回 phrases.txt
    pub fn sync_custom_phrases(&self) -> Result<(), String> {
        let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
        let mut manager = waylan_algo::custom_phrase::CustomPhraseManager::new();
        for (code, text) in &self.custom_phrases {
            manager.add_entry(code.clone(), text.clone());
        }
        manager
            .save_to_file(&phrase_path)
            .map_err(|e| format!("写入短语配置失败: {}", e))
    }

    /// 配置系统与备选中文字体
    pub fn configure_chinese_fonts(ctx: &egui::Context) {
        let font_candidates = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/mnt/c/Windows/Fonts/msyh.ttc",
            "/mnt/c/Windows/Fonts/simhei.ttf",
        ];

        for path_str in &font_candidates {
            let path = Path::new(path_str);
            if path.exists() {
                if let Ok(font_data) = fs::read(path) {
                    let mut fonts = FontDefinitions::default();
                    fonts.font_data.insert(
                        "cjk_fallback".to_owned(),
                        Arc::new(
                            FontData::from_owned(font_data).tweak(
                                egui::FontTweak {
                                    scale: 1.0,
                                    ..Default::default()
                                },
                            ),
                        ),
                    );

                    fonts
                        .families
                        .entry(FontFamily::Proportional)
                        .or_default()
                        .insert(0, "cjk_fallback".to_owned());

                    fonts
                        .families
                        .entry(FontFamily::Monospace)
                        .or_default()
                        .push("cjk_fallback".to_owned());

                    ctx.set_fonts(fonts);
                    break;
                }
            }
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            // 头部标题区域（翡翠青绿主题强调）
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                ui.heading(
                    RichText::new("微澜输入法 · 偏好设置")
                        .size(20.0)
                        .color(Color32::from_rgb(5, 150, 105))
                        .strong(),
                );
                ui.label(
                    RichText::new("专为 Wayland 打造的现代轻量纯 Rust 中文输入法")
                        .size(12.0)
                        .color(Color32::from_rgb(120, 120, 120)),
                );
                ui.add_space(4.0);
            });

            ui.separator();

            // 顶层导航 Tab 切换条
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.current_tab,
                    SettingsTab::General,
                    RichText::new("常规设置").strong(),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    SettingsTab::Pinyin,
                    RichText::new("汉字与拼音").strong(),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    SettingsTab::English,
                    RichText::new("中英混输").strong(),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    SettingsTab::Symbols,
                    RichText::new("符号与表情").strong(),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    SettingsTab::CustomPhrases,
                    RichText::new("自定义短语").strong(),
                );
            });

            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                match self.current_tab {
                    SettingsTab::General => {
                        // 1. 拼音方案配置
                        ui.group(|ui| {
                            ui.label(RichText::new("拼音输入方案").strong().size(14.0));
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut self.schema_choice, PinyinSchemaChoice::QuanPin, "标准全拼");
                                ui.radio_value(&mut self.schema_choice, PinyinSchemaChoice::XiaoHe, "小鹤双拼");
                                ui.radio_value(&mut self.schema_choice, PinyinSchemaChoice::ZiRanMa, "自然码双拼");
                            });
                        });

                        // 2. 候选外观与排版
                        ui.group(|ui| {
                            ui.label(RichText::new("候选框与排版外观").strong().size(14.0));

                            ui.horizontal(|ui| {
                                ui.label("单页候选词数:");
                                ui.add(egui::Slider::new(&mut self.config.page_size, 5..=9).text("项"));
                            });

                            ui.horizontal(|ui| {
                                ui.label("候选文字字号:");
                                ui.add(egui::Slider::new(&mut self.config.font_size, 12.0..=24.0).text("像素 (px)"));
                            });

                            ui.horizontal(|ui| {
                                ui.label("界面主题模式:");
                                let is_light = self.config.theme == ThemeMode::Light;
                                if ui.selectable_label(is_light, "浅色明亮").clicked() {
                                    self.config.theme = ThemeMode::Light;
                                }
                                if ui.selectable_label(!is_light, "深色护眼").clicked() {
                                    self.config.theme = ThemeMode::Dark;
                                }
                            });
                        });

                        // 3. 快捷键与输入习惯
                        ui.group(|ui| {
                            ui.label(RichText::new("按键与切换习惯").strong().size(14.0));

                            ui.horizontal(|ui| {
                                ui.label("中英文切换键:");
                                let is_shift = self.config.switch_key == SwitchKey::Shift;
                                if ui.selectable_label(is_shift, "Shift 键").clicked() {
                                    self.config.switch_key = SwitchKey::Shift;
                                }
                                if ui.selectable_label(!is_shift, "Ctrl 键").clicked() {
                                    self.config.switch_key = SwitchKey::Ctrl;
                                }
                            });

                            ui.horizontal(|ui| {
                                ui.label("候选翻页快捷键:");
                                let is_minus = self.config.page_keys == PageKeys::MinusEqual;
                                let is_bracket = self.config.page_keys == PageKeys::Bracket;
                                let is_comma = self.config.page_keys == PageKeys::CommaPeriod;

                                if ui.selectable_label(is_minus, "- / =").clicked() {
                                    self.config.page_keys = PageKeys::MinusEqual;
                                }
                                if ui.selectable_label(is_bracket, "[ / ]").clicked() {
                                    self.config.page_keys = PageKeys::Bracket;
                                }
                                if ui.selectable_label(is_comma, ", / .").clicked() {
                                    self.config.page_keys = PageKeys::CommaPeriod;
                                }
                            });
                        });

                        // 4. 智能标点与高级算法特性
                        ui.group(|ui| {
                            ui.label(RichText::new("标点与算法特性").strong().size(14.0));

                            ui.checkbox(&mut self.config.slash_to_dunhao, "斜杠替换为中文顿号 (/ 转为 、)");
                            ui.checkbox(&mut self.config.protect_digits_punctuation, "数字间标点保护 (如 3.14 与 12:30 不转中文)");
                            ui.checkbox(&mut self.config.auto_complete_pairs, "成对符号智能闭合与光标回退 (如 《》、“” )");
                            ui.checkbox(&mut self.config.fuzzy_pinyin, "开启模糊拼音 (平翘舌、前后鼻音)");
                            ui.checkbox(&mut self.config.v_mode, "开启 V 模式 (输入 v1+2 快捷计算求值)");
                            ui.checkbox(&mut self.config.traditional_chinese, "繁体中文模式 (默认输出繁体汉字)");
                            ui.checkbox(&mut self.config.auto_pangu_spacing, "中西文智能空格 (盘古之白，汉字与英文/数字自动空格)");
                        });
                    }

                    SettingsTab::Pinyin => {
                        ui.group(|ui| {
                            ui.label(RichText::new("全量汉语词库状态").strong().size(14.0));
                            ui.label("系统预编译只读词库: base.dict.bin (约 45.7 万组词条已就绪)");
                            ui.label(
                                RichText::new("涵盖 8105 通用规范单字库、核心现代汉语高频词库与计算机领域扩展词库。")
                                    .size(11.5)
                                    .color(Color32::from_rgb(120, 120, 120)),
                            );
                        });

                        ui.group(|ui| {
                            ui.label(RichText::new("外部汉语词库导入 (搜狗 .scel / Rime YAML)").strong().size(14.0));
                            ui.horizontal(|ui| {
                                ui.label("词库文件路径:");
                                ui.text_edit_singleline(&mut self.import_file_path);
                            });

                            ui.horizontal(|ui| {
                                if ui.button("开始导入并增量合入").clicked() {
                                    let path = Path::new(&self.import_file_path);
                                    if !path.exists() {
                                        self.import_status_message = Some("指定的词库文件不存在，请检查路径。".to_string());
                                    } else {
                                        match waylan_algo::dict_importer::DictImporter::import_file(path) {
                                            Ok(entries) => {
                                                let count = entries.len();
                                                let user_dict_path = AppConfig::user_dict_path();
                                                let mut user_model = waylan_algo::user_adaptive::UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
                                                    .unwrap_or_default();
                                                let now = 1_700_000_000;
                                                for entry in &entries {
                                                    user_model.record_commit(&entry.text, &entry.pinyin, now);
                                                }
                                                let _ = user_model.compact_wal(&user_dict_path);
                                                self.user_dict_count = user_model.total_entries();
                                                self.import_status_message = Some(format!("成功导入 {} 条词目，并已即时合入专属用户词库！", count));
                                            }
                                            Err(e) => {
                                                self.import_status_message = Some(format!("导入词库失败: {}", e));
                                            }
                                        }
                                    }
                                }
                            });

                            if let Some(msg) = &self.import_status_message {
                                ui.label(
                                    RichText::new(msg)
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.0),
                                );
                            }
                        });
                    }

                    SettingsTab::English => {
                        ui.group(|ui| {
                            ui.label(RichText::new("中英混输特性 (English Suggestion)").strong().size(14.0));
                            ui.label("在中文输入模式下，直接输入常用英文单词时，候选栏次位或首位自动呈现英文原词，按空格即可直接上屏，无需按 Shift 切换。");
                            ui.label(format!("当前已就绪英文单词数: {} 词", self.aux_dict.english_words.len()));
                        });

                        ui.group(|ui| {
                            ui.label(RichText::new("导入外部英文词库 (.txt / .dict.yaml)").strong().size(14.0));
                            ui.horizontal(|ui| {
                                ui.label("英文词库路径:");
                                ui.text_edit_singleline(&mut self.english_import_path);
                            });

                            ui.horizontal(|ui| {
                                if ui.button("导入英文单词表").clicked() {
                                    let path = Path::new(&self.english_import_path);
                                    if !path.exists() {
                                        self.english_status_message = Some("指定的英文词典文件不存在，请检查路径。".to_string());
                                    } else {
                                        // 读取文本每行一个词
                                        if let Ok(content) = fs::read_to_string(path) {
                                            let mut added = 0;
                                            for line in content.lines() {
                                                let word = line.split('\t').next().unwrap_or("").trim().to_lowercase();
                                                if !word.is_empty() && !self.aux_dict.english_words.iter().any(|(w, _)| w == &word) {
                                                    self.aux_dict.english_words.push((word, 50_000));
                                                    added += 1;
                                                }
                                            }
                                            let aux_path = PathBuf::from("data/aux.dict.bin");
                                            let _ = self.aux_dict.save_to_file(&aux_path);
                                            self.english_status_message = Some(format!("成功导入 {} 个新英文词汇并已更新！", added));
                                        } else {
                                            self.english_status_message = Some("读取英文词库文件失败。".to_string());
                                        }
                                    }
                                }
                            });

                            if let Some(msg) = &self.english_status_message {
                                ui.label(
                                    RichText::new(msg)
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.0),
                                );
                            }
                        });
                    }

                    SettingsTab::Symbols => {
                        ui.group(|ui| {
                            ui.label(RichText::new("符号与表情语义联想 (Symbol Suggestion)").strong().size(14.0));
                            ui.label("输入常见语义拼音（如 dianzan, daxiao, shengri, jia, jifen）在候选栏次位呈现对应符号。");
                            ui.label(
                                RichText::new("源码与通信严格遵守无 Emoji 规范，所有符号均采用标准 Unicode 标量值分发，由系统字体光栅化渲染。")
                                    .size(11.5)
                                    .color(Color32::from_rgb(120, 120, 120)),
                            );
                            ui.label(format!("当前收录符号与表情映射: {} 项", self.aux_dict.symbols.len()));
                        });

                        ui.group(|ui| {
                            ui.label(RichText::new("已收录符号映射一览 (示例)").strong().size(13.5));
                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .id_salt("symbols_preview_list")
                                .show(ui, |ui| {
                                    for (code, sym, desc) in &self.aux_dict.symbols {
                                        ui.horizontal(|ui| {
                                            ui.monospace(RichText::new(code).color(Color32::from_rgb(5, 150, 105)));
                                            ui.label("➔");
                                            ui.monospace(sym);
                                            ui.label(format!("({})", desc));
                                        });
                                    }
                                });
                        });

                        ui.group(|ui| {
                            ui.label(RichText::new("导入外部符号/表情词库 (.txt / .yaml)").strong().size(14.0));
                            ui.horizontal(|ui| {
                                ui.label("符号词库路径:");
                                ui.text_edit_singleline(&mut self.symbols_import_path);
                            });

                            ui.horizontal(|ui| {
                                if ui.button("导入符号映射表").clicked() {
                                    let path = Path::new(&self.symbols_import_path);
                                    if !path.exists() {
                                        self.symbols_status_message = Some("指定的符号文件不存在，请检查路径。".to_string());
                                    } else {
                                        if let Ok(content) = fs::read_to_string(path) {
                                            let mut added = 0;
                                            for line in content.lines() {
                                                let trimmed = line.trim();
                                                if trimmed.is_empty() || trimmed.starts_with('#') {
                                                    continue;
                                                }
                                                let parts: Vec<&str> = trimmed.split('\t').collect();
                                                if parts.len() >= 2 {
                                                    let key = parts[0].trim().to_string();
                                                    let sym = parts[1].trim().to_string();
                                                    let desc = if parts.len() >= 3 {
                                                        parts[2].trim().to_string()
                                                    } else {
                                                        "外部符号".to_string()
                                                    };
                                                    self.aux_dict.symbols.push((key, sym, desc));
                                                    added += 1;
                                                }
                                            }
                                            let aux_path = PathBuf::from("data/aux.dict.bin");
                                            let _ = self.aux_dict.save_to_file(&aux_path);
                                            self.symbols_status_message = Some(format!("成功导入 {} 条符号映射并已更新！", added));
                                        } else {
                                            self.symbols_status_message = Some("读取符号文件失败。".to_string());
                                        }
                                    }
                                }
                            });

                            if let Some(msg) = &self.symbols_status_message {
                                ui.label(
                                    RichText::new(msg)
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.0),
                                );
                            }
                        });
                    }

                    SettingsTab::CustomPhrases => {
                        // 1. 用户自定义短语管理 (phrases.txt)
                        ui.group(|ui| {
                            ui.label(RichText::new("自定义短语管理 (快捷展开)").strong().size(14.0));
                            ui.label(
                                RichText::new("输入简写代码直接置顶展开预设长文本（如敲入 mymail 展开邮箱地址）")
                                    .size(11.0)
                                    .color(Color32::from_rgb(120, 120, 120)),
                            );

                            // 新增短语输入栏
                            ui.horizontal(|ui| {
                                ui.label("简写代码:");
                                ui.add(egui::TextEdit::singleline(&mut self.new_phrase_code).desired_width(70.0));
                                ui.label("展开内容:");
                                ui.add(egui::TextEdit::singleline(&mut self.new_phrase_text).desired_width(160.0));

                                if ui.button(RichText::new("添加短语").color(Color32::from_rgb(5, 150, 105))).clicked() {
                                    let code = self.new_phrase_code.trim().to_lowercase();
                                    let text = self.new_phrase_text.trim().to_string();
                                    if code.is_empty() || text.is_empty() {
                                        self.phrase_status_message = Some("简写代码和展开内容均不能为空。".to_string());
                                    } else {
                                        self.custom_phrases.push((code, text));
                                        if let Err(e) = self.sync_custom_phrases() {
                                            self.phrase_status_message = Some(e);
                                        } else {
                                            self.phrase_status_message = Some("已成功添加并保存自定义短语！".to_string());
                                            self.new_phrase_code.clear();
                                            self.new_phrase_text.clear();
                                        }
                                    }
                                }
                            });

                            // 已有短语列表浏览与删除
                            ui.add_space(3.0);
                            if self.custom_phrases.is_empty() {
                                ui.label(RichText::new("暂无配置的自定义短语").size(12.0).color(Color32::from_rgb(150, 150, 150)));
                            } else {
                                let mut to_remove_idx = None;
                                egui::ScrollArea::vertical()
                                    .max_height(120.0)
                                    .id_salt("phrases_scroll_list")
                                    .show(ui, |ui| {
                                        for (idx, (code, text)) in self.custom_phrases.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.monospace(RichText::new(code).strong().color(Color32::from_rgb(5, 150, 105)));
                                                ui.label("➔");
                                                ui.label(text);
                                                if ui.small_button("删除").clicked() {
                                                    to_remove_idx = Some(idx);
                                                }
                                            });
                                        }
                                    });

                                if let Some(idx) = to_remove_idx {
                                    self.custom_phrases.remove(idx);
                                    if let Err(e) = self.sync_custom_phrases() {
                                        self.phrase_status_message = Some(e);
                                    } else {
                                        self.phrase_status_message = Some("已移除该自定义短语并同步更新。".to_string());
                                    }
                                }
                            }

                            ui.horizontal(|ui| {
                                if ui.button("在外部文本编辑器中打开 (phrases.txt)").clicked() {
                                    let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
                                    if !phrase_path.exists() {
                                        let _ = waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
                                    }
                                    let _ = std::process::Command::new("xdg-open").arg(&phrase_path).spawn();
                                }
                            });

                            if let Some(msg) = &self.phrase_status_message {
                                ui.label(
                                    RichText::new(msg)
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.0),
                                );
                            }
                        });

                        // 2. 用户自造词与调频记忆 (user_dict.bin)
                        ui.group(|ui| {
                            ui.label(RichText::new("用户自造词与个人打字记忆").strong().size(14.0));
                            ui.label(
                                RichText::new("根据您的选词频率与时间半衰期自动沉淀高频词与新词，重启后依然保留。")
                                    .size(11.0)
                                    .color(Color32::from_rgb(120, 120, 120)),
                            );

                            ui.horizontal(|ui| {
                                ui.label(format!("当前已记忆自造词/调频词条：{} 项", self.user_dict_count));

                                if ui.button("清空自造词历史").clicked() {
                                    let user_dict_path = AppConfig::user_dict_path();
                                    let empty_model = waylan_algo::user_adaptive::UserAdaptiveModel::new();
                                    if let Err(e) = empty_model.compact_wal(&user_dict_path) {
                                        self.user_dict_status_message = Some(format!("清空失败: {}", e));
                                    } else {
                                        self.user_dict_count = 0;
                                        self.user_dict_status_message = Some("已成功清空所有用户历史记忆与自造词条。".to_string());
                                    }
                                }
                            });

                            if let Some(msg) = &self.user_dict_status_message {
                                ui.label(
                                    RichText::new(msg)
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.0),
                                );
                            }
                        });
                    }
                }

                ui.add_space(8.0);
                ui.separator();

                // 操作按钮与状态反馈栏
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("保存并应用配置").strong().color(Color32::from_rgb(5, 150, 105))).clicked() {
                        self.config.double_pinyin = self.schema_choice.into();
                        match self.config.save_default() {
                            Ok(_) => {
                                self.status_message = Some("配置已成功保存！微澜守护进程已即时应用。".to_string());
                            }
                            Err(e) => {
                                self.status_message = Some(format!("保存配置失败: {}", e));
                            }
                        }
                    }

                    if ui.button("恢复默认设置").clicked() {
                        self.config = AppConfig::default();
                        self.schema_choice = PinyinSchemaChoice::from(self.config.double_pinyin);
                        self.status_message = Some("已重置为系统默认配置，请点击保存生效。".to_string());
                    }
                });

                if let Some(msg) = &self.status_message {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(msg)
                            .color(Color32::from_rgb(5, 150, 105))
                            .size(13.0),
                    );
                }
            });
    }
}

/// 运行偏好设置图形界面弹窗
pub fn run_settings_gui() -> Result<(), Box<dyn std::error::Error>> {
    // 若系统无物理显卡设备 (/dev/dri 不存在)，自动回退到 CPU 软光栅渲染模式
    if !std::path::Path::new("/dev/dri").exists() {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 700.0])
            .with_min_inner_size([460.0, 520.0])
            .with_title("微澜输入法 · 偏好设置"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "微澜输入法 · 偏好设置",
        native_options,
        Box::new(|cc| {
            SettingsApp::configure_chinese_fonts(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new()))
        }),
    )?;

    Ok(())
}
