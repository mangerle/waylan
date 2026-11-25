//! 文本排版度量与字形序列缓存模块
//!
//! # 设计原理
//! - **实现初衷**：消除同一文本在界面排版测量（Measure）与最终光栅化绘制（Draw）时的双重 HarfBuzz 塑形开销。
//! - **核心优势**：256 槽 2 路组相联伪 LRU 缓存表（128 组 × 2 路），零额外堆索引结构；
//!   每次未命中直接复用槽位既有 `Vec<CachedGlyph>` 容量，避免大网格高频塑形引发的堆分配与哈希冲突抖动。
//! - **代价与局限**：极端情况下单组 2 路仍可能被同时访问的第 3 个高频词淘汰，但相比直接映射命中率大幅提升。

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use std::hash::{DefaultHasher, Hash, Hasher};

/// 文本塑形度量缓存表的组数（必须为 2 的幂，每组 2 路共 256 槽）
pub(crate) const MEASURE_CACHE_SETS: usize = 128;
/// 总缓存槽位数量（供外部测试与容量估算使用）
pub(crate) const MEASURE_CACHE_SIZE: usize = MEASURE_CACHE_SETS * 2;

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

/// 文本塑形度量缓存表（2 路组相联伪 LRU）
pub(crate) struct TextMeasureCache {
    entries: Vec<ShapedTextEntry>,
    /// 每组的最近访问路标识（`false` 表示第 0 路最近被用过，淘汰第 1 路；反之亦然）
    recent_way: [bool; MEASURE_CACHE_SETS],
}

/// 单次文本塑形度量的输入参数
///
/// # 设计原理
/// - **实现初衷**：塑形所需的"文本内容 + 字号"是同一语义单元，与塑形器句柄分开传递易错位。
/// - **核心优势**：调用点按名传参，避免文本与字号顺序颠倒造成的静默错排。
/// - **代价与局限**：多引入一个结构体类型。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextShapeParams<'a> {
    /// 待塑形的文本内容
    pub(crate) text: &'a str,
    /// 塑形使用的字号（像素）
    pub(crate) size: f32,
}

impl Default for TextMeasureCache {
    fn default() -> Self {
        Self {
            entries: vec![ShapedTextEntry::default(); MEASURE_CACHE_SIZE],
            recent_way: [false; MEASURE_CACHE_SETS],
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
            entry.glyphs.clear();
        }
        self.recent_way.fill(false);
    }

    /// 确保文本完成 HarfBuzz 排版塑形并存入 2 路组相联缓存，消除同一文本测量与绘制时的双重排版
    pub(crate) fn ensure_shaped<'a>(
        &'a mut self,
        buffer: &mut Buffer,
        font_system: &mut FontSystem,
        params: &TextShapeParams<'_>,
    ) -> &'a ShapedTextEntry {
        let text = params.text;
        let size = params.size;
        let size_bits = size.to_bits();
        let hash = calculate_measure_hash(text, size_bits);
        let set_idx = (hash as usize) & (MEASURE_CACHE_SETS - 1);
        let base = set_idx * 2;

        if self.entries[base].hash == hash
            && self.entries[base].size_bits == size_bits
            && self.entries[base].text == text
        {
            self.recent_way[set_idx] = false;
            return &self.entries[base];
        }
        if self.entries[base + 1].hash == hash
            && self.entries[base + 1].size_bits == size_bits
            && self.entries[base + 1].text == text
        {
            self.recent_way[set_idx] = true;
            return &self.entries[base + 1];
        }

        // 选择空槽或最久未访问的一路作为替换目标，并原地复用其 Vec<CachedGlyph> 缓冲区
        let replace_way = if self.entries[base].hash == 0 {
            0
        } else if self.entries[base + 1].hash == 0 || !self.recent_way[set_idx] {
            1
        } else {
            0
        };
        let target_idx = base + replace_way;
        self.recent_way[set_idx] = replace_way == 1;

        let entry = &mut self.entries[target_idx];
        let width = shape_text_runs_into(buffer, font_system, text, size, &mut entry.glyphs);
        entry.hash = hash;
        entry.size_bits = size_bits;
        entry.width = width;
        entry.text.clear();
        entry.text.push_str(text);

        &self.entries[target_idx]
    }
}

/// 对文本执行 HarfBuzz 排版塑形，写入既有字形切片并返回排版宽度（零向量重分配）
fn shape_text_runs_into(
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    text: &str,
    size: f32,
    glyphs: &mut Vec<CachedGlyph>,
) -> f32 {
    buffer.set_metrics(Metrics::new(size, size * 1.2));
    let shaping = if text.is_ascii() {
        Shaping::Basic
    } else {
        Shaping::Advanced
    };
    buffer.set_text(text, &Attrs::new().family(Family::SansSerif), shaping, None);
    buffer.shape_until_scroll(font_system, false);

    let mut width = 0.0f32;
    glyphs.clear();
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

    width
}

/// 计算文本测量查询键的哈希指纹
fn calculate_measure_hash(text: &str, size_bits: u32) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    size_bits.hash(&mut hasher);
    let h = hasher.finish();
    if h == 0 { 1 } else { h }
}
