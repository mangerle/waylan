//! 候选词翻页快捷键配置模式

use super::constants::{
    KEY_COMMA, KEY_DOT, KEY_EQUAL, KEY_LEFTBRACE, KEY_MINUS, KEY_PAGEDOWN, KEY_PAGEUP,
    KEY_RIGHTBRACE,
};
use crate::config::PageKeys;

/// 候选词翻页快捷键配置模式（支持多选同时生效）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageKeyConfig {
    /// 减号/等号翻页 (-/=)
    pub minus_equal: bool,
    /// 方括号翻页 ([/])
    pub bracket: bool,
    /// 逗号/句号翻页 (,/.)
    pub comma_period: bool,
}

impl Default for PageKeyConfig {
    fn default() -> Self {
        Self {
            minus_equal: true,
            bracket: false,
            comma_period: false,
        }
    }
}

impl PageKeyConfig {
    /// 判断给定的键码是否对应向上一页翻页操作
    pub fn is_prev_page(&self, keycode: u32) -> bool {
        if keycode == KEY_PAGEUP {
            return true;
        }
        if self.minus_equal && keycode == KEY_MINUS {
            return true;
        }
        if self.bracket && keycode == KEY_LEFTBRACE {
            return true;
        }
        if self.comma_period && keycode == KEY_COMMA {
            return true;
        }
        false
    }

    /// 判断给定的键码是否对应向下一页翻页操作
    pub fn is_next_page(&self, keycode: u32) -> bool {
        if keycode == KEY_PAGEDOWN {
            return true;
        }
        if self.minus_equal && keycode == KEY_EQUAL {
            return true;
        }
        if self.bracket && keycode == KEY_RIGHTBRACE {
            return true;
        }
        if self.comma_period && keycode == KEY_DOT {
            return true;
        }
        false
    }

    /// 判断给定的键码是否属于当前已启用的翻页键
    pub fn is_page_key(&self, keycode: u32) -> bool {
        self.is_prev_page(keycode) || self.is_next_page(keycode)
    }
}

impl From<PageKeys> for PageKeyConfig {
    fn from(pk: PageKeys) -> Self {
        Self {
            minus_equal: pk.minus_equal,
            bracket: pk.bracket,
            comma_period: pk.comma_period,
        }
    }
}
