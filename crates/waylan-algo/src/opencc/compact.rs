/// 简繁短语映射的紧凑存储表（字符串偏移 + 长度 + 连续字节池）
#[derive(Debug, Clone, Default)]
pub struct CompactPhraseTable {
    /// 紧凑短语条目列表：(simp_offset, simp_len, trad_offset, trad_len)
    pub records: Vec<(u32, u16, u32, u16)>,
    /// 连续字节字符串池
    pub pool: Vec<u8>,
}

impl CompactPhraseTable {
    /// 基于有序字节序二分查找简体短语对应的繁体短语
    pub fn lookup(&self, simp: &str) -> Option<&str> {
        let target_bytes = simp.as_bytes();
        let idx = self
            .records
            .binary_search_by(|&(s_off, s_len, _, _)| {
                let start = s_off as usize;
                let end = start.saturating_add(s_len as usize);
                if end > self.pool.len() {
                    return std::cmp::Ordering::Less;
                }
                let cand = &self.pool[start..end];
                cand.cmp(target_bytes)
            })
            .ok()?;

        let (_, _, t_off, t_len) = self.records[idx];
        let start = t_off as usize;
        let end = start.saturating_add(t_len as usize);
        if end > self.pool.len() {
            return None;
        }
        std::str::from_utf8(&self.pool[start..end]).ok()
    }

    /// 获取短语总条目数
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 判断是否为空
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
