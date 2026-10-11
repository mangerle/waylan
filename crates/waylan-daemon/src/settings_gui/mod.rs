mod phrases;
mod views;

use crate::config::AppConfig;
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use std::fs;
use std::sync::Arc;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;

/// 偏好设置中心导航 Tab 标签页枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
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

/// 英文词库异步导入结果类型
pub(crate) type EnglishImportResult = Result<(usize, Vec<(String, u32)>), String>;
/// 符号词库异步导入结果类型
pub(crate) type SymbolsImportResult = Result<(usize, Vec<(String, String, String)>), String>;

/// 微澜图形化偏好设置应用状态
pub(crate) struct SettingsApp {
    /// 当前选中的导航 Tab
    pub(crate) current_tab: SettingsTab,
    /// 内存中编辑的配置副本
    pub(crate) config: AppConfig,
    /// 全局操作反馈提示信息
    pub(crate) status_message: Option<String>,
    /// 汉语拼音词库文件导入路径
    pub(crate) import_file_path: String,
    /// 汉语拼音词库导入反馈提示
    pub(crate) import_status_message: Option<String>,

    /// 辅助与混输词库（包含英文词库与符号表情库）
    pub(crate) aux_dict: AuxiliaryDict,
    /// 外部英文词库导入路径
    pub(crate) english_import_path: String,
    /// 英文词库导入反馈提示
    pub(crate) english_status_message: Option<String>,
    /// 外部符号词库导入路径
    pub(crate) symbols_import_path: String,
    /// 符号词库导入反馈提示
    pub(crate) symbols_status_message: Option<String>,

    /// 当前已配置的自定义快捷短语列表 (快捷码, 展开内容)
    pub(crate) custom_phrases: Vec<(String, String)>,
    /// 新增短语的快捷代码输入
    pub(crate) new_phrase_code: String,
    /// 新增短语的展开文本输入
    pub(crate) new_phrase_text: String,
    /// 短语操作状态提示
    pub(crate) phrase_status_message: Option<String>,
    /// 用户自适应自造词库当前词条总数
    pub(crate) user_dict_count: usize,
    /// 用户自造词库操作状态提示
    pub(crate) user_dict_status_message: Option<String>,
    /// 词库异步导入进度接收端 (导入数, 当前总词数)
    pub(crate) import_rx: Option<std::sync::mpsc::Receiver<Result<(usize, usize), String>>>,
    /// 是否正在后台异步执行词库导入
    pub(crate) import_in_progress: bool,
    /// 英文词库异步导入接收端 (新增词数, 最新词库列表)
    pub(crate) english_import_rx: Option<std::sync::mpsc::Receiver<EnglishImportResult>>,
    /// 符号词库异步导入接收端 (新增符号数, 最新符号列表)
    pub(crate) symbols_import_rx: Option<std::sync::mpsc::Receiver<SymbolsImportResult>>,
}

impl Default for SettingsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsApp {
    pub(crate) fn new() -> Self {
        let config = AppConfig::load_or_create();

        // 加载现有自定义短语
        let phrase_path = CustomPhraseManager::default_path();
        let phrase_manager = CustomPhraseManager::load_or_init(&phrase_path);
        let custom_phrases = phrase_manager.all_entries();

        // 加载用户自造词统计
        let user_dict_path = AppConfig::user_dict_path();
        let user_dict_count = UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
            .map(|m| m.total_entries())
            .unwrap_or(0);

        // 加载辅助与混输词库
        let mut aux_dict = AuxiliaryDict::with_default_data();
        for path in AppConfig::aux_dict_search_paths() {
            if path.exists() {
                if let Ok(loaded) = AuxiliaryDict::load_from_file(&path) {
                    aux_dict = loaded;
                    break;
                }
            }
        }

        Self {
            current_tab: SettingsTab::General,
            config,
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
            import_rx: None,
            import_in_progress: false,
            english_import_rx: None,
            symbols_import_rx: None,
        }
    }

    /// 将当前的自定义短语同步保存写回 phrases.txt
    pub(crate) fn sync_custom_phrases(&self) -> Result<(), String> {
        let phrase_path = CustomPhraseManager::default_path();
        let mut manager = CustomPhraseManager::new();
        for (code, text) in &self.custom_phrases {
            manager.add_entry(code.clone(), text.clone());
        }
        manager
            .save_to_file(&phrase_path)
            .map_err(|e| format!("写入短语配置失败: {}", e))
    }

    /// 探测并加载系统或用户环境中的 CJK 中文字体二进制数据
    fn find_cjk_font_data() -> Option<Vec<u8>> {
        if let Some(custom) = crate::config::env_var_non_empty("WAYLAN_FONT_PATH") {
            if let Ok(data) = fs::read(custom) {
                return Some(data);
            }
        }

        let system_paths = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk-fonts/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/noto/NotoSansSC-Regular.otf",
            "/usr/share/fonts/noto/NotoSansSC-Regular.ttf",
            "/usr/share/fonts/adobe-source-han-sans/SourceHanSansCN-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/arphic/uming.ttc",
            "/usr/share/fonts/truetype/arphic/ukai.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ];

        for path in system_paths {
            if let Ok(data) = fs::read(path) {
                return Some(data);
            }
        }

        if let Some(home) = crate::config::env_var_non_empty("HOME") {
            for sub in ["NotoSansCJK-Regular.ttc", "NotoSansSC-Regular.otf"] {
                let user_font = format!("{}/.local/share/fonts/{}", home, sub);
                if let Ok(data) = fs::read(user_font) {
                    return Some(data);
                }
            }
        }
        None
    }

    /// 配置系统与备选中文字体
    pub(crate) fn configure_chinese_fonts(ctx: &egui::Context) {
        if let Some(font_data) = Self::find_cjk_font_data() {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "cjk_fallback".to_owned(),
                Arc::new(FontData::from_owned(font_data).tweak(egui::FontTweak {
                    scale: 1.0,
                    ..Default::default()
                })),
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
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 轮询后台异步词库导入任务
        if let Some(ref rx) = self.import_rx {
            if let Ok(res) = rx.try_recv() {
                self.import_in_progress = false;
                match res {
                    Ok((imported_count, total_count)) => {
                        self.user_dict_count = total_count;
                        self.import_status_message = Some(format!(
                            "成功导入 {} 条词目，并已即时合入专属用户词库！",
                            imported_count
                        ));
                    }
                    Err(e) => {
                        self.import_status_message = Some(format!("导入词库失败: {}", e));
                    }
                }
                self.import_rx = None;
            }
        }

        if let Some(ref rx) = self.english_import_rx {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok((added, words)) => {
                        self.aux_dict.english_words = words;
                        self.english_status_message =
                            Some(format!("成功导入 {} 个新英文词汇并已更新！", added));
                    }
                    Err(e) => {
                        self.english_status_message = Some(format!("导入英文词库失败: {}", e));
                    }
                }
                self.english_import_rx = None;
            }
        }

        if let Some(ref rx) = self.symbols_import_rx {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok((added, syms)) => {
                        self.aux_dict.symbols = syms;
                        self.symbols_status_message =
                            Some(format!("成功导入 {} 条符号映射并已更新！", added));
                    }
                    Err(e) => {
                        self.symbols_status_message = Some(format!("导入符号词库失败: {}", e));
                    }
                }
                self.symbols_import_rx = None;
            }
        }

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
                SettingsTab::General => views::render_general_tab(self, ui),
                SettingsTab::Pinyin => views::render_pinyin_tab(self, ui),
                SettingsTab::English => views::render_english_tab(self, ui),
                SettingsTab::Symbols => views::render_symbols_tab(self, ui),
                SettingsTab::CustomPhrases => phrases::render_custom_phrases_tab(self, ui),
            }

            ui.add_space(8.0);
            ui.separator();

            // 操作按钮与状态反馈栏
            ui.horizontal(|ui| {
                if ui
                    .button(
                        RichText::new("保存并应用配置")
                            .strong()
                            .color(Color32::from_rgb(5, 150, 105)),
                    )
                    .clicked()
                {
                    match self.config.save_default() {
                        Ok(_) => {
                            self.status_message =
                                Some("配置已成功保存！微澜守护进程已即时应用。".to_string());
                        }
                        Err(e) => {
                            self.status_message = Some(format!("保存配置失败: {}", e));
                        }
                    }
                }

                if ui.button("恢复默认设置").clicked() {
                    self.config = AppConfig::default();
                    match self.config.save_default() {
                        Ok(_) => {
                            self.status_message =
                                Some("已重置为系统默认配置并成功保存持久化！".to_string());
                        }
                        Err(e) => {
                            self.status_message =
                                Some(format!("已重置为默认配置，但落盘保存失败: {}", e));
                        }
                    }
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
    // 若系统无物理显卡设备 (/dev/dri 不存在)，在主线程 UI 启动前安全回退到 CPU 软光栅渲染模式
    if !std::path::Path::new("/dev/dri").exists() {
        if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none() {
            unsafe {
                // 安全说明：该设置仅在进程启动、尚未派生多线程工作协程的前置单线程阶段执行
                std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
            }
        }
        if std::env::var_os("GALLIUM_DRIVER").is_none() {
            unsafe {
                // 安全说明：该设置仅在进程启动、尚未派生多线程工作协程的前置单线程阶段执行
                std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_default_config_persistence() {
        let mut app = SettingsApp::new();
        app.config.page_size = 9;
        app.config.auto_pangu_spacing = true;
        assert_ne!(app.config.page_size, AppConfig::default().page_size);

        app.config = AppConfig::default();
        assert_eq!(app.config.page_size, 7);
        assert!(!app.config.auto_pangu_spacing);

        let temp_dir = tempfile::tempdir().expect("创建测试临时目录失败");
        let temp_file = temp_dir.path().join("config.toml");
        app.config
            .save_to_file(&temp_file)
            .expect("重置配置保存应成功");
        assert!(temp_file.exists());
    }

    #[test]
    fn test_find_cjk_font_data_does_not_panic() {
        let _ = SettingsApp::find_cjk_font_data();
    }
}
