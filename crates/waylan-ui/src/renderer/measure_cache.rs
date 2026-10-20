//! 文本排版度量与字形序列缓存模块
//!
//! # 设计原理
//! - **实现初衷**：消除同一文本在界面排版测量（Measure）与最终光栅化绘制（Draw）时的双重 HarfBuzz 塑形开销。
//! - **核心优势**：固定 128 项直接映射哈希表，零堆分配淘汰；缓存单项包含排版宽度与物理字形坐标序列。
//! - **代价与局限**：容量固定为 128 项，极极端并发大量不同长度字符串时会产生哈希冲突覆盖，但属于安全降级。

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use std::hash::{DefaultHasher, Hash, Hasher};

pub(crate) const MEASURE_CACHE_SIZE: usize = 128;

/// 单个排版字形的物理光栅化定位参数
#[derive(Clone, Copy)]
pub(crate) struct CachedGlyph {
    pub(crate) cache_key: cosmic_text::CacheKey,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// 单项文本排版度量与字形序列缓存条目
#[derive(Clone, Default)]
pub(crate) struct ShapedTextEntry {
    pub(crate) hash: u64,
    pub(crate) size_bits: u32,
    pub(crate) width: f32,
    pub(crate) glyphs: Vec<CachedGlyph>,
    pub(crate) text: String,
}

/// 文本塑形度量缓存表
pub(crate) struct TextMeasureCache {
    entries: [ShapedTextEntry; MEASURE_CACHE_SIZE],
}

impl Default for TextMeasureCache {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| ShapedTextEntry::default()),
        }
    }
}

impl TextMeasureCache {
    /// 创建空的文本测量缓存表
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 清空所有缓存项（如重载字体或字号全局变更时）
    pub(crate) fn clear(&mut self) {
        for entry in &mut self.entries {
            entry.hash = 0;
            entry.text.clear();
        }
    }

    /// 确保文本完成 HarfBuzz 排版塑形并存入直接映射缓存，消除同一文本测量与绘制时的双重排版
    pub(crate) fn ensure_shaped<'a>(
        &'a mut self,
        buffer: &mut Buffer,
        font_system: &mut FontSystem,
        text: &str,
        size: f32,
    ) -> &'a ShapedTextEntry {
        let size_bits = size.to_bits();
        let hash = calculate_measure_hash(text, size_bits);
        let idx = (hash as usize) & (MEASURE_CACHE_SIZE - 1);

        if self.entries[idx].hash == hash
            && self.entries[idx].size_bits == size_bits
            && self.entries[idx].text == text
        {
            return &self.entries[idx];
        }

        let (width, glyphs) = shape_text_runs(buffer, font_system, text, size);

        let entry = &mut self.entries[idx];
        entry.hash = hash;
        entry.size_bits = size_bits;
        entry.width = width;
        entry.glyphs = glyphs;
        entry.text.clear();
        entry.text.push_str(text);

        &self.entries[idx]
    }
}

/// 对文本执行 HarfBuzz 排版塑形，返回排版宽度与物理字形序列
fn shape_text_runs(
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    text: &str,
    size: f32,
) -> (f32, Vec<CachedGlyph>) {
    buffer.set_metrics(Metrics::new(size, size * 1.2));
    let shaping = if text.is_ascii() {
        Shaping::Basic
    } else {
        Shaping::Advanced
    };
    buffer.set_text(text, &Attrs::new().family(Family::SansSerif), shaping, None);
    buffer.shape_until_scroll(font_system, false);

    let mut width = 0.0f32;
    let mut glyphs = Vec::with_capacity(text.len().max(4));
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, -run.line_y), 1.0);
            glyphs.push(CachedGlyph {
                cache_key: physical.cache_key,
                x: physical.x,
                y: physical.y,
            });
        }
    }

    (width, glyphs)
}

/// 计算文本测量查询键的哈希指纹
fn calculate_measure_hash(text: &str, size_bits: u32) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    size_bits.hash(&mut hasher);
    let h = hasher.finish();
    if h == 0 {
        1
    } else {
        h
    }
}
