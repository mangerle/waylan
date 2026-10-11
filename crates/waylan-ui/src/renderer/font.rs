use fontdue::{Font, FontSettings};
use log::{info, warn};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// 缓存的单字形光栅化位图与度量信息
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    pub metrics: fontdue::Metrics,
    pub bitmap: Arc<[u8]>,
}

/// 系统字体加载与探测模块
pub struct FontManager;

impl FontManager {
    /// 加载默认系统字体
    pub fn load_default_fonts() -> Option<Font> {
        let mut candidates = vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc".to_string(),
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".to_string(),
            "/usr/share/fonts/opentype/noto/NotoSansSC-Regular.otf".to_string(),
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc".to_string(),
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc".to_string(),
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".to_string(),
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc".to_string(),
        ];

        if let Ok(home) = std::env::var("HOME") {
            let user_font = PathBuf::from(home).join(".local/share/fonts");
            candidates.push(
                user_font
                    .join("NotoSansCJK-Regular.ttc")
                    .to_string_lossy()
                    .to_string(),
            );
            candidates.push(
                user_font
                    .join("NotoSansSC-Regular.otf")
                    .to_string_lossy()
                    .to_string(),
            );
        }

        for path in &candidates {
            if let Ok(data) = fs::read(path) {
                if let Ok(font) = Font::from_bytes(&data[..], FontSettings::default()) {
                    if font.lookup_glyph_index('中') != 0 {
                        info!("[矢量渲染] 成功加载默认中文字体: {}", path);
                        return Some(font);
                    }
                }
            }
        }

        if let Some(font) = Self::scan_fallback_fonts() {
            return Some(font);
        }

        warn!("[矢量渲染] 未检测到任何中文字体，启用占位降级模式。");
        None
    }

    /// 回退字体深度安全探测（基于显式栈迭代遍历子目录，防爆栈，且强制校验包含中文汉字字形）
    fn scan_fallback_fonts() -> Option<Font> {
        let mut search_dirs = vec![
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ];

        if let Ok(home) = std::env::var("HOME") {
            let user_font_dir = PathBuf::from(home).join(".local/share/fonts");
            if user_font_dir.exists() {
                search_dirs.push(user_font_dir);
            }
        }

        let mut queue = VecDeque::new();
        for dir in search_dirs {
            if dir.exists() {
                queue.push_back((dir, 0usize));
            }
        }

        while let Some((dir, depth)) = queue.pop_front() {
            if depth > 4 {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    queue.push_back((p, depth + 1));
                } else if p.is_file() {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(ext, "ttf" | "ttc" | "otf") {
                        if let Ok(data) = fs::read(&p) {
                            if let Ok(font) = Font::from_bytes(&data[..], FontSettings::default()) {
                                if font.lookup_glyph_index('中') != 0 {
                                    info!("[矢量渲染] 成功从回退目录加载中文字体: {:?}", p);
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
