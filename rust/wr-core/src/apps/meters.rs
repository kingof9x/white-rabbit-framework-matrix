//! The new monitor halves: a graph with a memory meter on top (CPU + RAM, GPU + GPU memory),
//! network over disk with a drive-space meter, and a battery shaped like a battery.
//!
//! `y` is where the half panel starts: 0 for the top half, 16 for the bottom half.

use crate::drawing::draw_history_graph_with;
use crate::grid::{Grid, WIDTH};
use crate::pyround::int_round;

/// The rows inside a half's border: 1-15 on the top half, 17-32 on the bottom half.
fn inside_rows(y: usize) -> Vec<usize> {
    if y == 0 {
        (1..16).collect()
    } else {
        (17..33).collect()
    }
}

/// A meter two rows tall and 7 wide, filled left to right, the top row first (as the memory
/// meter): 14 steps.
pub fn draw_two_row_meter(grid: &mut Grid, ratio: f64, fill_value: i32, top: usize) {
    let lit = 7.0 * 2.0 * ratio.clamp(0.0, 1.0);
    let bottom = int_round(lit / 2.0);
    let upper = int_round((lit - 0.49) / 2.0);
    grid.fill_x(1, 1 + upper, top, fill_value);
    grid.fill_x(1, 1 + bottom, top + 1, fill_value);
}

/// A history graph (CPU or GPU) with a memory meter above it: the meter in the first two rows,
/// a line under it, and the graph in the rows left.
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
    let graph_rows = &rows[3..];
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
pub fn draw_graph_with_meter_border(grid: &mut Grid, value: i32, y: usize) {
    crate::drawing::draw_1_x_2_vert_grid(grid, value, y, 3);
}

/// Bars growing up from `bottom`, 3 wide.
fn short_bar(grid: &mut Grid, ratio: f64, value: i32, x: usize, top: usize, bottom: usize) {
    crate::drawing::draw_short_bar(grid, ratio, value, x, top, bottom);
}

/// Where the parts of the network + disk half are: (network rows, disk rows, space meter's top row).
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
    grid.fill_y(4, y as i64, line_b as i64, value);
    let _ = space_top;
}

// The battery --------------------------------------------------------------------------------------

/// How the battery shows that it is charging.
pub const CHARGE_LOOKS: [&str; 4] = ["bolt", "sweep", "bubbles", "pour"];

/// The battery's parts for the half at `y`: (nub row, body's top row, body's bottom row).
fn battery_rows(y: usize) -> (usize, usize, usize) {
    // A dark row between the nub and the line above, so the nub stands out
    if y == 0 {
        (0, 1, 15)
    } else {
        (18, 19, 33)
    }
}

const ARROW_DOWN: [&str; 5] = ["..#..", "..#..", "#.#.#", ".###.", "..#.."];
const BOLT: [&str; 7] = ["...##", "..##.", ".##..", "#####", "..##.", ".##..", "##..."];

/// Draws `shape` centred in the battery, lit where the battery is empty and dark where it is
/// filled, at `value`.
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

/// A battery shaped like a battery: a nub on top, an outline, and filled from the bottom. While it
/// runs on battery a down arrow shows; while charging the `look` (one of [`CHARGE_LOOKS`]) moves.
/// A low battery (7 % or less) flashes while not charging. `now` is in seconds.
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
    // The outline and the nub
    for x in 3..6 {
        grid.set(x, nub, outline_value);
    }
    for x in 0..WIDTH {
        grid.set(x, body_top, outline_value);
        grid.set(x, body_bottom, outline_value);
    }
    grid.fill_y(0, body_top as i64, body_bottom as i64, outline_value);
    grid.fill_y(8, body_top as i64, body_bottom as i64, outline_value);

    let inside_top = body_top + 1;
    let inside_bottom = body_bottom - 1; // the lowest row inside
    let rows = inside_bottom - inside_top + 1; // 14
    let ratio = ratio.clamp(0.0, 1.0);
    let level = ratio * rows as f64; // rows filled, with a part of a row
    let full_rows = level.floor() as usize;
    let part = level - full_rows as f64;

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
    let dim = int_round(fill_value as f64 * 0.45) as i32;
    let arrow_top = inside_top + (rows - ARROW_DOWN.len()) / 2;
    if !plugged {
        carve(grid, &ARROW_DOWN, arrow_top, fill_value, dim);
        return;
    }
    let top_of_fill = inside_bottom + 1 - full_rows.min(rows); // first filled row
    match look {
        "sweep" => {
            // A dim band climbs from the fill to the top, again and again
            let empty = top_of_fill.saturating_sub(inside_top);
            if empty > 0 {
                let t = (now / 0.12) as usize % (empty + 4);
                for k in 0..=t.min(empty - 1) {
                    let row = top_of_fill - 1 - k;
                    let fade = if t >= empty { 0.0 } else { 0.35 + 0.65 * (k as f64 / empty as f64) };
                    let v = int_round(dim as f64 * fade) as i32;
                    if grid.v[1][row] < v {
                        grid.fill_x(1, 8, row, v);
                    }
                }
            }
        }
        "bubbles" => {
            // Dark bubbles rise through the fill and pop at the top
            for b in 0..4usize {
                let speed = 4.0 + b as f64 * 1.3;
                let x = [2usize, 5, 3, 6][b];
                let travel = full_rows.max(1) as f64 + 2.0;
                let p = (now * speed / travel + b as f64 * 0.37).fract() * travel;
                let i = p.floor() as usize;
                if i < full_rows {
                    let row = row_at(i);
                    grid.v[x][row] = int_round(fill_value as f64 * 0.15) as i32;
                } else if i == full_rows && full_rows < rows {
                    // the pop, just above the fill
                    grid.v[x][row_at(i)] = dim;
                }
            }
        }
        "pour" => {
            // Drops fall in through the nub and land on the fill
            let landing = top_of_fill.saturating_sub(1).max(inside_top);
            let fall = landing - nub + 1;
            for d in 0..2usize {
                let t = now * 14.0 + d as f64 * fall as f64 / 2.0;
                let row = nub + (t as usize % fall);
                grid.set(4, row, fill_value);
                if row > nub {
                    grid.set(4, row - 1, dim);
                }
            }
        }
        _ => {
            // The bolt, gently pulsing
            let pulse = 0.55 + 0.45 * (now * std::f64::consts::PI / 1.5).sin().abs();
            let bolt_top = inside_top + (rows - BOLT.len()) / 2;
            let v = int_round(fill_value as f64 * pulse) as i32;
            carve(grid, &BOLT, bolt_top, fill_value, v);
        }
    }
}
