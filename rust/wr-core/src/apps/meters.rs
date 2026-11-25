//! The new monitor halves: a graph with a memory meter on top (CPU + RAM, GPU + GPU memory),
//! network over disk with a drive-space meter, and a battery shaped like a battery.
//!
//! `y` is where the half panel starts: 0 for the top half, 16 for the bottom half.
//!
//! Where this sits: `render.rs` calls these on the monitor's main loop (ten times a second per panel)
//! and in the settings window's preview. The values come from the `Readings` trait as fractions
//! 0.0-1.0; `fill_value` is the app's LED brightness (0-255) and the borders are drawn separately at
//! the dimmer background brightness. Nothing here keeps state: the moving battery looks are worked
//! out from `now` (seconds on a steady clock) each frame.
//!
//! Coordinates: the grid is 9 columns (x 0-8) by 34 rows (y 0-33). Each half has a one-pixel outline,
//! so the usable inside is columns 1-7 and, on the top half, rows 1-15 (row 0 and row 16 are its
//! outline), on the bottom half rows 17-32.

use crate::drawing::draw_history_graph_with;
use crate::grid::{Grid, WIDTH};
use crate::pyround::int_round;

/// The rows inside a half's border: 1-15 on the top half, 17-32 on the bottom half.
/// The bottom half has one more inside row because it ends at the panel's last row (33).
fn inside_rows(y: usize) -> Vec<usize> {
    if y == 0 {
        (1..16).collect()
    } else {
        (17..33).collect()
    }
}

/// A meter two rows tall and 7 wide, filled left to right, the top row first (as the memory
/// meter): 14 steps.
/// Columns 1-7 of rows `top` and `top + 1`; `ratio` is clamped to 0.0-1.0. Used for memory and for
/// drive space.
pub fn draw_two_row_meter(grid: &mut Grid, ratio: f64, fill_value: i32, top: usize) {
    // `lit` is how many of the 14 pixels should be on. The rows are rounded separately (Python's
    // rounding, halves to even) and offset so the second row runs about half a step ahead: at 50%
    // (7 pixels) the first row has 3 and the second 4, as the test below checks.
    let lit = 7.0 * 2.0 * ratio.clamp(0.0, 1.0);
    let bottom = int_round(lit / 2.0);
    let upper = int_round((lit - 0.49) / 2.0);
    // fill_x's end column is exclusive: columns 1 .. 1 + count.
    grid.fill_x(1, 1 + upper, top, fill_value);
    grid.fill_x(1, 1 + bottom, top + 1, fill_value);
}

/// A history graph (CPU or GPU) with a memory meter above it: the meter in the first two rows,
/// a line under it, and the graph in the rows left.
/// `history` is the usage history (0.0-1.0 per second, None where unknown), `memory` the memory meter's
/// fraction, and `bars_from` / `newest` the graph's options (see `drawing::draw_history_graph_with`).
pub fn draw_graph_with_meter(
    grid: &mut Grid,
    history: &[Option<f64>],
    memory: f64,
    fill_value: i32,
    y: usize,
    bars_from: &str,
    newest: &str,
) {
    let rows = inside_rows(y);
    draw_two_row_meter(grid, memory, fill_value, rows[0]);
    // The graph below the line under the meter
    // Rows 0-1 of the inside are the meter, row 2 the line drawn by the border, the rest the graph.
    let graph_rows = &rows[3..];
    // Draw the full-height graph on a scratch grid, then copy only as many rows as fit below the
    // meter. The graph is cropped, not squashed: the rows with the newest seconds are kept (the
    // bottom ones when "newest" is "bottom", otherwise the top ones), so the oldest seconds drop off.
    let mut graph = Grid::new();
    draw_history_graph_with(&mut graph, history, fill_value, y, bars_from, newest);
    let source_rows = inside_rows(y);
    let keep = graph_rows.len();
    let from: Vec<usize> = if newest == "bottom" {
        source_rows[source_rows.len() - keep..].to_vec()
    } else {
        source_rows[..keep].to_vec()
    };
    for (&to, &from) in graph_rows.iter().zip(from.iter()) {
        for x in 1..8 {
            grid.v[x][to] = graph.v[x][from];
        }
    }
}

/// The border of a graph with a meter: the outline and a line under the meter.
/// The line under the meter is the half's 4th row (row 3 on the top half, 19 on the bottom).
pub fn draw_graph_with_meter_border(grid: &mut Grid, value: i32, y: usize) {
    crate::drawing::draw_1_x_2_vert_grid(grid, value, y, 3);
}

/// Bars growing up from `bottom`, 3 wide.
/// `top` and `bottom` are inclusive rows; `x` is the bar's left column. A thin wrapper so the calls
/// below read the same as in Python.
fn short_bar(grid: &mut Grid, ratio: f64, value: i32, x: usize, top: usize, bottom: usize) {
    crate::drawing::draw_short_bar(grid, ratio, value, x, top, bottom);
}

/// Where the parts of the network + disk half are: (network rows, disk rows, space meter's top row).
/// All ranges are inclusive rows. The meter takes its top row and the one under it.
fn net_disk_rows(y: usize) -> (std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>, usize) {
    if y == 0 {
        // 15 rows: 5 network, line, 5 disk, line, 2 space, 1 spare at the bottom
        (1..=5, 7..=11, 13)
    } else {
        // 16 rows: 6 network, line, 6 disk, line, 2 space
        (17..=22, 24..=29, 31)
    }
}

/// Network (upload, download) on top, disk (read, write) under it, and how full a drive is at
/// the bottom.
/// `net` is (upload, download) and `disk` (read, write), each 0.0-1.0 of the highest rate seen
/// (see `rates.rs`). `space` is how full the chosen drive is, 0.0-1.0; None draws no meter (no drive
/// set, or it could not be read). Each pair is drawn as a left bar (columns 1-3) and a right bar (5-7).
pub fn draw_net_disk_space(
    grid: &mut Grid,
    net: (f64, f64),
    disk: (f64, f64),
    space: Option<f64>,
    fill_value: i32,
    y: usize,
) {
    let (net_rows, disk_rows, space_top) = net_disk_rows(y);
    short_bar(grid, net.0, fill_value, 1, *net_rows.start(), *net_rows.end());
    short_bar(grid, net.1, fill_value, 5, *net_rows.start(), *net_rows.end());
    short_bar(grid, disk.0, fill_value, 1, *disk_rows.start(), *disk_rows.end());
    short_bar(grid, disk.1, fill_value, 5, *disk_rows.start(), *disk_rows.end());
    if let Some(space) = space {
        draw_two_row_meter(grid, space, fill_value, space_top);
    }
}

/// The outline and lines of the network + disk half: the half's outline, a line under the network
/// bars, a line under the disk bars, and a line down the middle between the left and right bars.
pub fn draw_net_disk_space_border(grid: &mut Grid, value: i32, y: usize) {
    let (net_rows, disk_rows, space_top) = net_disk_rows(y);
    crate::drawing::draw_half_outline(grid, value, y);
    let line_a = net_rows.end() + 1;
    let line_b = disk_rows.end() + 1;
    for x in 0..WIDTH {
        grid.set(x, line_a, value);
        grid.set(x, line_b, value);
    }
    // The line between the two bars, down to the space meter
    // fill_y's end row is exclusive, but `line_b` is set by the loop above anyway.
    grid.fill_y(4, y as i64, line_b as i64, value);
    // The meter has no border of its own; the variable is only unpacked for symmetry with the
    // drawing function.
    let _ = space_top;
}

// The battery --------------------------------------------------------------------------------------

/// How the battery shows that it is charging.
/// The battery app's "look" setting; an unknown look draws as "bolt". Each look has a charging and an
/// on-battery version so you can see which it is at a glance.
pub const CHARGE_LOOKS: [&str; 6] = ["bolt", "sweep", "bubbles", "pour", "wave", "orbit"];

/// The battery's parts for the half at `y`: (nub row, body's top row, body's bottom row).
/// The nub (the battery's positive terminal) is columns 3-5 of its row. The body's top and bottom rows
/// are full-width lines; its sides are columns 0 and 8.
fn battery_rows(y: usize) -> (usize, usize, usize) {
    // On the bottom half the nub sits right under the line above and the body reaches the
    // panel's last row
    if y == 0 {
        (0, 1, 15)
    } else {
        (17, 18, 33)
    }
}

/// A 5 x 5 arrow pointing down, shown on battery ("#" is a pixel). Carved into the battery by `carve`.
const ARROW_DOWN: [&str; 5] = ["..#..", "..#..", "#.#.#", ".###.", "..#.."];
/// A 5 x 7 lightning bolt, shown while charging ("#" is a pixel). Carved into the battery by `carve`.
const BOLT: [&str; 7] = ["...##", "..##.", ".##..", "#####", "..##.", ".##..", "##..."];

/// Draws `shape` centred in the battery, lit where the battery is empty and dark where it is
/// filled, at `value`.
/// Each "#" pixel turns a filled (lit) pixel dark (`value` subtracted, floored at 0) and an empty
/// (dark) pixel lit at `empty_value`, so the shape shows on both the charge and the empty part.
/// `top` is the shape's top row; it is centred across the 7 inside columns.
fn carve(grid: &mut Grid, shape: &[&str], top: usize, value: i32, empty_value: i32) {
    let left = 1 + (7 - shape[0].len()) / 2;
    for (r, line) in shape.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if ch == '#' {
                let (x, yy) = (left + c, top + r);
                grid.v[x][yy] = if grid.v[x][yy] > 0 { (grid.v[x][yy] - value).max(0) } else { empty_value };
            }
        }
    }
}

/// Just the battery's outline and nub (an empty battery, for a computer without one).
/// The outline is drawn at `outline_value` (the dimmer border brightness).
pub fn draw_battery_outline(grid: &mut Grid, outline_value: i32, y: usize) {
    let (nub, body_top, body_bottom) = battery_rows(y);
    for x in 3..6 {
        grid.set(x, nub, outline_value);
    }
    for x in 0..WIDTH {
        grid.set(x, body_top, outline_value);
        grid.set(x, body_bottom, outline_value);
    }
    // The two sides; fill_y's end is exclusive, and the bottom row was already set above.
    grid.fill_y(0, body_top as i64, body_bottom as i64, outline_value);
    grid.fill_y(8, body_top as i64, body_bottom as i64, outline_value);
}

/// A battery shaped like a battery: a nub on top, an outline, and filled from the bottom. The
/// `look` (one of [`CHARGE_LOOKS`]) moves one way while charging and the other way on battery.
/// A low battery (7 % or less) flashes while not charging. `now` is in seconds.
/// `ratio` is the charge 0.0-1.0 (clamped) and `plugged` whether it is on the charger. The charge is
/// drawn at `fill_value` and the outline at `outline_value`.
#[allow(clippy::too_many_arguments)]
pub fn draw_battery_shape(
    grid: &mut Grid,
    ratio: f64,
    plugged: bool,
    look: &str,
    fill_value: i32,
    outline_value: i32,
    y: usize,
    now: f64,
) {
    let (nub, body_top, body_bottom) = battery_rows(y);
    draw_battery_outline(grid, outline_value, y);

    // The inside is columns 1-7 between the body's top and bottom lines: 13 rows on the top half
    // and 14 on the bottom half.
    let inside_top = body_top + 1;
    let inside_bottom = body_bottom - 1; // the lowest row inside
    let rows = inside_bottom - inside_top + 1; // 14
    let ratio = ratio.clamp(0.0, 1.0);
    // Whole rows are fully lit; the next row up is lit in proportion to the part of a row left.
    let level = ratio * rows as f64; // rows filled, with a part of a row
    let full_rows = level.floor() as usize;
    let part = level - full_rows as f64;

    // Low and not charging: for the first second of every two, show only the outline.
    if ratio <= 0.07 && !plugged && now.rem_euclid(2.0) < 1.0 {
        return; // flashing
    }
    let row_at = |i: usize| inside_bottom - i; // i rows up from the bottom
    for i in 0..full_rows.min(rows) {
        grid.fill_x(1, 8, row_at(i), fill_value);
    }
    if full_rows < rows && part > 0.0 {
        grid.fill_x(1, 8, row_at(full_rows), int_round(fill_value as f64 * part) as i32);
    }
    // Two shades the looks use: `dim` (45% of the fill) for faint light, `dark` (12%) for a
    // notch cut into the charge that should still be slightly visible.
    let dim = int_round(fill_value as f64 * 0.45) as i32;
    let dark = int_round(fill_value as f64 * 0.12) as i32;
    let filled = full_rows.min(rows);
    // Next, gather the positions so each look below can work from them; the look draws on top of the fill.
    let top_of_fill = inside_bottom + 1 - filled; // first filled row (inside_bottom + 1 when empty)
    let b = Battery { nub, inside_top, inside_bottom, filled, top_of_fill, fill_value, dim, dark, outline_value };
    match look {
        "sweep" => b.sweep(grid, plugged, now),
        "bubbles" => b.bubbles(grid, plugged, now),
        "pour" => b.pour(grid, plugged, now),
        "wave" => b.wave(grid, plugged, now),
        "orbit" => b.orbit(grid, plugged, now),
        _ => b.bolt(grid, plugged, now),
    }
}

/// Where the battery's parts are, for the looks.
/// Built fresh for each frame by `draw_battery_shape`. All rows are panel rows (0-33).
struct Battery {
    /// The nub's row.
    nub: usize,
    /// The top row inside the outline.
    inside_top: usize,
    /// The bottom row inside the outline (the fill starts here).
    inside_bottom: usize,
    /// Whole rows filled
    filled: usize,
    /// The highest fully filled row; `inside_bottom + 1` when no row is full. The row above it holds
    /// the part-filled row, if any.
    top_of_fill: usize,
    /// Brightness of the charge (0-255).
    fill_value: i32,
    /// 45% of `fill_value`.
    dim: i32,
    /// 12% of `fill_value`.
    dark: i32,
    /// Brightness of the outline.
    outline_value: i32,
}

impl Battery {
    /// How many rows are inside the outline (13 or 14).
    fn rows(&self) -> usize {
        self.inside_bottom - self.inside_top + 1
    }

    /// Charging: a pulsing bolt. On battery: a pulsing down arrow.
    fn bolt(&self, grid: &mut Grid, plugged: bool, now: f64) {
        // Brightness between 55% and 100%; |sin| repeats every 1.5 seconds.
        let pulse = 0.55 + 0.45 * (now * std::f64::consts::PI / 1.5).sin().abs();
        let v = int_round(self.fill_value as f64 * pulse) as i32;
        let shape: &[&str] = if plugged { &BOLT } else { &ARROW_DOWN };
        // Centre the shape top to bottom inside the battery.
        let top = self.inside_top + (self.rows() - shape.len()) / 2;
        carve(grid, shape, top, self.fill_value, v);
    }

    /// Charging: a dim band climbs from the fill to the top. On battery: a dark band falls
    /// through the fill from its top to the bottom.
    fn sweep(&self, grid: &mut Grid, plugged: bool, now: f64) {
        if plugged {
            // Rows above the full rows (including a part-filled one).
            let empty = self.top_of_fill - self.inside_top;
            if empty == 0 {
                return;
            }
            // One step every 0.12 s. The band grows one row per step, then 4 steps of rest. Rows
            // further up get brighter (35% to 100% of `dim`); in the rest steps nothing is drawn.
            let t = (now / 0.12) as usize % (empty + 4);
            for k in 0..=t.min(empty - 1) {
                let row = self.top_of_fill - 1 - k;
                let fade = if t >= empty { 0.0 } else { 0.35 + 0.65 * (k as f64 / empty as f64) };
                let v = int_round(self.dim as f64 * fade) as i32;
                // Never dim a row that is already brighter (the part-filled row).
                if grid.v[1][row] < v {
                    grid.fill_x(1, 8, row, v);
                }
            }
        } else {
            if self.filled == 0 {
                return;
            }
            // A dark row moves down one row per step with a dim row behind it, then 4 steps rest.
            let t = (now / 0.12) as usize % (self.filled + 4);
            if t < self.filled {
                let row = self.top_of_fill + t;
                grid.fill_x(1, 8, row, self.dark);
                if t > 0 {
                    grid.fill_x(1, 8, row - 1, self.dim);
                }
            }
        }
    }

    /// Charging: dark bubbles rise through the fill and pop at the top. On battery: they sink
    /// from the top of the fill to the bottom.
    fn bubbles(&self, grid: &mut Grid, plugged: bool, now: f64) {
        for n in 0..4usize {
            // Four bubbles in different columns at 4 to about 8 rows a second. `travel` is the fill
            // height plus 2 rows (for the pop and a pause), and the 0.37 offsets keep them apart.
            let speed = 4.0 + n as f64 * 1.3;
            let x = [2usize, 5, 3, 6][n];
            let travel = self.filled.max(1) as f64 + 2.0;
            let p = (now * speed / travel + n as f64 * 0.37).fract() * travel;
            let i = p.floor() as usize; // rows up from the bottom while rising
            if plugged {
                if i < self.filled {
                    grid.v[x][self.inside_bottom - i] = self.dark;
                } else if i == self.filled && self.filled < self.rows() {
                    grid.v[x][self.inside_bottom - i] = self.dim; // the pop
                }
            // On battery: first a dim drop just above the charge, then a dark bubble sinking
            // through it one row per step.
            } else if i == 0 && self.top_of_fill > self.inside_top {
                grid.v[x][self.top_of_fill - 1] = self.dim; // a drop going in
            } else if i >= 1 && i <= self.filled {
                grid.v[x][self.top_of_fill + i - 1] = self.dark;
            }
        }
    }

    /// Charging: drops fall in through the nub onto the fill. On battery: drops leave the fill
    /// and float up and out through the nub.
    fn pour(&self, grid: &mut Grid, plugged: bool, now: f64) {
        // Drops fall down column 4 from the nub to the row just above the charge (or the top
        // inside row when full). Two drops half a fall apart, at 14 rows a second.
        let landing = self.top_of_fill.saturating_sub(1).max(self.inside_top);
        let fall = landing - self.nub + 1;
        for d in 0..2usize {
            let t = now * 14.0 + d as f64 * fall as f64 / 2.0;
            let k = t as usize % fall;
            if plugged {
                let row = self.nub + k;
                grid.set(4, row, self.fill_value);
                if row > self.nub {
                    grid.set(4, row - 1, self.dim);
                }
            } else {
                let row = landing - k;
                grid.set(4, row, self.fill_value);
                if row < landing {
                    grid.set(4, row + 1, self.dim);
                }
                // For the first two steps of each drop, a dark notch at the top of the charge.
                if self.filled > 0 && k < 2 {
                    grid.set(4, self.top_of_fill, self.dark); // the gap it left
                }
            }
        }
    }

    /// The top of the fill ripples. Charging: waves roll to the right and splash above it. On
    /// battery: slower waves roll to the left and dip into it.
    fn wave(&self, grid: &mut Grid, plugged: bool, now: f64) {
        if self.filled == 0 {
            return;
        }
        for x in 1..8 {
            // A sine wave across the 7 columns (0.9 radians per column). Minus time makes the
            // crests move right, plus time moves them left.
            let phase = if plugged { x as f64 * 0.9 - now * 6.0 } else { x as f64 * 0.9 + now * 3.5 };
            let h = phase.sin();
            if plugged {
                if h > 0.2 && self.top_of_fill > self.inside_top {
                    // A splash in the empty row above the charge, 44% to 100% bright.
                    let v = int_round(self.fill_value as f64 * (0.3 + 0.7 * h)) as i32;
                    grid.v[x][self.top_of_fill - 1] = grid.v[x][self.top_of_fill - 1].max(v);
                }
            } else if h < -0.35 {
                // A dip: the charge's top row drops to 25%-48% brightness, and in the deepest
                // part the row under it to `dim` as well.
                grid.v[x][self.top_of_fill] = int_round(self.fill_value as f64 * (0.6 + 0.35 * h)) as i32;
                if h < -0.85 && self.filled > 1 {
                    grid.v[x][self.top_of_fill + 1] = self.dim;
                }
            }
        }
    }

    /// A bright spark runs along the outline, lighting up the edge of the inside next to it (or
    /// carving a dark notch into the charge). Charging: up both sides into the nub, which then
    /// flashes. On battery: the nub flashes, then sparks run out of it and down both sides.
    fn orbit(&self, grid: &mut Grid, plugged: bool, now: f64) {
        // The outline rows above and below the inside.
        let body_top = self.inside_top - 1;
        let body_bottom = self.inside_bottom + 1;
        // The left half of the path, from the bottom corner up to the nub; the right half mirrors it
        let mut path: Vec<(usize, usize)> = (body_top..=body_bottom).rev().map(|y| (0, y)).collect();
        path.extend([(1, body_top), (2, body_top), (3, body_top), (3, self.nub), (4, self.nub)]);
        let n = path.len();
        const TAIL: [f64; 5] = [1.0, 0.85, 0.7, 0.55, 0.4];
        // One step is 1/12 s. A cycle runs the whole path, lets the 5-pixel tail leave it, and
        // keeps the nub lit for 4 steps.
        let flash = 4; // steps the nub stays lit
        let cycle = n + TAIL.len() + flash;
        let step = (now * 12.0) as usize % cycle;
        // Charging runs the path forwards then flashes; on battery flashes, then runs backwards
        let (pos, nub_lit) =
            if plugged { (step, step >= n && step < n + flash) } else { (step.wrapping_sub(flash), step < flash) };
        // Dim the outline a little so the spark stands out
        for &(x, y) in &path {
            for xx in [x, 8 - x] {
                grid.v[xx][y] = int_round(self.outline_value as f64 * 0.6) as i32;
            }
        }
        if nub_lit {
            for x in 3..6 {
                grid.v[x][self.nub] = self.fill_value;
            }
        }
        if !plugged && step < flash {
            return;
        }
        // The spark's head is at `pos`, each tail pixel one step behind and dimmer. On battery
        // the path is walked backwards, from the nub out to the bottom corners.
        for (tail, share) in TAIL.iter().enumerate() {
            let Some(p) = pos.checked_sub(tail) else { continue };
            if p >= n {
                continue;
            }
            let i = if plugged { p } else { n - 1 - p };
            let v = int_round(self.fill_value as f64 * share) as i32;
            let (x, y) = path[i];
            for (xx, inner) in [(x, x + 1), (8 - x, 7 - x)] {
                grid.v[xx][y] = grid.v[xx][y].max(v);
                // The pixel just inside the outline, on the sides
                if x == 0 && y > body_top && y < body_bottom {
                    let here = grid.v[inner][y];
                    grid.v[inner][y] = if here >= self.dim { self.dark } else { int_round(v as f64 * 0.6) as i32 };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many of the inside columns (1-7) of row `y` are lit.
    fn lit(grid: &Grid, y: usize) -> usize {
        (1..8).filter(|&x| grid.v[x][y] > 0).count()
    }

    /// On the bottom half: a half-full meter lights 7 of its 14 pixels, the line under it stays dark,
    /// and a full history fills every graph row below.
    #[test]
    fn graph_with_meter_keeps_the_meter_on_top() {
        let history: Vec<Option<f64>> = (0..16).map(|_| Some(1.0)).collect();
        let mut g = Grid::new();
        draw_graph_with_meter(&mut g, &history, 0.5, 160, 16, "", "");
        // 7 of 14 meter pixels: 3 on the first row and 4 on the second
        assert_eq!(lit(&g, 17) + lit(&g, 18), 7);
        assert_eq!(lit(&g, 19), 0); // the line under the meter
        assert!((20..33).all(|y| lit(&g, y) == 7)); // the graph, full
    }

    /// Each bar lands in its own rows and column, and a full drive lights all 14 meter pixels.
    #[test]
    fn net_disk_and_space_rows() {
        let mut g = Grid::new();
        draw_net_disk_space(&mut g, (1.0, 0.0), (0.0, 1.0), Some(1.0), 160, 16);
        // Upload full (left), download empty; disk read empty, write full (right)
        assert!((17..=22).all(|y| g.v[2][y] > 0 && g.v[6][y] == 0));
        assert!((24..=29).all(|y| g.v[2][y] == 0 && g.v[6][y] > 0));
        assert_eq!(lit(&g, 31) + lit(&g, 32), 14);
    }

    /// Every look, charging or not, at many moments: the fill reaches the bottom, the outline's bottom
    /// is the panel's last row, the nub is drawn on the top half, and nothing spills into the other half.
    #[test]
    fn battery_fills_from_the_bottom_and_every_look_draws() {
        for look in CHARGE_LOOKS {
            for plugged in [true, false] {
                for t in 0..40 {
                    let mut g = Grid::new();
                    draw_battery_shape(&mut g, 0.5, plugged, look, 160, 70, 16, t as f64 * 0.1);
                    assert!(g.v[1][32] > 0, "{look}: the bottom row is filled");
                    assert!((0..WIDTH).all(|x| g.v[x][33] > 0), "{look}: the outline's bottom is the panel's last row");
                    let mut top = Grid::new();
                    draw_battery_shape(&mut top, 0.5, plugged, look, 160, 70, 0, t as f64 * 0.1);
                    assert!(top.v[4][0] > 0, "{look}: the nub on top");
                }
            }
            // Empty and full don't go out of the battery
            for ratio in [0.0, 1.0] {
                let mut g = Grid::new();
                draw_battery_shape(&mut g, ratio, true, look, 160, 70, 16, 1.3);
                assert!((0..17).all(|y| (0..WIDTH).all(|x| g.v[x][y] == 0)), "{look} stays in its half");
            }
        }
    }

    /// At 5% and on battery the picture differs between the two seconds of the flash, and in the "off"
    /// second the inside is dark.
    #[test]
    fn low_battery_flashes_when_not_charging() {
        let mut on = Grid::new();
        draw_battery_shape(&mut on, 0.05, false, "bolt", 160, 70, 16, 1.5);
        let mut off = Grid::new();
        draw_battery_shape(&mut off, 0.05, false, "bolt", 160, 70, 16, 0.5);
        assert_ne!(on, off);
        assert!((19..33).all(|y| lit(&off, y) == 0));
    }
}
