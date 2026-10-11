/// 将字节偏移安全对齐到不大于 `idx` 的合法 UTF-8 字符边界
#[inline]
pub(crate) fn clamp_char_boundary_floor(text: &str, idx: usize) -> usize {
    let mut pos = idx.min(text.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// 将字节偏移安全对齐到不小于 `idx` 的合法 UTF-8 字符边界
#[inline]
pub(crate) fn clamp_char_boundary_ceil(text: &str, idx: usize) -> usize {
    let len = text.len();
    let mut pos = idx.min(len);
    while pos < len && !text.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

/// 计算退格删除（Backspace）在当前 surrounding_text 下应通知合成器删除的字节区间 (before_bytes, after_bytes)
pub fn calculate_delete_backward_range(surrounding: Option<&(String, u32, u32)>) -> (u32, u32) {
    let Some((text, cursor, anchor)) = surrounding else {
        return (1, 0);
    };

    let cursor_idx = *cursor as usize;
    let anchor_idx = *anchor as usize;
    let len = text.len();

    // 边界安全防御：若游标越界，返回 (0, 0) 防止截断多字节 UTF-8 字符
    if cursor_idx > len || anchor_idx > len {
        return (0, 0);
    }

    // 1. 若当前存在文本选区 (cursor != anchor)，对齐 UTF-8 边界后删除整个高亮选区
    if cursor_idx != anchor_idx {
        let safe_c = clamp_char_boundary_floor(text, cursor_idx);
        let safe_a = clamp_char_boundary_floor(text, anchor_idx);
        if safe_c > safe_a {
            return ((safe_c - safe_a) as u32, 0);
        } else {
            return (0, (safe_a - safe_c) as u32);
        }
    }

    // 2. 无选区：光标前向删除 1 个 UTF-8 字符（自动向左对齐字符边界）
    let safe_cursor = clamp_char_boundary_floor(text, cursor_idx);
    if safe_cursor == 0 {
        return (0, 0);
    }

    let prefix = &text[..safe_cursor];
    let char_len = prefix
        .chars()
        .next_back()
        .map(|c| c.len_utf8() as u32)
        .unwrap_or(1);
    (char_len, 0)
}

/// 计算向前删除（Delete）在当前 surrounding_text 下应通知合成器删除的字节区间 (before_bytes, after_bytes)
pub fn calculate_delete_forward_range(surrounding: Option<&(String, u32, u32)>) -> (u32, u32) {
    let Some((text, cursor, anchor)) = surrounding else {
        return (0, 1);
    };

    let cursor_idx = *cursor as usize;
    let anchor_idx = *anchor as usize;
    let len = text.len();

    // 边界安全防御：若游标越界，返回 (0, 0) 防止截断多字节 UTF-8 字符
    if cursor_idx > len || anchor_idx > len {
        return (0, 0);
    }

    // 1. 若当前存在文本选区 (cursor != anchor)，对齐 UTF-8 边界后删除整个高亮选区
    if cursor_idx != anchor_idx {
        let safe_c = clamp_char_boundary_floor(text, cursor_idx);
        let safe_a = clamp_char_boundary_floor(text, anchor_idx);
        if safe_c > safe_a {
            return ((safe_c - safe_a) as u32, 0);
        } else {
            return (0, (safe_a - safe_c) as u32);
        }
    }

    // 2. 无选区：光标后向删除 1 个 UTF-8 字符
    let safe_cursor = clamp_char_boundary_ceil(text, cursor_idx);
    if safe_cursor >= len {
        return (0, 0);
    }

    let suffix = &text[safe_cursor..];
    let char_len = suffix
        .chars()
        .next()
        .map(|c| c.len_utf8() as u32)
        .unwrap_or(1);
    (0, char_len)
}
