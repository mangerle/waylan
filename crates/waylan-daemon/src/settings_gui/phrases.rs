use crate::config::AppConfig;
use eframe::egui::{self, Color32, RichText};

/// 渲染自定义短语与用户自造词标签页
pub fn render_custom_phrases_tab(app: &mut crate::settings_gui::SettingsApp, ui: &mut egui::Ui) {
    // 1. 用户自定义短语管理 (phrases.txt)
    ui.group(|ui| {
        ui.label(
            RichText::new("自定义短语管理 (快捷展开)")
                .strong()
                .size(14.0),
        );
        ui.label(
            RichText::new("输入简写代码直接置顶展开预设长文本（如敲入 mymail 展开邮箱地址）")
                .size(11.0)
                .color(Color32::from_rgb(120, 120, 120)),
        );

        // 新增短语输入栏
        ui.horizontal(|ui| {
            ui.label("简写代码:");
            ui.add(egui::TextEdit::singleline(&mut app.new_phrase_code).desired_width(70.0));

            ui.label("展开内容:");
            ui.add(egui::TextEdit::singleline(&mut app.new_phrase_text).desired_width(160.0));

            if ui
                .button(RichText::new("添加短语").color(Color32::from_rgb(5, 150, 105)))
                .clicked()
            {
                let code = app.new_phrase_code.trim().to_lowercase();
                let text = app.new_phrase_text.trim().to_string();
                if code.is_empty() || text.is_empty() {
                    app.phrase_status_message = Some("简写代码和展开内容均不能为空。".to_string());
                } else {
                    app.custom_phrases.push((code, text));
                    if let Err(e) = app.sync_custom_phrases() {
                        app.phrase_status_message = Some(e);
                    } else {
                        app.phrase_status_message =
                            Some("已成功添加并保存自定义短语！".to_string());
                        app.new_phrase_code.clear();
                        app.new_phrase_text.clear();
                    }
                }
            }
        });

        // 已有短语列表浏览与删除
        ui.add_space(3.0);
        if app.custom_phrases.is_empty() {
            ui.label(
                RichText::new("暂无配置的自定义短语")
                    .size(12.0)
                    .color(Color32::from_rgb(150, 150, 150)),
            );
        } else {
            let mut to_remove_idx = None;
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .id_salt("phrases_scroll_list")
                .show(ui, |ui| {
                    for (idx, (code, text)) in app.custom_phrases.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.monospace(
                                RichText::new(code)
                                    .strong()
                                    .color(Color32::from_rgb(5, 150, 105)),
                            );
                            ui.label("➔");
                            ui.label(text);
                            if ui.small_button("删除").clicked() {
                                to_remove_idx = Some(idx);
                            }
                        });
                    }
                });

            if let Some(idx) = to_remove_idx {
                app.custom_phrases.remove(idx);
                if let Err(e) = app.sync_custom_phrases() {
                    app.phrase_status_message = Some(e);
                } else {
                    app.phrase_status_message = Some("已移除该自定义短语并同步更新。".to_string());
                }
            }
        }

        ui.horizontal(|ui| {
            if ui.button("在外部文本编辑器中打开 (phrases.txt)").clicked() {
                let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
                if !phrase_path.exists() {
                    let _ =
                        waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
                }
                let _ = std::process::Command::new("xdg-open")
                    .arg(&phrase_path)
                    .spawn();
            }
        });

        if let Some(msg) = &app.phrase_status_message {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(5, 150, 105))
                    .size(12.0),
            );
        }
    });

    // 2. 用户自造词与调频记忆 (user_dict.bin)
    ui.group(|ui| {
        ui.label(
            RichText::new("用户自造词与个人打字记忆")
                .strong()
                .size(14.0),
        );
        ui.label(
            RichText::new("根据您的选词频率与时间半衰期自动沉淀高频词与新词，重启后依然保留。")
                .size(11.0)
                .color(Color32::from_rgb(120, 120, 120)),
        );

        ui.horizontal(|ui| {
            ui.label(format!(
                "当前已记忆自造词/调频词条：{} 项",
                app.user_dict_count
            ));

            if ui.button("清空自造词历史").clicked() {
                let user_dict_path = AppConfig::user_dict_path();
                let empty_model = waylan_algo::user_adaptive::UserAdaptiveModel::new();
                if let Err(e) = empty_model.compact_wal(&user_dict_path) {
                    app.user_dict_status_message = Some(format!("清空失败: {}", e));
                } else {
                    app.user_dict_count = 0;
                    app.user_dict_status_message =
                        Some("已成功清空所有用户历史记忆与自造词条。".to_string());
                }
            }
        });

        if let Some(msg) = &app.user_dict_status_message {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(5, 150, 105))
                    .size(12.0),
            );
        }
    });
}
