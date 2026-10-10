/// 计算退格删除（Backspace）在当前 surrounding_text 下应通知合成器删除的字节区间 (before_bytes, after_bytes)
pub fn calculate_delete_backward_range(surrounding: Option<&(String, u32, u32)>) -> (u32, u32) {
    let Some((text, cursor, anchor)) = surrounding else {
        return (1, 0);
    };

    let cursor_idx = *cursor as usize;
    let anchor_idx = *anchor as usize;
    let len = text.len();

    // 边界安全防御：若游标越界，兜底返回 1 字节
    if cursor_idx > len || anchor_idx > len {
        return (1, 0);
    }

    // 1. 若当前存在文本选区 (cursor != anchor)，优先删除整个高亮选区
    if cursor_idx != anchor_idx {
        if cursor_idx > anchor_idx {
            return ((cursor_idx - anchor_idx) as u32, 0);
        } else {
            return (0, (anchor_idx - cursor_idx) as u32);
        }
    }

    // 2. 无选区：光标前向删除 1 个 UTF-8 字符
    if cursor_idx == 0 {
        return (0, 0);
    }

    // 防御性校验 UTF-8 字符边界，避免切片引发 panic
    let mut safe_cursor = cursor_idx.min(len);
    while safe_cursor > 0 && !text.is_char_boundary(safe_cursor) {
        safe_cursor -= 1;
    }
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

    // 边界安全防御：若游标越界，兜底返回 1 字节
    if cursor_idx > len || anchor_idx > len {
        return (0, 1);
    }

    // 1. 若当前存在文本选区 (cursor != anchor)，优先删除整个高亮选区
    if cursor_idx != anchor_idx {
        if cursor_idx > anchor_idx {
            return ((cursor_idx - anchor_idx) as u32, 0);
        } else {
            return (0, (anchor_idx - cursor_idx) as u32);
        }
    }

    // 2. 无选区：光标后向删除 1 个 UTF-8 字符
    if cursor_idx >= len {
        return (0, 0);
    }

    // 防御性校验 UTF-8 字符边界，避免切片引发 panic
    let mut safe_cursor = cursor_idx;
    while safe_cursor < len && !text.is_char_boundary(safe_cursor) {
        safe_cursor += 1;
    }
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
