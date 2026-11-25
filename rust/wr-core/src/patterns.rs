//! The pixel patterns (digits, symbols, weather icons, app names, CPU spirals, the battery's bolt),
//! exported from `led_mon/patterns.py` by `scripts/export_patterns.py`.
//!
//! The data itself lives in `patterns_data.rs` (a generated table, excluded from rustfmt); this file
//! re-exports those tables and gives the small helpers the drawing code uses to read them. A pattern
//! is stored as a string of '0' and '1' characters, row by row, which keeps the generated file
//! readable and lets it be compared with the Python source by eye.

pub use crate::patterns_data::*;

/// A 2-D 0/1 picture with numpy's shape: `rows` x `cols`, stored row by row.
/// Patterns are `Copy` and point at `'static` strings, so passing them around costs nothing.
#[derive(Clone, Copy, Debug)]
pub struct Pattern {
    /// Height of the picture in LEDs (numpy's first axis).
    pub rows: usize,
    /// Width of the picture in LEDs (numpy's second axis).
    pub cols: usize,
    /// `rows * cols` characters, each `'0'` (off) or `'1'` (lit), row by row from the top-left.
    pub bits: &'static str,
}

impl Pattern {
    /// `pattern[r, c]`: 1 when lit.
    /// Panics if `r` / `c` are outside the pattern (the callers loop over `rows` / `cols`).
    pub fn at(&self, r: usize, c: usize) -> i32 {
        (self.bits.as_bytes()[r * self.cols + c] == b'1') as i32
    }
}

/// The pattern with this key in one of the tables.
/// `table` is one of the generated tables (for example `NUMERALS` or `ICONS`). Returns `None` for an
/// unknown key, which the callers treat as "draw nothing for this character".
pub fn find(table: &[(&str, Pattern)], key: &str) -> Option<Pattern> {
    table.iter().find(|(k, _)| *k == key).map(|(_, p)| *p)
}

/// A character for the clock and weather apps, looked up like `{**numerals, **symbols, **icons}`.
/// Icons are tried first, then symbols, then numerals: in Python the later dict in `{**a, **b}` wins,
/// so this order gives the same answer when a key is in more than one table.
pub fn char_pattern(ch: &str) -> Option<Pattern> {
    find(ICONS, ch).or_else(|| find(SYMBOLS, ch)).or_else(|| find(NUMERALS, ch))
}

/// The CPU spiral picture number `index` (0-9) from `SPIRALS`; anything above 9 uses the last one.
/// Used by `drawing.rs` to show a CPU core's load as a spiral that fills up.
pub fn spiral(index: usize) -> Pattern {
    SPIRALS[index.min(9)].1
}
