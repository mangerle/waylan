use crate::config::{AppConfig, PageKeys, SwitchKey, ThemeMode};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::time::{SystemTime, UNIX_EPOCH};

/// 外部导入文本词库最大允许字节数（64 MB），防止误导入超大文件触发 OOM
const MAX_IMPORT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 渲染常规设置标签页（包含候选外观、按键切换习惯与标点/算法特性）
pub fn render_general_tab(app: &mut crate::settings_gui::SettingsApp, ui: &mut egui::Ui) {
    // 1. 候选外观与排版
    ui.group(|ui| {
        ui.label(RichText::new("候选框与排版外观").strong().size(14.0));

        ui.horizontal(|ui| {
            ui.label("单页候选词数:");
            ui.add(egui::Slider::new(&mut app.config.page_size, 5..=9).text("项"));
        });

        ui.horizontal(|ui| {
            ui.label("候选文字字号:");
            ui.add(egui::Slider::new(&mut app.config.font_size, 12.0..=24.0).text("像素 (px)"));
        });

        ui.horizontal(|ui| {
            ui.label("界面主题模式:");
            let is_light = app.config.theme == ThemeMode::Light;
            if ui.selectable_label(is_light, "浅色明亮").clicked() {
                app.config.theme = ThemeMode::Light;
            }
            if ui.selectable_label(!is_light, "深色护眼").clicked() {
                app.config.theme = ThemeMode::Dark;
            }
        });
    });

    // 2. 快捷键与输入习惯
    ui.group(|ui| {
        ui.label(RichText::new("按键与切换习惯").strong().size(14.0));

        ui.horizontal(|ui| {
            ui.label("中英文切换键:");
            let is_shift = app.config.switch_key == SwitchKey::Shift;
            if ui.selectable_label(is_shift, "Shift 键").clicked() {
                app.config.switch_key = SwitchKey::Shift;
            }
            if ui.selectable_label(!is_shift, "Ctrl 键").clicked() {
                app.config.switch_key = SwitchKey::Ctrl;
            }
        });

        ui.horizontal(|ui| {
            ui.label("候选翻页快捷键:");
            let is_minus = app.config.page_keys == PageKeys::MinusEqual;
            let is_bracket = app.config.page_keys == PageKeys::Bracket;
            let is_comma = app.config.page_keys == PageKeys::CommaPeriod;

            if ui.selectable_label(is_minus, "- / =").clicked() {
                app.config.page_keys = PageKeys::MinusEqual;
            }
            if ui.selectable_label(is_bracket, "[ / ]").clicked() {
                app.config.page_keys = PageKeys::Bracket;
            }
            if ui.selectable_label(is_comma, ", / .").clicked() {
                app.config.page_keys = PageKeys::CommaPeriod;
            }
        });
    });

    // 3. 智能标点与高级算法特性
    ui.group(|ui| {
        ui.label(RichText::new("标点与算法特性").strong().size(14.0));

        ui.checkbox(
            &mut app.config.slash_to_dunhao,
            "斜杠替换为中文顿号 (/ 转为 、)",
        );
        ui.checkbox(
            &mut app.config.protect_digits_punctuation,
            "数字间标点保护 (如 3.14 与 12:30 不转中文)",
        );
        ui.checkbox(
            &mut app.config.auto_complete_pairs,
            "成对符号智能闭合与光标回退 (如 《》、“” )",
        );
        ui.checkbox(
            &mut app.config.fuzzy_pinyin,
            "开启模糊拼音 (平翘舌、前后鼻音)",
        );
        ui.checkbox(
            &mut app.config.v_mode,
            "开启 V 模式 (输入 v1+2 快捷计算求值)",
        );
        ui.checkbox(
            &mut app.config.traditional_chinese,
            "繁体中文模式 (默认输出繁体汉字)",
        );
        ui.checkbox(
            &mut app.config.auto_pangu_spacing,
            "中西文智能空格 (盘古之白，汉字与英文/数字自动空格)",
        );
    });
}

/// 渲染拼音与词库标签页（词库状态与外部词库导入）
pub fn render_pinyin_tab(app: &mut crate::settings_gui::SettingsApp, ui: &mut egui::Ui) {
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
        ui.label(
            RichText::new("外部汉语词库导入 (搜狗 .scel / Rime YAML)")
                .strong()
                .size(14.0),
        );
        ui.horizontal(|ui| {
            ui.label("词库文件路径:");
            ui.text_edit_singleline(&mut app.import_file_path);
        });

        ui.horizontal(|ui| {
            let button_text = if app.import_in_progress {
                "正在后台导入中..."
            } else {
                "开始导入并增量合入"
            };
            if ui
                .add_enabled(!app.import_in_progress, egui::Button::new(button_text))
                .clicked()
            {
                let path = Path::new(app.import_file_path.trim());
                if !path.is_file() {
                    app.import_status_message =
                        Some("指定的词库文件不存在或不是有效文件，请检查路径。".to_string());
                } else if path
                    .metadata()
                    .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
                    .unwrap_or(true)
                {
                    app.import_status_message =
                        Some("词库文件体积超过 64MB 安全上限，已拒绝加载。".to_string());
                } else {
                    let path_buf = path.to_path_buf();
                    let (tx, rx) = sync_channel(1);
                    app.import_rx = Some(rx);
                    app.import_in_progress = true;
                    app.import_status_message =
                        Some("正在后台异步解析与合入词库，请稍候...".to_string());

                    std::thread::spawn(move || {
                        let res = (|| -> Result<(usize, usize), String> {
                            let entries =
                                waylan_algo::dict_importer::DictImporter::import_file(&path_buf)
                                    .map_err(|e| format!("{}", e))?;
                            let count = entries.len();
                            let user_dict_path = AppConfig::user_dict_path();
                            let mut user_model =
                                waylan_algo::user_adaptive::UserAdaptiveModel::load_or_replay_wal(
                                    &user_dict_path,
                                )
                                .unwrap_or_default();
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(1_700_000_000);
                            for entry in &entries {
                                user_model.record_commit(&entry.text, &entry.pinyin, now);
                            }
                            user_model
                                .compact_wal(&user_dict_path)
                                .map_err(|e| format!("保存用户词库失败: {}", e))?;
                            Ok((count, user_model.total_entries()))
                        })();
                        let _ = tx.send(res);
                    });
                }
            }
        });

        if let Some(msg) = &app.import_status_message {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(5, 150, 105))
                    .size(12.0),
            );
        }
    });
}

/// 渲染中英混输标签页
pub fn render_english_tab(app: &mut crate::settings_gui::SettingsApp, ui: &mut egui::Ui) {
    ui.group(|ui| {
        ui.label(RichText::new("中英混输特性 (English Suggestion)").strong().size(14.0));
        ui.label("在中文输入模式下，直接输入常用英文单词时，候选栏次位或首位自动呈现英文原词，按空格即可直接上屏，无需按 Shift 切换。");
        ui.label(format!("当前已就绪英文单词数: {} 词", app.aux_dict.english_words.len()));
    });

    ui.group(|ui| {
        ui.label(
            RichText::new("导入外部英文词库 (.txt / .dict.yaml)")
                .strong()
                .size(14.0),
        );
        ui.horizontal(|ui| {
            ui.label("英文词库路径:");
            ui.text_edit_singleline(&mut app.english_import_path);
        });

        ui.horizontal(|ui| {
            if ui.button("导入英文单词表").clicked() {
                let path = Path::new(app.english_import_path.trim());
                if !path.is_file() {
                    app.english_status_message =
                        Some("指定的英文词典文件不存在或不是有效文件，请检查路径。".to_string());
                } else if path
                    .metadata()
                    .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
                    .unwrap_or(true)
                {
                    app.english_status_message =
                        Some("英文词库文件体积超过 64MB 安全上限，已拒绝加载。".to_string());
                } else if let Ok(content) = fs::read_to_string(path) {
                    let mut existing: HashSet<String> = app
                        .aux_dict
                        .english_words
                        .iter()
                        .map(|(w, _)| w.to_lowercase())
                        .collect();
                    let mut added = 0;
                    let mut in_yaml_header = false;
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed == "---" {
                            in_yaml_header = true;
                            continue;
                        }
                        if trimmed == "..." {
                            in_yaml_header = false;
                            continue;
                        }
                        if in_yaml_header || trimmed.is_empty() || trimmed.starts_with('#') {
                            continue;
                        }
                        let word = trimmed.split('\t').next().unwrap_or("").trim();
                        if !word.is_empty()
                            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
                            && existing.insert(word.to_lowercase())
                        {
                            app.aux_dict.english_words.push((word.to_string(), 50_000));
                            added += 1;
                        }
                    }
                    let aux_path = AppConfig::user_aux_dict_path();
                    match app.aux_dict.save_to_file(&aux_path) {
                        Ok(()) => {
                            app.english_status_message =
                                Some(format!("成功导入 {} 个新英文词汇并已更新！", added));
                        }
                        Err(e) => {
                            app.english_status_message = Some(format!("保存英文词库失败: {}", e));
                        }
                    }
                } else {
                    app.english_status_message = Some("读取英文词库文件失败。".to_string());
                }
            }
        });

        if let Some(msg) = &app.english_status_message {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(5, 150, 105))
                    .size(12.0),
            );
        }
    });
}

/// 渲染符号与表情标签页
pub fn render_symbols_tab(app: &mut crate::settings_gui::SettingsApp, ui: &mut egui::Ui) {
    ui.group(|ui| {
        ui.label(RichText::new("符号与表情语义联想 (Symbol Suggestion)").strong().size(14.0));
        ui.label("输入常见语义拼音（如 dianzan, daxiao, shengri, jia, jifen）在候选栏次位呈现对应符号。");
        ui.label(
            RichText::new("源码与通信严格遵守无 Emoji 规范，所有符号均采用标准 Unicode 标量值分发，由系统字体光栅化渲染。")
                .size(11.5)
                .color(Color32::from_rgb(120, 120, 120)),
        );
        ui.label(format!("当前收录符号与表情映射: {} 项", app.aux_dict.symbols.len()));
    });

    ui.group(|ui| {
        ui.label(
            RichText::new("已收录符号映射一览 (示例)")
                .strong()
                .size(13.5),
        );
        egui::ScrollArea::vertical()
            .max_height(140.0)
            .id_salt("symbols_preview_list")
            .show(ui, |ui| {
                for (code, sym, desc) in app.aux_dict.symbols.iter().take(200) {
                    ui.horizontal(|ui| {
                        ui.monospace(RichText::new(code).color(Color32::from_rgb(5, 150, 105)));
                        ui.label("->");
                        ui.monospace(sym);
                        ui.label(format!("({})", desc));
                    });
                }
            });
    });

    ui.group(|ui| {
        ui.label(
            RichText::new("导入外部符号/表情词库 (.txt / .yaml)")
                .strong()
                .size(14.0),
        );
        ui.horizontal(|ui| {
            ui.label("符号词库路径:");
            ui.text_edit_singleline(&mut app.symbols_import_path);
        });

        ui.horizontal(|ui| {
            if ui.button("导入符号映射表").clicked() {
                let path = Path::new(app.symbols_import_path.trim());
                if !path.is_file() {
                    app.symbols_status_message =
                        Some("指定的符号文件不存在或不是有效文件，请检查路径。".to_string());
                } else if path
                    .metadata()
                    .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
                    .unwrap_or(true)
                {
                    app.symbols_status_message =
                        Some("符号文件体积超过 64MB 安全上限，已拒绝加载。".to_string());
                } else if let Ok(content) = fs::read_to_string(path) {
                    let mut existing: HashSet<(String, String)> = app
                        .aux_dict
                        .symbols
                        .iter()
                        .map(|(k, s, _)| (k.clone(), s.clone()))
                        .collect();
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
                            if existing.insert((key.clone(), sym.clone())) {
                                app.aux_dict.symbols.push((key, sym, desc));
                                added += 1;
                            }
                        }
                    }
                    let aux_path = AppConfig::user_aux_dict_path();
                    match app.aux_dict.save_to_file(&aux_path) {
                        Ok(()) => {
                            app.symbols_status_message =
                                Some(format!("成功导入 {} 条符号映射并已更新！", added));
                        }
                        Err(e) => {
                            app.symbols_status_message = Some(format!("保存符号词库失败: {}", e));
                        }
                    }
                } else {
                    app.symbols_status_message = Some("读取符号文件失败。".to_string());
                }
            }
        });

        if let Some(msg) = &app.symbols_status_message {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(5, 150, 105))
                    .size(12.0),
            );
        }
    });
}
