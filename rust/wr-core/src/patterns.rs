//! The pixel patterns (digits, symbols, weather icons, app names, CPU spirals, the battery's bolt),
//! exported from `led_mon/patterns.py` by `scripts/export_patterns.py`.

pub use crate::patterns_data::*;

/// A 2-D 0/1 picture with numpy's shape: `rows` x `cols`, stored row by row.
#[derive(Clone, Copy, Debug)]
pub struct Pattern {
    pub rows: usize,
    pub cols: usize,
    pub bits: &'static str,
}

impl Pattern {
    /// `pattern[r, c]`: 1 when lit.
    pub fn at(&self, r: usize, c: usize) -> i32 {
        (self.bits.as_bytes()[r * self.cols + c] == b'1') as i32
    }
}

/// The pattern with this key in one of the tables.
pub fn find(table: &[(&str, Pattern)], key: &str) -> Option<Pattern> {
    table.iter().find(|(k, _)| *k == key).map(|(_, p)| *p)
}

/// A character for the clock and weather apps, looked up like `{**numerals, **symbols, **icons}`.
pub fn char_pattern(ch: &str) -> Option<Pattern> {
    find(ICONS, ch).or_else(|| find(SYMBOLS, ch)).or_else(|| find(NUMERALS, ch))
}

pub fn spiral(index: usize) -> Pattern {
    SPIRALS[index.min(9)].1
}
