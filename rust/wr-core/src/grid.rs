//! One panel's picture: 9 columns by 34 rows, indexed `[x][y]` like the Python grid's `grid[x, y]`.

pub const WIDTH: usize = 9;
pub const HEIGHT: usize = 34;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Values are kept as i32 because the battery's lightning bolt goes below zero before it is flipped back.
    pub v: [[i32; HEIGHT]; WIDTH],
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

impl Grid {
    pub fn new() -> Self {
        Grid { v: [[0; HEIGHT]; WIDTH] }
    }

    pub fn get(&self, x: usize, y: usize) -> i32 {
        self.v[x][y]
    }

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

    pub fn is_dark(&self) -> bool {
        self.v.iter().all(|c| c.iter().all(|&v| v <= 0))
    }
}

/// The indexes a Python slice `start:stop` covers on an axis of `len` (no negative indexes are used here).
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
