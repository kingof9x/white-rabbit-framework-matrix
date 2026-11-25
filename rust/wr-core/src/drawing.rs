//! Drawing the monitor's apps onto one panel, a port of `led_mon/drawing.py` and the plugins' drawing.
//!
//! `y` is where the half panel starts: 0 for the top half, 16 for the bottom half (the battery uses
//! 3 and 19, under the memory bars). Values are LED brightness, 0-255.
//!
//! Where it sits: `render.rs` decides which app shows in each half of a panel and calls these functions
//! (on the monitor's main loop, about ten times a second, and in the window for previews). Every function
//! only writes into the `Grid` it is given; nothing here keeps state, reads files or talks to a panel.
//! The ones that animate (battery flash, charging pulse) are handed the time instead of reading a clock.
//!
//! Layout of a half panel: row `y` and the row 16 (or 33) below are the border rows, columns 0 and 8 the
//! border columns, so the content sits in columns 1-7 and rows `y+1` up to 15 (top) or 32 (bottom). The
//! borders themselves are drawn separately (`draw_app_border`), at a different brightness.
//!
//! Exact pixel equality with Python matters (see `tests/golden.rs`), so rounding goes through `pyround`
//! and several odd-looking offsets (such as `- 0.49`) are copied from the Python code as they are.

use crate::grid::{clip, Grid, HEIGHT, WIDTH};
use crate::patterns::{self, Pattern};
use crate::pyround::{int, int_round};

/// The content rows of a half panel: rows 1-15 for the top half (`y` 0), rows `y+1` to 32 otherwise
/// (for `y` 16 that is 17-32). The border rows are left out.
fn half_rows(y: usize) -> std::ops::Range<usize> {
    if y == 0 {
        y + 1..16
    } else {
        y + 1..33
    }
}

/// Puts `pattern` (numpy shape rows x cols) at `grid[x0 + c, y0 + r]` for a pattern stored as
/// rows = y and cols = x, i.e. `grid.T[y0:y0+rows, x0:x0+cols] = pattern * value`.
///
/// Only the first `max_rows` pattern rows are drawn, so a character near the bottom is cut off rather
/// than written past the grid. Columns past the right edge are dropped by `Grid::set`.
fn put_transposed(grid: &mut Grid, p: &Pattern, x0: usize, y0: usize, value: i32, max_rows: usize) {
    for r in 0..p.rows.min(max_rows) {
        for c in 0..p.cols {
            grid.set(x0 + c, y0 + r, p.at(r, c) * value);
        }
    }
}

/// Puts `pattern` at `grid[x0 + r, y0 + c]`, i.e. `grid[x0:x0+rows, y0:y0+cols] = pattern * value`.
///
/// So the pattern's rows run left to right across the panel; used for the CPU spirals and app names,
/// which were stored that way in Python.
fn put(grid: &mut Grid, p: &Pattern, x0: usize, y0: usize, value: i32) {
    for r in 0..p.rows {
        for c in 0..p.cols {
            grid.set(x0 + r, y0 + c, p.at(r, c) * value);
        }
    }
}

// CPU -----------------------------------------------------------------------------------------------

/// Which of the ten spiral pictures (0 = empty, 9 = full) shows a core load of `fill_ratio` (0.0-1.0).
/// The range is cut into ten equal steps; the `9.999999` and `- 0.5` come from the Python code and
/// make 1.0 land on 9. Out-of-range input is clamped.
pub fn spiral_index(fill_ratio: f64) -> usize {
    int_round(fill_ratio * 9.999999 - 0.5).clamp(0, 9) as usize
}

/// A 3x3 spiral per core, two columns of four, for up to eight cores.
///
/// Core `i` goes in column `i % 2` (x 1 or 5) and row `i / 2` (4 rows apart), matching the 2 x 4 border
/// drawn by `draw_8_x_8_grid`. More than eight values would draw past the half (the caller switches to
/// bars before that).
pub fn draw_spiral_vals(grid: &mut Grid, cpu_values: &[f64], fill_value: i32, y: usize) {
    // Skip the border row at the top of the half.
    let y = y + 1;
    for (i, &v) in cpu_values.iter().enumerate() {
        let column = i % 2;
        let row = i / 2;
        let spiral = patterns::spiral(spiral_index(v));
        put(grid, &spiral, 1 + column * 4, y + row * 4, fill_value);
    }
}

/// A horizontal bar from column `start`, up to `width` long; a partly filled last pixel is dimmer.
///
/// `ratio` is 0.0-1.0 (clamped) and `fill_value` the brightness of a full pixel. For example a ratio of
/// 0.5 over 7 pixels lights 3 pixels fully and the 4th at half brightness, which makes the bar move
/// smoothly instead of in whole-pixel steps.
pub fn draw_hbar(grid: &mut Grid, ratio: f64, fill_value: i32, row: usize, width: usize, start: usize) {
    let length = ratio.clamp(0.0, 1.0) * width as f64;
    let full = int(length) as usize;
    grid.fill_x(start as i64, (start + full) as i64, row, fill_value);
    if full < width {
        grid.set(start + full, row, int_round(fill_value as f64 * (length - full as f64)) as i32);
    }
}

/// One horizontal bar per core, for more cores than the eight spirals fit.
///
/// When there are more cores than rows, neighbouring cores are averaged onto one row (`per_row` cores
/// each), so every core still counts. Each bar is 7 pixels wide, from column 1.
pub fn draw_core_bars(grid: &mut Grid, core_values: &[f64], fill_value: i32, y: usize) {
    let rows: Vec<usize> = half_rows(y).collect();
    if core_values.is_empty() {
        return;
    }
    let per_row = core_values.len().div_ceil(rows.len());
    let values: Vec<f64> = core_values.chunks(per_row).map(|c| c.iter().sum::<f64>() / c.len() as f64).collect();
    for (row, v) in rows.into_iter().zip(values) {
        draw_hbar(grid, v, fill_value, row, 7, 1);
    }
}

/// The cpu app: a spiral per core up to eight cores, otherwise a bar per core.
///
/// `cpu_values` are the smoothed loads from `monitors::CpuSmoother`, 0.0-1.0 per core.
pub fn draw_cpu_vals(grid: &mut Grid, cpu_values: &[f64], fill_value: i32, y: usize) {
    if cpu_values.len() <= 8 {
        draw_spiral_vals(grid, cpu_values, fill_value, y);
    } else {
        draw_core_bars(grid, cpu_values, fill_value, y);
    }
}

/// A scrolling graph: one row per reading, newest at the top. `None` is an empty row.
///
/// `values` comes from `monitors::History` (newest first, 0.0-1.0). Each reading becomes a horizontal
/// bar on its own row, so the graph scrolls down one row per second. Extra values are ignored.
pub fn draw_history_graph(grid: &mut Grid, values: &[Option<f64>], fill_value: i32, y: usize) {
    for (row, v) in half_rows(y).zip(values.iter()) {
        if let Some(v) = v {
            draw_hbar(grid, *v, fill_value, row, 7, 1);
        }
    }
}

/// The history graph with its options: `bars_from` "right" grows the bars from the right edge
/// instead of the left, and `newest` "bottom" puts the newest second at the bottom so the graph
/// scrolls upwards instead of downwards.
pub fn draw_history_graph_with(
    grid: &mut Grid,
    values: &[Option<f64>],
    fill_value: i32,
    y: usize,
    bars_from: &str,
    newest: &str,
) {
    let rows: Vec<usize> = half_rows(y).collect();
    let n = rows.len().min(values.len());
    // With "bottom", take the last `n` rows and walk them upwards so the newest reading is at the bottom.
    let order: Vec<usize> =
        if newest == "bottom" { rows[rows.len() - n..].iter().rev().copied().collect() } else { rows[..n].to_vec() };
    for (&row, v) in order.iter().zip(values.iter()) {
        if let Some(v) = v {
            draw_hbar(grid, *v, fill_value, row, 7, 1);
            if bars_from == "right" {
                // (swap 1<->7, 2<->6, 3<->5; column 4 is the middle and stays)
                // Mirror columns 1..=7
                for x in 1..4 {
                    let (a, b) = (grid.v[x][row], grid.v[8 - x][row]);
                    grid.v[x][row] = b;
                    grid.v[8 - x][row] = a;
                }
            }
        }
    }
}

// Memory and battery ---------------------------------------------------------------------------------

/// Two rows of up to 7 pixels.
///
/// `memory_ratio` (0.0-1.0) of 14 pixels is lit, shared between rows `y+1` and `y+2`. The lower row is
/// rounded up first, so with an odd count it is one pixel ahead of the upper row (the `- 0.49` makes
/// the upper row round down at exactly half, as Python's code did).
pub fn draw_memory(grid: &mut Grid, memory_ratio: f64, fill_value: i32, y: usize) {
    let lit = 7.0 * 2.0 * memory_ratio;
    let bottom = int_round(lit / 2.0);
    let top = int_round((lit - 0.49) / 2.0);
    grid.fill_x(1, 1 + top, y + 1, fill_value);
    grid.fill_x(1, 1 + bottom, y + 2, fill_value);
}

/// The battery under the memory bars (`y` is 3 on the top half, 19 on the bottom). `now` is the time
/// in seconds (Python's `time.time()`): a low battery flashes, and the charging bolt pulses.
pub fn draw_battery(grid: &mut Grid, ratio: f64, plugged: bool, fill_value: i32, y: usize, now: f64) {
    // Below 7% and on battery the whole picture flashes: hidden for the first half of every 2 seconds.
    const LOW: f64 = 0.07;
    const FLASH_TIME: f64 = 2.0;
    // The bolt's brightness swings by +-10 with a sine of period 2*pi*3 seconds (about 19 s).
    const PULSE_TIME: f64 = 3.0;
    // `bot` is the row just below the battery, `num_rows` its height; each half has its own bolt
    // picture to fit. The `unwrap`s cannot fail: both keys are in the generated table.
    let (bot, num_rows, bolt) = if y == 19 {
        (33usize, 13usize, patterns::find(patterns::LIGHTNING_BOLT, "bottom").unwrap())
    } else {
        (16usize, 12usize, patterns::find(patterns::LIGHTNING_BOLT, "top").unwrap())
    };
    let bat_top = y + 1;
    // `lit` pixels in total, spread over the 7 columns: each gets `base` and the first `remainder`
    // columns one more, so the level rises evenly from the bottom one pixel at a time.
    let lit = int_round(num_rows as f64 * 7.0 * ratio);
    let base = lit.div_euclid(7);
    let remainder = lit.rem_euclid(7);
    if ratio <= LOW && !plugged && now.rem_euclid(FLASH_TIME) * 2.0 < FLASH_TIME {
        return;
    }
    for i in 0..7 {
        let mut pixels = base;
        if (i as i64) < remainder {
            pixels += 1;
        }
        grid.fill_y(i + 1, bot as i64 - pixels, bot as i64, fill_value);
    }
    // The bolt is drawn by subtracting about `fill_value` from its pixels and then taking the
    // absolute value: where the battery is lit the bolt goes almost dark, and where it is dark the
    // bolt lights up. So the bolt always shows against whatever is behind it. This is why `Grid`
    // holds signed values.
    if plugged {
        let pulse = (now / PULSE_TIME).sin();
        let minus = (fill_value as f64 + 10.0 * pulse).round_ties_even() as i32;
        for r in 0..bolt.rows {
            for c in 0..bolt.cols {
                if bolt.at(r, c) == 1 {
                    // Pattern rows run across (x 1-7) and columns down; this stays inside the grid.
                    let (x, yy) = (1 + r, bat_top + c);
                    grid.v[x][yy] -= minus;
                }
            }
        }
        for column in &mut grid.v[1..8] {
            for v in &mut column[bat_top..bat_top + num_rows] {
                *v = v.abs();
            }
        }
    }
}

// Bars -----------------------------------------------------------------------------------------------

/// A bar 3 wide and 16 tall at `bar_x_offset`: down from the top on the top half, up from the bottom
/// on the bottom half (`y` 16).
///
/// `ratio` (0.0-1.0) of 48 pixels (3 columns x 16 rows) is lit, spread like the battery so the bar rises
/// one pixel at a time. Values outside 0-1 are not clamped here; the slice is clipped to the grid.
pub fn draw_bar(grid: &mut Grid, ratio: f64, value: i32, bar_x_offset: usize, y: usize) {
    let lit = int_round(16.0 * 3.0 * ratio);
    let base = lit.div_euclid(3);
    let remainder = lit.rem_euclid(3);
    for i in 0..3 {
        let mut pixels = base;
        if (i as i64) < remainder {
            pixels += 1;
        }
        if y == 16 {
            grid.fill_y(bar_x_offset + i, 33 - pixels, 33, value);
        } else {
            grid.fill_y(bar_x_offset + i, 1, 1 + pixels, value);
        }
    }
}

/// Two short bars growing up from `bottom`, 3 wide at `bar_x_offset`.
/// `top` and `bottom` are inclusive rows; `ratio` is clamped to 0.0-1.0. Used for disk and network.
pub fn draw_short_bar(grid: &mut Grid, ratio: f64, value: i32, bar_x_offset: usize, top: usize, bottom: usize) {
    let height = (bottom - top + 1) as f64;
    let lit = int_round(height * 3.0 * ratio.clamp(0.0, 1.0));
    let base = lit.div_euclid(3);
    let remainder = lit.rem_euclid(3);
    for i in 0..3 {
        let mut pixels = base;
        if (i as i64) < remainder {
            pixels += 1;
        }
        if pixels > 0 {
            grid.fill_y(bar_x_offset + i, bottom as i64 + 1 - pixels, bottom as i64 + 1, value);
        }
    }
}

/// Disk (read, write) on the upper part of the half panel, network (upload, download) below it.
///
/// Each value is 0.0-1.0 of the bar's height (the caller scales bytes per second). Read and upload are
/// on the left (columns 1-3), write and download on the right (columns 5-7). The disk bars take the
/// first 7 content rows, the row after them is the border drawn by `draw_disk_net_border`, and the
/// network bars take the rest.
pub fn draw_disk_net(grid: &mut Grid, disk: (f64, f64), net: (f64, f64), fill_value: i32, y: usize) {
    let rows: Vec<usize> = half_rows(y).collect();
    let (disk_top, disk_bottom) = (rows[0], rows[6]);
    let (net_top, net_bottom) = (rows[8], *rows.last().unwrap());
    draw_short_bar(grid, disk.0, fill_value, 1, disk_top, disk_bottom);
    draw_short_bar(grid, disk.1, fill_value, 5, disk_top, disk_bottom);
    draw_short_bar(grid, net.0, fill_value, 1, net_top, net_bottom);
    draw_short_bar(grid, net.1, fill_value, 5, net_top, net_bottom);
}

// FPS ------------------------------------------------------------------------------------------------

/// The digits of an FPS number and their columns: (character, column, dimmer).
///
/// The panel is only 9 LEDs wide and a digit is 3 wide, so the layout depends on the size of the number:
/// 200-999 uses three touching digits with the middle one dimmer to tell them apart; 100-199 draws the
/// leading 1 as a single line (`'|'`) to save space; 10-99 and 0-9 are centred. The value is rounded
/// and clamped to 0-999, so 1500 FPS shows as 999.
pub fn fps_number_columns(fps: f64) -> Vec<(char, usize, bool)> {
    let value = int_round(fps).clamp(0, 999);
    let digit = |n: i64| char::from(b'0' + n as u8);
    if value >= 200 {
        vec![(digit(value / 100), 0, false), (digit(value / 10 % 10), 3, true), (digit(value % 10), 6, false)]
    } else if value >= 100 {
        vec![('|', 0, false), (digit(value / 10 % 10), 2, false), (digit(value % 10), 6, false)]
    } else if value >= 10 {
        vec![(digit(value / 10), 1, false), (digit(value % 10), 5, false)]
    } else {
        vec![(digit(value), 3, false)]
    }
}

/// The FPS in rows `top` to `top + 9`: 3x5 digits drawn twice as tall. "?" when it can't be measured.
///
/// Each 5-row digit row is drawn twice (`j / 2`), giving 10 rows. The dimmer digit is at 55%.
pub fn draw_fps_number(grid: &mut Grid, fps: f64, available: bool, fill_value: i32, top: usize) {
    if !available {
        // The unwraps on table lookups in this file cannot fail: the keys are in the generated tables.
        let q = patterns::find(patterns::SYMBOLS, "?").unwrap();
        for r in 0..q.rows.min(6) {
            for c in 0..q.cols {
                grid.set(c, top + 2 + r, q.at(r, c) * fill_value);
            }
        }
        return;
    }
    for (ch, x, dim) in fps_number_columns(fps) {
        let value = if dim { int_round(fill_value as f64 * 0.55) as i32 } else { fill_value };
        if ch == '|' {
            grid.fill_y(x, top as i64, top as i64 + 10, value);
        } else {
            let d = patterns::find(patterns::DIGITS_3X5, &ch.to_string()).unwrap();
            for j in 0..10 {
                for i in 0..3 {
                    grid.set(x + i, top + j, d.at(j / 2, i) * value);
                }
            }
        }
    }
}

/// The possible tops of the FPS histogram, in frames per second (common refresh rates).
pub const FPS_SCALES: [f64; 10] = [30.0, 60.0, 90.0, 120.0, 144.0, 165.0, 240.0, 360.0, 500.0, 1000.0];

/// The histogram's highest FPS: the first scale at or above the highest reading.
///
/// Zero readings (no game running) are ignored, so a quiet history uses the smallest scale (30). Above
/// 1000 the scale stays 1000 and the bars are capped.
pub fn fps_scale(history: &[f64]) -> f64 {
    let top = history.iter().copied().filter(|v| *v != 0.0).fold(1.0f64, f64::max);
    FPS_SCALES.iter().copied().find(|s| *s >= top).unwrap_or(1000.0)
}

/// The fps app on the whole panel: the number on top, and a histogram of the last 9 seconds below it.
///
/// `history` is newest first, one value per second; the newest is the rightmost column. Each column is a
/// bar up from the bottom over the 16 rows from row 18, with a dimmer top pixel for a part-filled row.
/// A zero reading leaves its column empty.
pub fn draw_fps(grid: &mut Grid, fps: f64, history: &[f64], available: bool, fill_value: i32) {
    const HISTOGRAM_TOP: usize = 18;
    const COLUMNS: usize = 9;
    draw_fps_number(grid, fps, available, fill_value, 3);
    let scale = fps_scale(history);
    let height = (HEIGHT - HISTOGRAM_TOP) as f64;
    for (i, &v) in history.iter().take(COLUMNS).enumerate() {
        if v == 0.0 {
            continue;
        }
        let x = COLUMNS - 1 - i;
        let length = (v / scale).clamp(0.0, 1.0) * height;
        let full = int(length);
        let bottom = HEIGHT as i64;
        grid.fill_y(x, bottom - full, bottom, fill_value);
        if (full as f64) < height && length > full as f64 {
            let y = bottom - full - 1;
            grid.set(x, y as usize, int_round(fill_value as f64 * (length - full as f64)) as i32);
        }
    }
}

/// The fps-num app: just the number, in the top or bottom half.
pub fn draw_fps_half(grid: &mut Grid, fps: f64, available: bool, fill_value: i32, y: usize) {
    draw_fps_number(grid, fps, available, fill_value, if y == 0 { 3 } else { 20 });
}

// Pictures, characters, temperatures -----------------------------------------------------------------

/// A picture from a snapshot file: 34 rows of 9 values (usually 0 or 1).
///
/// Each value is scaled by `fill_value` and truncated, so values between 0 and 1 give dimmer pixels.
/// Missing or extra rows and columns are ignored.
pub fn draw_snapshot(grid: &mut Grid, rows: &[Vec<f64>], fill_value: i32) {
    for (y, row) in rows.iter().enumerate().take(HEIGHT) {
        for (x, v) in row.iter().enumerate().take(WIDTH) {
            grid.set(x, y, int(v * fill_value as f64) as i32);
        }
    }
}

/// Characters (digits, symbols, weather icons) one under the other from row `y`, one row apart.
/// Unknown characters are skipped, as in Python.
///
/// Used by the clock and weather apps, which write text downwards because the panel is tall and narrow.
/// Each character is drawn across the full width from column 0; whatever falls below row 33 is cut off.
pub fn draw_chars_list(grid: &mut Grid, chars: &[&str], fill_value: i32, y: usize) {
    let mut y = y;
    for ch in chars {
        if let Some(p) = patterns::char_pattern(ch) {
            if y < HEIGHT {
                put_transposed(grid, &p, 0, y, fill_value, HEIGHT - y);
            }
            y += p.rows + 1;
        }
    }
}

/// The clock's "PM" dot in the bottom-right corner.
///
/// At column 7, rows 32-33 (at most two rows of the pattern are drawn).
pub fn draw_pm_indicator(grid: &mut Grid, fill_value: i32) {
    let pm = patterns::find(patterns::ICONS, "pm_indicator").unwrap();
    put_transposed(grid, &pm, 7, 32, fill_value, 2);
}

/// The 3x5 digits of a temperature and their columns; `None` draws "--".
///
/// Like `fps_number_columns`, but the value is rounded and clamped to 0-199 (degrees), so a reading
/// above 199 shows as 199. A missing reading (sensor not available) shows as two dashes.
pub fn temp_number_columns(value: Option<f64>) -> Vec<(char, usize)> {
    let Some(v) = value else { return vec![('-', 1), ('-', 5)] };
    let v = int_round(v).clamp(0, 199);
    let digit = |n: i64| char::from(b'0' + n as u8);
    if v >= 100 {
        vec![('|', 0), (digit(v / 10 % 10), 2), (digit(v % 10), 6)]
    } else if v >= 10 {
        vec![(digit(v / 10), 1), (digit(v % 10), 5)]
    } else {
        vec![(digit(v), 3)]
    }
}

/// Three temperatures one under the other in the top (y 0) or bottom (y 16) half; the middle one dimmer.
///
/// Each number is 5 rows tall at rows `tops[i]`. Only the first three values are used.
pub fn draw_temp_numbers(grid: &mut Grid, values: &[Option<f64>], fill_value: i32, y: usize) {
    let tops: [usize; 3] = if y == 0 { [0, 6, 11] } else { [17, 23, 29] };
    for (i, (top, value)) in tops.iter().zip(values.iter().take(3)).enumerate() {
        let brightness = if i != 1 { fill_value } else { int_round(fill_value as f64 * 0.55) as i32 };
        for (ch, x) in temp_number_columns(*value) {
            if ch == '|' {
                grid.fill_y(x, *top as i64, *top as i64 + 5, brightness);
            } else {
                let d = patterns::find(patterns::DIGITS_3X5, &ch.to_string()).unwrap();
                put_transposed(grid, &d, x, *top, brightness, 5);
            }
        }
    }
}

// App names ------------------------------------------------------------------------------------------

/// The names of the apps on the top and bottom half (shown with Ctrl+Alt+Shift+I).
///
/// `top` and `bottom` are app ids (for example "cpu"); their pictures in `ID_PATTERNS` are 7 x 15 and
/// are placed inside the border of each half. An unknown id, or one whose picture is the whole-panel
/// size, draws nothing for that half.
pub fn draw_ids(grid: &mut Grid, top: &str, bottom: &str, fill_value: i32) {
    if let Some(p) = patterns::find(patterns::ID_PATTERNS, top) {
        if p.rows == 7 && p.cols == 15 {
            put(grid, &p, 1, 1, fill_value);
        }
    }
    if let Some(p) = patterns::find(patterns::ID_PATTERNS, bottom) {
        if p.rows == 7 && p.cols == 15 {
            put(grid, &p, 1, 18, fill_value);
        }
    }
}

/// The name of an app that has the whole panel.
///
/// Only pictures of the full 9 x 34 size are drawn; a half-panel picture or an unknown id draws nothing.
pub fn draw_id(grid: &mut Grid, id: &str, fill_value: i32) {
    if let Some(p) = patterns::find(patterns::ID_PATTERNS, id) {
        if p.rows == WIDTH && p.cols == HEIGHT {
            put(grid, &p, 0, 0, fill_value);
        }
    }
}

// Borders --------------------------------------------------------------------------------------------

/// The distance from a half's top border row to its bottom border row: 16 (rows 0 and 16) for the top
/// half and 17 (rows 16 and 33) for the bottom half. The two halves share row 16.
fn half_height(y: usize) -> usize {
    if y == 0 {
        16
    } else {
        17
    }
}

/// Sets a whole row (all 9 columns) to `value`.
fn row(grid: &mut Grid, y: usize, value: i32) {
    for x in clip(0, WIDTH as i64, WIDTH) {
        grid.set(x, y, value);
    }
}

/// A rectangle around one half panel: its top and bottom rows and columns 0 and 8.
pub fn draw_half_outline(grid: &mut Grid, value: i32, y: usize) {
    let h = half_height(y);
    row(grid, y, value);
    row(grid, y + h, value);
    grid.fill_y(0, y as i64, (y + h) as i64, value);
    grid.fill_y(8, y as i64, (y + h) as i64, value);
}

/// Around eight spirals: a 2 x 4 grid.
///
/// The outline plus a line down column 4 and lines across at rows `y+4`, `y+8` and `y+12`, giving eight
/// 3 x 3 boxes for the spirals of `draw_spiral_vals`.
pub fn draw_8_x_8_grid(grid: &mut Grid, value: i32, y: usize) {
    draw_half_outline(grid, value, y);
    let h = half_height(y);
    grid.fill_y(4, y as i64, (y + h) as i64, value);
    for dy in [4, 8, 12] {
        row(grid, y + dy, value);
    }
}

/// Split into a left and a right part at column `x_split`.
pub fn draw_2_x_1_horiz_grid(grid: &mut Grid, value: i32, y: usize, x_split: usize) {
    draw_half_outline(grid, value, y);
    let h = half_height(y);
    grid.fill_y(x_split, y as i64, (y + h) as i64, value);
}

/// Split into an upper and a lower part at row `y + y_split`.
pub fn draw_1_x_2_vert_grid(grid: &mut Grid, value: i32, y: usize, y_split: usize) {
    draw_half_outline(grid, value, y);
    row(grid, y + y_split, value);
}

/// The cpu app's border: the eight-box grid for up to eight cores, otherwise only the outline (bars).
pub fn draw_cpu_border(grid: &mut Grid, value: i32, y: usize, core_count: usize) {
    if core_count <= 8 {
        draw_8_x_8_grid(grid, value, y);
    } else {
        draw_half_outline(grid, value, y);
    }
}

/// Left and right halves for the two bars of each pair, and a line at row `y+8` between disk and network.
pub fn draw_disk_net_border(grid: &mut Grid, value: i32, y: usize) {
    draw_2_x_1_horiz_grid(grid, value, y, 4);
    row(grid, y + 8, value);
}

/// Around the whole panel, split into two halves.
pub fn draw_outline_border(grid: &mut Grid, value: i32) {
    row(grid, 0, value);
    row(grid, 16, value);
    row(grid, 33, value);
    grid.fill_y(0, 0, HEIGHT as i64, value);
    grid.fill_y(8, 0, HEIGHT as i64, value);
}

/// The border an app gets (none for whole-panel apps and the number apps). `core_count` is for the cpu app.
///
/// `app` is the app id from the layout; an unknown id draws nothing. `value` is the border brightness.
pub fn draw_app_border(app: &str, grid: &mut Grid, value: i32, y: usize, core_count: usize) {
    match app {
        "cpu" => draw_cpu_border(grid, value, y, core_count),
        "cpu-graph" | "gpu-graph" => draw_half_outline(grid, value, y),
        "disk-net" => draw_disk_net_border(grid, value, y),
        "disk" | "net" | "fan" => draw_2_x_1_horiz_grid(grid, value, y, 4),
        "mem" | "mem-bat" | "bat" => draw_1_x_2_vert_grid(grid, value, y, 3),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the load-to-spiral steps match the values Python gives at the edges of the buckets.
    #[test]
    fn spiral_index_matches_python() {
        assert_eq!(spiral_index(0.0), 0);
        assert_eq!(spiral_index(0.1), 0);
        assert_eq!(spiral_index(0.15), 1);
        assert_eq!(spiral_index(1.0), 9);
    }

    /// Proves the three FPS layouts (1 as a line, single digit centred) and the 999 cap.
    #[test]
    fn fps_columns() {
        assert_eq!(fps_number_columns(144.0), vec![('|', 0, false), ('4', 2, false), ('4', 6, false)]);
        assert_eq!(fps_number_columns(7.0), vec![('7', 3, false)]);
        assert_eq!(fps_number_columns(1500.0)[0].0, '9');
    }
}

#[cfg(test)]
mod graph_option_tests {
    use super::*;

    /// Proves the graph options: no options equals the plain graph, "right" mirrors it left to right and
    /// "bottom" flips it top to bottom.
    #[test]
    fn graph_options_mirror_and_flip_the_default_graph() {
        let values: Vec<Option<f64>> = (0..15).map(|i| Some(i as f64 / 14.0)).collect();
        let mut plain = Grid::new();
        draw_history_graph(&mut plain, &values, 200, 0);
        let mut same = Grid::new();
        draw_history_graph_with(&mut same, &values, 200, 0, "", "");
        assert_eq!(plain.v, same.v);
        let mut right = Grid::new();
        draw_history_graph_with(&mut right, &values, 200, 0, "right", "top");
        let mut bottom = Grid::new();
        draw_history_graph_with(&mut bottom, &values, 200, 0, "left", "bottom");
        for row in 1..16 {
            for x in 1..8 {
                assert_eq!(right.v[x][row], plain.v[8 - x][row]);
                assert_eq!(bottom.v[x][row], plain.v[x][16 - row]);
            }
        }
    }
}
