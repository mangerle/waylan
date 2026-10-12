use fontdue::{Font, FontSettings};
use log::{info, warn};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// 缓存的单字形光栅化位图与度量信息
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    pub metrics: fontdue::Metrics,
    pub bitmap: Arc<[u8]>,
}

/// 全局主字体加载结果缓存，避免 V2 降级 V1 或重建渲染器时重复扫描磁盘
static GLOBAL_FONT_CACHE: OnceLock<Option<Arc<Font>>> = OnceLock::new();
/// 全局回退字体链加载结果缓存，避免多次读盘与重复解析
static GLOBAL_FALLBACK_FONTS_CACHE: OnceLock<Arc<Vec<Font>>> = OnceLock::new();

/// 系统字体加载与探测模块
pub struct FontManager;

impl FontManager {
    /// 加载默认系统字体（通过全局 OnceLock 与 Arc 共享避免重复扫描与深拷贝）
    pub fn load_default_fonts() -> Option<Arc<Font>> {
        let cached =
            GLOBAL_FONT_CACHE.get_or_init(|| Self::load_default_fonts_impl().map(Arc::new));
        cached.as_ref().map(Arc::clone)
    }

    /// 加载系统回退字体链（包括特殊符号及常见备选字体，通过 Arc 共享）
    pub fn load_fallback_fonts() -> Arc<Vec<Font>> {
        let cached =
            GLOBAL_FALLBACK_FONTS_CACHE.get_or_init(|| Arc::new(Self::load_fallback_fonts_impl()));
        Arc::clone(cached)
    }

    fn load_fallback_fonts_impl() -> Vec<Font> {
        let candidates = [
            // 特殊符号、数学符号与单色符号矢量字体（移除纯 CBDT 位图字体，确保 fontdue 提取有效矢量轮廓）
            "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansSymbols-Regular.otf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
            "/usr/share/fonts/truetype/ancient-scripts/Symbola.ttf",
            // 常见通用备选西文字体
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ];

        let mut fallback_fonts = Vec::new();
        for path in &candidates {
            if let Ok(data) = fs::read(path) {
                if let Ok(font) = Font::from_bytes(&data[..], FontSettings::default()) {
                    info!("[矢量渲染] 成功加载回退字体: {}", path);
                    fallback_fonts.push(font);
                }
            }
        }
        fallback_fonts
    }

    fn load_default_fonts_impl() -> Option<Font> {
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

    /// 回退字体深度安全探测（带文件数量与单文件体积预算上限，避免无限制读盘引发数秒级启动延迟）
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

        // 设置安全预算：最多检测 60 个字体文件，单个文件超过 35MB 过滤跳过
        const MAX_FILES_BUDGET: usize = 60;
        const MAX_FILE_SIZE: u64 = 35 * 1024 * 1024;
        let mut inspected_files = 0;

        while let Some((dir, depth)) = queue.pop_front() {
            if depth > 4 || inspected_files >= MAX_FILES_BUDGET {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                if inspected_files >= MAX_FILES_BUDGET {
                    break;
                }
                let p = entry.path();
                if p.is_dir() {
                    queue.push_back((p, depth + 1));
                } else if p.is_file() {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(ext, "ttf" | "ttc" | "otf") {
                        if let Ok(meta) = entry.metadata() {
                            if meta.len() > MAX_FILE_SIZE {
                                continue;
                            }
                        }
                        inspected_files += 1;
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
