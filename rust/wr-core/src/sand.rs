//! The "Battery (sand)" app, a port of `led_mon/sand.py`: the battery level as a pile of sand in a
//! half panel. Charging rains grains in from the top until the pile matches the battery level; on
//! battery the pile drains away grain by grain from the bottom as the level drops.
//!
//! The grains fall with Python's `random.Random`, number for number ([`crate::pyrand`]), so a pile
//! with the same seed falls exactly like the Python one. The time comes in as `now` (seconds, like
//! `time.time()`) instead of being read here.
//!
//! Where it sits: `render.rs` calls `draw_sand_battery` for the "battery-sand" app on every frame (about
//! ten a second), passing the piles it keeps in its global `MOVING` store, so each pile lives on from
//! frame to frame. The window's preview uses the same code with its own piles.
//!
//! The model: the half panel's 7 content columns x 15 or 16 rows are cells that hold a grain or not.
//! The number of grains the pile should have is "battery level x number of cells". Each frame the pile
//! takes a few steps (1-3 by the speed setting): add or remove one grain if the count is off, then let
//! every grain fall one row (straight down, or diagonally off a slope). So a change in the level shows
//! as sand slowly raining in or draining out instead of a jump.

use std::collections::HashMap;

use crate::grid::Grid;
use crate::pyrand::Random;
use crate::pyround::{int, int_round};

/// Grain steps per frame for each speed setting (the monitor draws about 10 frames a second).
pub const SPEEDS: [(&str, usize); 3] = [("slow", 1), ("normal", 2), ("fast", 3)];
/// The names shown in the window for each speed setting.
pub const SPEED_LABELS: [(&str, &str); 3] = [("slow", "Slow"), ("normal", "Normal"), ("fast", "Fast")];
/// At or below this battery level, off the charger, the pile flashes.
pub const LOW: f64 = 0.07;

/// Grain steps per frame for a speed setting; unknown settings count as "normal".
pub fn speed_steps(speed: &str) -> usize {
    SPEEDS.iter().find(|(k, _)| *k == speed).map_or(2, |&(_, n)| n)
}

/// Grains in a box of `rows` x `cols`. `cells[row][col]` is true for a grain; row 0 is the top.
#[derive(Clone)]
pub struct SandPile {
    /// Height of the box in cells (LED rows).
    pub rows: usize,
    /// Width of the box in cells (always 7, the content columns of a half panel, in the app).
    pub cols: usize,
    /// `cells[row][col]`, true where there is a grain; row 0 is the top.
    pub cells: Vec<Vec<bool>>,
    /// The pile's own Python-compatible random numbers (which column a grain falls into, and so on).
    rng: Random,
    /// False until the first update, which fills the pile instantly instead of raining it in.
    pub started: bool,
}

impl SandPile {
    /// An empty pile. `seed` is `random.Random(seed)`'s seed; None seeds it from the clock.
    pub fn new(rows: usize, cols: usize, seed: Option<u64>) -> SandPile {
        SandPile {
            rows,
            cols,
            cells: vec![vec![false; cols]; rows],
            rng: seed.map_or_else(Random::from_entropy, Random::new),
            started: false,
        }
    }

    /// How many grains are in the pile.
    pub fn count(&self) -> usize {
        self.cells.iter().map(|row| row.iter().filter(|&&c| c).count()).sum()
    }

    /// Fill instantly to `target` grains, packed from the bottom (used on the first draw).
    ///
    /// Grains fill the bottom row first, left to right, then the row above. A `target` over the box size
    /// fills the box.
    pub fn settle_to(&mut self, target: usize) {
        for row in &mut self.cells {
            row.fill(false);
        }
        for i in 0..target.min(self.rows * self.cols) {
            let r = self.rows - 1 - i / self.cols;
            let c = i % self.cols;
            self.cells[r][c] = true;
        }
    }

    /// One fall step: each grain drops straight down, or slides diagonally off a pile. True if any moved.
    ///
    /// Rows are walked from the second-lowest upwards, so a grain that has just fallen into a row is not
    /// moved again in the same step: every grain moves at most one row per step. Within a row the
    /// columns are visited in a random order so the pile does not lean to one side.
    pub fn step(&mut self) -> bool {
        let mut moved = false;
        for r in (0..self.rows.saturating_sub(1)).rev() {
            let mut cols: Vec<usize> = (0..self.cols).collect();
            self.rng.shuffle(&mut cols);
            for c in cols {
                if !self.cells[r][c] {
                    continue;
                }
                if !self.cells[r + 1][c] {
                    self.cells[r][c] = false;
                    self.cells[r + 1][c] = true;
                    moved = true;
                    continue;
                }
                // Blocked below: it may slide down-left or down-right, but only if both the cell beside it
                // and the one below that are free (it cannot squeeze between two grains at a corner).
                let sides: Vec<i64> = [-1i64, 1]
                    .into_iter()
                    .filter(|&d| {
                        let n = c as i64 + d;
                        (0..self.cols as i64).contains(&n)
                            && !self.cells[r + 1][n as usize]
                            && !self.cells[r][n as usize]
                    })
                    .collect();
                if !sides.is_empty() {
                    // Python calls `choice` even when only one side is free; doing the same keeps the
                    // random numbers in step with Python.
                    let d = self.rng.choice(&sides);
                    self.cells[r][c] = false;
                    self.cells[r + 1][(c as i64 + d) as usize] = true;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Drop one grain in at a free place along the top row.
    ///
    /// Does nothing if the top row is full.
    pub fn add_grain(&mut self) {
        let free: Vec<usize> = (0..self.cols).filter(|&c| !self.cells[0][c]).collect();
        if !free.is_empty() {
            let c = self.rng.choice(&free);
            self.cells[0][c] = true;
        }
    }

    /// Let one grain out through the floor, from the bottom row.
    ///
    /// A random grain of the bottom row disappears; the grains above then fall into the gap over the
    /// following steps. Does nothing if the bottom row is empty.
    pub fn remove_grain(&mut self) {
        let last = self.rows - 1;
        let bottom: Vec<usize> = (0..self.cols).filter(|&c| self.cells[last][c]).collect();
        if !bottom.is_empty() {
            let c = self.rng.choice(&bottom);
            self.cells[last][c] = false;
        }
    }

    /// Move the pile `steps` grain steps towards `target` grains (or fill it at once the first time).
    ///
    /// `target` is clamped to 0 up to the box size. Each step adds or removes at most one grain, then lets
    /// everything fall one row.
    pub fn update(&mut self, target: i64, steps: usize) {
        let target = target.min((self.rows * self.cols) as i64).max(0) as usize;
        if !self.started {
            self.settle_to(target);
            self.started = true;
            return;
        }
        for _ in 0..steps {
            let n = self.count();
            if n < target {
                self.add_grain();
            } else if n > target {
                self.remove_grain();
            }
            self.step();
        }
    }
}

/// The piles being drawn, one per panel and half panel (Python's `_piles` and `pile_for`).
#[derive(Clone, Default)]
pub struct SandPiles {
    /// Keyed by (panel key such as "left", the half's top row `y`).
    piles: HashMap<(String, usize), SandPile>,
}

impl SandPiles {
    /// No piles yet; they are made on first use.
    pub fn new() -> SandPiles {
        SandPiles::default()
    }

    /// The pile for `key` (the panel) and `y` (the half), made anew with `seed` if there is none yet
    /// or it has a different number of rows.
    pub fn pile_for(&mut self, key: &str, y: usize, rows: usize, seed: Option<u64>) -> &mut SandPile {
        // Seven columns: the content width inside a half panel's border.
        let entry = self.piles.entry((key.to_string(), y)).or_insert_with(|| SandPile::new(rows, 7, seed));
        if entry.rows != rows {
            *entry = SandPile::new(rows, 7, seed);
        }
        entry
    }
}

/// Draw into the half panel starting at row `y` (0 top half, 16 bottom half), inside the border.
///
/// `battery` is (level 0.0-1.0, plugged in), or None when there is no battery (nothing is drawn).
/// `key` names the panel, so each panel keeps its own pile. `now` is the time in seconds.
#[allow(clippy::too_many_arguments)]
pub fn draw_sand_battery(
    piles: &mut SandPiles,
    grid: &mut Grid,
    battery: Option<(f64, bool)>,
    fill_value: i32,
    y: usize,
    key: &str,
    speed: &str,
    now: f64,
) {
    let top = y + 1;
    // The content rows inside the border: rows 1-15 on the top half, 17-32 on the bottom half.
    let rows = if y == 0 { 15 } else { 16 };
    // `None`: a real pile is seeded from the clock. The pile is made even without a battery, but it is
    // only updated and drawn when there is one.
    let pile = piles.pile_for(key, y, rows, None);
    let Some((ratio, plugged)) = battery else {
        return;
    };
    // The number of grains the level stands for. The pile keeps moving during the flash below, so it
    // stays up to date while hidden.
    let target = int_round(rows as f64 * 7.0 * ratio.clamp(0.0, 1.0));
    pile.update(target, speed_steps(speed));
    if ratio <= LOW && !plugged && now.rem_euclid(2.0) < 1.0 {
        return; // Flash when nearly empty, like the plain battery app
    }
    // Each grain is a little brighter or dimmer, so the pile reads as sand
    for r in 0..rows {
        for c in 0..7 {
            // Every third cell along the diagonals is at 80%, a fixed texture.
            let shade = if (r + c) % 3 == 0 { 0.8 } else { 1.0 };
            let cell = if pile.cells[r][c] { 1.0 } else { 0.0 };
            let value = int(cell * shade * fill_value as f64) as i32;
            let old = grid.get(1 + c, top + r);
            // `max` keeps anything already drawn there; the sand never darkens a pixel.
            grid.set(1 + c, top + r, old.max(value));
        }
    }
    // (Not when full: at 99.5% or more there is nothing left to fill.)
    if plugged && ratio < 0.995 {
        // While charging, a dimmer grain keeps falling into the pile
        if let (col, Some(row)) = drip_position(pile, now, 12.0) {
            let old = grid.get(1 + col, top + row);
            grid.set(1 + col, top + row, old.max(int(fill_value as f64 * 0.6) as i32));
        }
    }
}

/// (column, row) of the falling charging grain, or (column, None) when there is no room for it.
/// `rate` is rows per second (12 in the app).
///
/// The grain falls one row per tick (`rate` ticks a second) from the top to the pile's surface, then
/// starts again. Every `rows` ticks it moves 3 columns to the right (wrapping), so the drips land in
/// different places. When its column is full to the top there is nowhere to fall and nothing is drawn.
pub fn drip_position(pile: &SandPile, now: f64, rate: f64) -> (usize, Option<usize>) {
    let cycle = int(now * rate);
    let rows = pile.rows as i64;
    let col = (cycle.div_euclid(rows) * 3).rem_euclid(pile.cols as i64) as usize;
    // The first row from the top holding a grain in this column (or the floor if the column is empty).
    let surface = (0..pile.rows).find(|&r| pile.cells[r][col]).unwrap_or(pile.rows) as i64;
    if surface == 0 {
        return (col, None);
    }
    (col, Some(cycle.rem_euclid(rows).rem_euclid(surface) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the first update fills the pile at once, packed from the bottom, and later updates drain
    /// it grain by grain to the new target.
    #[test]
    fn settles_from_the_bottom_and_drains() {
        let mut p = SandPile::new(4, 7, Some(1));
        p.update(9, 2);
        assert_eq!(p.count(), 9);
        assert!(p.cells[3].iter().all(|&c| c));
        assert_eq!(p.cells[2].iter().filter(|&&c| c).count(), 2);
        for _ in 0..20 {
            p.update(3, 2);
        }
        assert_eq!(p.count(), 3);
    }

    /// Proves the speed names map to their step counts and an unknown name falls back to "normal".
    #[test]
    fn unknown_speed_is_normal() {
        assert_eq!(speed_steps("fast"), 3);
        assert_eq!(speed_steps("warp"), 2);
    }
}
