use cosmic_text::FontSystem;
use log::{debug, info};
use std::path::Path;
use waylan_core::config::user_data_dir_opt;

/// 系统字体加载与探测模块
///
/// # 设计原理
/// - **实现初衷**：为 `cosmic-text` 提供开箱即用的系统字体与 CJK 回退链初始化能力，防御精简系统无 fontconfig 配置导致的缺失。
/// - **核心优势**：自动扫描系统字体的同时显式注册关键中文字体路径，保证多语言与中文字形稳定命中。
/// - **代价与局限**：首次初始化耗时约 5~20ms，通过在渲染器生命周期中常驻复用彻底消除后续按键开销。
pub struct FontManager;

impl FontManager {
    /// 创建并初始化字体系统（基于 cosmic-text，自动集成系统字体库与 CJK 回退链）
    pub fn create_font_system() -> FontSystem {
        let mut font_system = FontSystem::new();
        Self::ensure_cjk_fonts(&mut font_system);
        font_system
    }

    /// 确保字体数据库中补充加载了可用的 CJK 中文字体
    fn ensure_cjk_fonts(font_system: &mut FontSystem) {
        let candidates = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansSC-Regular.otf",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ];

        for path in candidates {
            if Path::new(path).exists() {
                if let Err(e) = font_system.db_mut().load_font_file(path) {
                    debug!(
                        "[cosmic-text 字体库] 补充加载字体文件失败: {}，原因: {}",
                        path, e
                    );
                } else {
                    info!("[cosmic-text 字体库] 成功补充加载中文字体: {}", path);
                }
            }
        }

        if let Some(data_dir) = user_data_dir_opt() {
            let user_noto = data_dir.join("fonts/NotoSansCJK-Regular.ttc");
            if user_noto.exists() {
                let _ = font_system.db_mut().load_font_file(&user_noto);
            }
        }

        let face_count = font_system.db().faces().count();
        info!(
            "[cosmic-text 字体库] 字体系统初始化就绪，当前已加载字体变体总数: {}",
            face_count
        );
    }
}
