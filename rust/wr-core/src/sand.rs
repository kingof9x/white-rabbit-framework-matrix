//! The "Battery (sand)" app, a port of `led_mon/sand.py`: the battery level as a pile of sand in a
//! half panel. Charging rains grains in from the top until the pile matches the battery level; on
//! battery the pile drains away grain by grain from the bottom as the level drops.
//!
//! The grains fall with Python's `random.Random`, number for number ([`crate::pyrand`]), so a pile
//! with the same seed falls exactly like the Python one. The time comes in as `now` (seconds, like
//! `time.time()`) instead of being read here.

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
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Vec<bool>>,
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
    pub fn add_grain(&mut self) {
        let free: Vec<usize> = (0..self.cols).filter(|&c| !self.cells[0][c]).collect();
        if !free.is_empty() {
            let c = self.rng.choice(&free);
            self.cells[0][c] = true;
        }
    }

    /// Let one grain out through the floor, from the bottom row.
    pub fn remove_grain(&mut self) {
        let last = self.rows - 1;
        let bottom: Vec<usize> = (0..self.cols).filter(|&c| self.cells[last][c]).collect();
        if !bottom.is_empty() {
            let c = self.rng.choice(&bottom);
            self.cells[last][c] = false;
        }
    }

    /// Move the pile `steps` grain steps towards `target` grains (or fill it at once the first time).
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
    piles: HashMap<(String, usize), SandPile>,
}

impl SandPiles {
    pub fn new() -> SandPiles {
        SandPiles::default()
    }

    /// The pile for `key` (the panel) and `y` (the half), made anew with `seed` if there is none yet
    /// or it has a different number of rows.
    pub fn pile_for(&mut self, key: &str, y: usize, rows: usize, seed: Option<u64>) -> &mut SandPile {
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
    let rows = if y == 0 { 15 } else { 16 };
    let pile = piles.pile_for(key, y, rows, None);
    let Some((ratio, plugged)) = battery else {
        return;
    };
    let target = int_round(rows as f64 * 7.0 * ratio.clamp(0.0, 1.0));
    pile.update(target, speed_steps(speed));
    if ratio <= LOW && !plugged && now.rem_euclid(2.0) < 1.0 {
        return; // Flash when nearly empty, like the plain battery app
    }
    // Each grain is a little brighter or dimmer, so the pile reads as sand
    for r in 0..rows {
        for c in 0..7 {
            let shade = if (r + c) % 3 == 0 { 0.8 } else { 1.0 };
            let cell = if pile.cells[r][c] { 1.0 } else { 0.0 };
            let value = int(cell * shade * fill_value as f64) as i32;
            let old = grid.get(1 + c, top + r);
            grid.set(1 + c, top + r, old.max(value));
        }
    }
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
pub fn drip_position(pile: &SandPile, now: f64, rate: f64) -> (usize, Option<usize>) {
    let cycle = int(now * rate);
    let rows = pile.rows as i64;
    let col = (cycle.div_euclid(rows) * 3).rem_euclid(pile.cols as i64) as usize;
    let surface = (0..pile.rows).find(|&r| pile.cells[r][col]).unwrap_or(pile.rows) as i64;
    if surface == 0 {
        return (col, None);
    }
    (col, Some(cycle.rem_euclid(rows).rem_euclid(surface) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn unknown_speed_is_normal() {
        assert_eq!(speed_steps("fast"), 3);
        assert_eq!(speed_steps("warp"), 2);
    }
}
