//! One panel's picture: 9 columns by 34 rows, indexed `[x][y]` like the Python grid's `grid[x, y]`.
//!
//! Every app in `render.rs` / `apps/` draws into a `Grid`, the effects in `effects.rs` and `animations.rs`
//! transform grids, and the panel's drawing thread (`wr-panel`) turns the finished grid into bytes with
//! `to_bytes` and sends them to the LED panel. `x` is the column (0 = left, 8 = right) and `y` the row
//! (0 = top, 33 = bottom). A value is a brightness: 0 is off and 255 is fully lit.
//!
//! The grid is a plain value (about 1.2 KB, copied freely), with no locking: whoever owns it draws on it.
//! Writes outside the grid are silently ignored, which matches how the Python drawing code relied on numpy
//! slices being clipped to the array.

/// Number of LED columns on one Framework 16 LED Matrix panel.
pub const WIDTH: usize = 9;
/// Number of LED rows on one panel.
pub const HEIGHT: usize = 34;

/// The brightness of every LED on one panel, `v[x][y]`. Values are normally 0-255; see the field note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Values are kept as i32 because the battery's lightning bolt goes below zero before it is flipped back.
    pub v: [[i32; HEIGHT]; WIDTH],
}

impl Default for Grid {
    /// An all-dark grid, the same as `Grid::new()`.
    fn default() -> Self {
        Self::new()
    }
}

impl Grid {
    /// An all-dark grid (every value 0).
    pub fn new() -> Self {
        Grid { v: [[0; HEIGHT]; WIDTH] }
    }

    /// The value at column `x`, row `y`. Panics if either is outside the grid (unlike `set`).
    pub fn get(&self, x: usize, y: usize) -> i32 {
        self.v[x][y]
    }

    /// Sets one LED. Coordinates outside the grid are ignored on purpose, so drawing code can run off the
    /// edge without checking, as the Python numpy code could.
    pub fn set(&mut self, x: usize, y: usize, value: i32) {
        if x < WIDTH && y < HEIGHT {
            self.v[x][y] = value;
        }
    }

    /// `grid[x0:x1, y] = value`, with Python's clipping of a slice to the grid.
    pub fn fill_x(&mut self, x0: i64, x1: i64, y: usize, value: i32) {
        for x in clip(x0, x1, WIDTH) {
            self.set(x, y, value);
        }
    }

    /// `grid[x, y0:y1] = value`, with Python's clipping of a slice to the grid.
    pub fn fill_y(&mut self, x: usize, y0: i64, y1: i64, value: i32) {
        for y in clip(y0, y1, HEIGHT) {
            self.set(x, y, value);
        }
    }

    /// The values as bytes (0-255), clipped like numpy's `np.clip(grid, 0, 255).astype(np.uint8)`.
    pub fn to_bytes(&self) -> [[u8; HEIGHT]; WIDTH] {
        let mut out = [[0u8; HEIGHT]; WIDTH];
        for (column, values) in out.iter_mut().zip(&self.v) {
            for (byte, v) in column.iter_mut().zip(values) {
                *byte = (*v).clamp(0, 255) as u8;
            }
        }
        out
    }

    /// True when no LED would light: every value is 0 or below (negative values clip to 0 when sent).
    pub fn is_dark(&self) -> bool {
        self.v.iter().all(|c| c.iter().all(|&v| v <= 0))
    }
}

/// The indexes a Python slice `start:stop` covers on an axis of `len` (no negative indexes are used here).
///
/// Example: `clip(-2, 4, 9)` is `0..4` and `clip(7, 20, 9)` is `7..9`. A slice that ends before it
/// starts (or lies wholly outside) is empty, as in Python.
pub fn clip(start: i64, stop: i64, len: usize) -> std::ops::Range<usize> {
    let len = len as i64;
    let s = start.clamp(0, len);
    let e = stop.clamp(0, len);
    if e <= s {
        0..0
    } else {
        s as usize..e as usize
    }
}
