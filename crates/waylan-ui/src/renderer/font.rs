use fontdue::{Font, FontSettings};
use log::{info, warn};
use std::fs;
use std::path::Path;

/// 缓存的单字形光栅化位图与度量信息
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    pub metrics: fontdue::Metrics,
    pub bitmap: Vec<u8>,
}

/// 系统字体加载与探测模块
pub struct FontManager;

impl FontManager {
    /// 加载默认系统字体
    pub fn load_default_fonts() -> Option<Font> {
        let candidates = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ];

        for path in &candidates {
            if let Ok(data) = fs::read(path) {
                if let Ok(font) = Font::from_bytes(&data[..], FontSettings::default()) {
                    info!("[矢量渲染] 成功加载默认中文字体: {}", path);
                    return Some(font);
                }
            }
        }

        if let Some(font) = Self::scan_fallback_fonts() {
            return Some(font);
        }

        warn!("[矢量渲染] 未检测到任何中文字体，启用占位降级模式。");
        None
    }

    /// 回退字体深层探测
    fn scan_fallback_fonts() -> Option<Font> {
        let search_dirs = [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "~/.local/share/fonts",
        ];

        for dir in &search_dirs {
            let path = Path::new(dir);
            if !path.exists() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if matches!(ext, "ttf" | "ttc" | "otf") {
                            if let Ok(data) = fs::read(&p) {
                                if let Ok(font) =
                                    Font::from_bytes(&data[..], FontSettings::default())
                                {
                                    return Some(font);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}
