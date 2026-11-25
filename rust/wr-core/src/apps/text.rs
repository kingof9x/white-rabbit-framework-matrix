//! The Text app: your own words on a whole panel, with live values filled in. A port of
//! `led_mon/text_styles.py`.
//!
//! `{time}`, `{battery}` and `{cpu}` are filled in each time the panel is drawn. Three styles:
//! ticker (upright letters scrolling up the panel), sideways (letters on their side, also scrolling)
//! and stacked (two letters per row, still when it fits, otherwise sliding down and back up).
//!
//! Where this sits: `render::render_panel` calls `draw_app` for an app named "text" (on the monitor's
//! main loop, every frame, and in the settings window's preview). `draw_app` reads the live values
//! through the `Readings` trait, fills them into the text, and redraws the whole 9 x 34 grid.
//!
//! How it works: the text is first turned into a "strip", a list of 9-pixel rows as long as the text
//! needs (letters from the 3 x 5 `TEXT_FONT` in `patterns.rs`). The scrolling styles then show a
//! 34-row window of that strip that moves down it with time, wrapping round with a gap; the stacked
//! style shows the top of it, or bounces the window when the strip is taller than the panel. Because
//! the position only depends on the clock, nothing has to be remembered between frames.

use crate::grid::{Grid, HEIGHT, WIDTH};
use crate::layout::App;
use crate::patterns::{self, Pattern, TEXT_FONT};
use crate::pyround::{int, int_round};
use crate::render::Readings;

/// The style names a layout can give in the app's "style" setting; anything else draws as ticker.
pub const STYLES: [&str; 3] = ["ticker", "sideways", "stacked"];
/// The text shown when the layout has no "text" setting at all.
pub const DEFAULT_TEXT: &str = "{time} {battery} {cpu}";

/// Rows per second the ticker and sideways styles move.
const SCROLL_SPEED: f64 = 6.0;
/// Empty rows between the end of the text and its next pass.
const LOOP_GAP: usize = 10;
/// Stacked text that doesn't fit: seconds to wait at each end, and rows per second while moving.
const STACK_PAUSE: f64 = 2.5;
/// Rows per second while stacked text slides (see `STACK_PAUSE` above).
const STACK_SPEED: f64 = 4.0;
/// Brightness of the small % under a stacked number and of the dotted line between stacked words.
/// A fraction of the app's brightness (55%).
const DIM: f64 = 0.55;

/// One row of a strip: a value for each of the panel's 9 columns.
type Row = [i32; WIDTH];

/// The letter's 5 x 3 shape; letters the font doesn't have draw as "?".
/// Lower case is upper-cased first because the font only has capitals. The "?" glyph is always in
/// the font, so the inner unwrap cannot fail.
fn glyph(ch: char) -> Pattern {
    let upper: String = ch.to_uppercase().collect();
    patterns::find(TEXT_FONT, &upper).unwrap_or_else(|| patterns::find(TEXT_FONT, "?").unwrap())
}

// Live values ----------------------------------------------------------------------------------------

/// The strings put in place of `{time}`, `{battery}` and `{cpu}`.
/// Built fresh for every frame by `values_from` (or given by tests and the preview).
#[derive(Clone, Debug, PartialEq)]
pub struct TextValues {
    /// The clock, such as "10:47"; empty if the clock digits could not be read.
    pub time: String,
    /// The battery charge, such as "87%", or "--%" without a battery.
    pub battery: String,
    /// The CPU use, such as "12%", or "--%" before any reading.
    pub cpu: String,
}

/// The clock as 10:47 (12-hour, no leading zero) or 09:05 (24-hour).
/// `hour` is 0-23. In 12-hour form, midnight and noon both show as 12.
pub fn time_text(hour: u32, minute: u32, fmt_24_hour: bool) -> String {
    if fmt_24_hour {
        format!("{hour:02}:{minute:02}")
    } else {
        let h = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        format!("{h}:{minute:02}")
    }
}

/// A share as 87%, or --% when it isn't known.
/// `ratio` is 0.0-1.0; values outside are clamped, and the rounding is Python's (halves go to the
/// even number, so 0.125 shows 12%).
pub fn percent_text(ratio: Option<f64>) -> String {
    match ratio {
        Some(r) => format!("{}%", int_round(r.clamp(0.0, 1.0) * 100.0)),
        None => "--%".into(),
    }
}

/// `text` with `{time}`, `{battery}` and `{cpu}` (in any case) replaced. Anything else in braces is
/// left as typed.
/// A "{" with no matching token is copied as is and the search goes on from the next character, so
/// "{{time}}" gives "{" plus the time plus "}". Never panics on non-ASCII text.
pub fn fill_tokens(text: &str, values: &TextValues) -> String {
    let tokens: [(&str, &str); 3] = [("time", &values.time), ("battery", &values.battery), ("cpu", &values.cpu)];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        for (name, value) in tokens {
            // `get` (not slicing) returns None instead of panicking when the cut would fall inside
            // a multi-byte character such as "é"; then it is simply not a token.
            let matched = after.get(..name.len()).is_some_and(|w| w.eq_ignore_ascii_case(name));
            if matched && after[name.len()..].starts_with('}') {
                out.push_str(value);
                rest = &after[name.len() + 1..];
                continue 'outer;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

// Building the picture -------------------------------------------------------------------------------
// A strip is 9 columns wide and as many rows long as the text needs.

/// Adds `count` dark rows to the end of the strip (used for letter gaps and spaces).
fn empty_rows(strip: &mut Vec<Row>, count: usize) {
    strip.extend(std::iter::repeat_n([0; WIDTH], count));
}

/// Upright letters, one under the other in the middle of the panel.
/// Each letter takes 6 rows (5 rows of letter and a 1-row gap) in columns 3-5, the middle of the
/// 9-wide panel; a space or other whitespace is 3 dark rows. `fill_value` is the LED brightness.
pub fn ticker_strip(text: &str, fill_value: i32) -> Vec<Row> {
    let mut strip = Vec::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            empty_rows(&mut strip, 3);
            continue;
        }
        let g = glyph(ch);
        let start = strip.len();
        empty_rows(&mut strip, 6);
        for y in 0..5 {
            for x in 0..3 {
                strip[start + y][3 + x] = g.at(y, x) * fill_value;
            }
        }
    }
    strip
}

/// Letters turned a quarter turn clockwise, so the text reads down the panel with your head tilted right.
/// Each letter takes 4 rows (its 3 columns become 3 rows, plus a 1-row gap) and its 5 rows become
/// columns 2-6; whitespace is 3 dark rows.
pub fn sideways_strip(text: &str, fill_value: i32) -> Vec<Row> {
    let mut strip = Vec::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            empty_rows(&mut strip, 3);
            continue;
        }
        let g = glyph(ch);
        let start = strip.len();
        empty_rows(&mut strip, 4);
        // The letter's bottom row ends up on the left
        for y in 0..3 {
            for x in 0..5 {
                // Strip row y is the letter's column y; strip column 2 + x is the letter's
                // row 4 - x, so the letter's top row lands on the right.
                strip[start + y][2 + x] = g.at(4 - x, y) * fill_value;
            }
        }
    }
    strip
}

/// Each word as rows of up to two letters. A colon starts a new row (10:47 is 10 over 47).
/// Empty parts (from "::" or a word ending in ":") add no rows; whitespace only separates words.
pub fn stacked_lines(text: &str) -> Vec<Vec<String>> {
    let mut words = Vec::new();
    for word in text.split_whitespace() {
        let mut rows = Vec::new();
        for part in word.split(':') {
            let chars: Vec<char> = part.chars().collect();
            for pair in chars.chunks(2) {
                rows.push(pair.iter().collect::<String>());
            }
        }
        if !rows.is_empty() {
            words.push(rows);
        }
    }
    words
}

/// Two letters per row, each word under the last with a dotted line between words. A % left on its
/// own row becomes a small, dimmer % right under the number.
/// A row of two letters puts them in columns 1-3 and 5-7 with a dark column between; a single
/// letter sits in the middle (columns 3-5). Each row is 6 rows tall (5 plus a gap).
pub fn stacked_strip(text: &str, fill_value: i32) -> Vec<Row> {
    let mut strip: Vec<Row> = Vec::new();
    // The brightness of the dotted line and the small %, a bit over half the text's.
    let dim = int_round(fill_value as f64 * DIM) as i32;
    for (w, rows) in stacked_lines(text).iter().enumerate() {
        if w > 0 {
            // The row above is already empty (the gap under the last line), so: dots, then one empty row
            let mut dots = [0; WIDTH];
            // Dots in columns 1, 3, 5 and 7.
            for x in (1..8).step_by(2) {
                dots[x] = dim;
            }
            strip.push(dots);
            empty_rows(&mut strip, 1);
        }
        for (r, row) in rows.iter().enumerate() {
            if row == "%" && r > 0 {
                // No gap above it
                strip.pop();
                for y in 0..3 {
                    let mut line = [0; WIDTH];
                    // Three dim pixels on a diagonal (columns 5, 4, 3 going down): a tiny "/"
                    // that reads as a percent sign under the number.
                    line[3 + 2 - y] = dim;
                    strip.push(line);
                }
                empty_rows(&mut strip, 1);
                continue;
            }
            let start = strip.len();
            empty_rows(&mut strip, 6);
            let letters: Vec<char> = row.chars().collect();
            // (letter, leftmost column) for one or two letters on this row.
            let placed: Vec<(char, usize)> =
                if letters.len() == 1 { vec![(letters[0], 3)] } else { vec![(letters[0], 1), (letters[1], 5)] };
            for (ch, left) in placed {
                let g = glyph(ch);
                for y in 0..5 {
                    for x in 0..3 {
                        strip[start + y][left + x] = g.at(y, x) * fill_value;
                    }
                }
            }
        }
    }
    // Drop the empty row after the last line
    if strip.last().is_some_and(|row| row.iter().all(|&v| v == 0)) {
        strip.pop();
    }
    strip
}

/// The 34 rows of `strip` starting `offset` rows in, the text repeating with a gap between passes.
/// `offset` grows with time, so the text moves up the panel. Rows of the gap (indexes past the end
/// of the strip) are left as they were, which is dark because `draw_text` clears the grid first.
fn scroll_window(grid: &mut Grid, strip: &[Row], offset: usize) {
    let length = strip.len() + LOOP_GAP;
    for y in 0..HEIGHT {
        // Wrap round the strip plus its gap, so the text comes back from the bottom.
        let i = (y + offset) % length;
        if let Some(row) = strip.get(i) {
            for (x, v) in row.iter().enumerate() {
                grid.set(x, y, *v);
            }
        }
    }
}

/// For stacked text taller than the panel: wait at the top, slide down, wait, slide back up.
/// `overflow` is how many rows the strip is taller than the panel, and `seconds` a rising clock.
/// Returns how many rows down the window starts (0..=overflow). One cycle is: pause at the top,
/// slide down at `STACK_SPEED` rows a second, pause at the bottom, slide back up.
pub fn bounce_offset(overflow: usize, seconds: f64) -> usize {
    let overflow_f = overflow as f64;
    // Seconds one slide takes; the whole cycle is two pauses and two slides.
    let travel = overflow_f / STACK_SPEED;
    let cycle = 2.0 * (STACK_PAUSE + travel);
    let mut t = seconds % cycle;
    if t < STACK_PAUSE {
        return 0;
    }
    t -= STACK_PAUSE;
    if t < travel {
        return int(t * STACK_SPEED) as usize;
    }
    t -= travel;
    if t < STACK_PAUSE {
        return overflow;
    }
    t -= STACK_PAUSE;
    (overflow as i64 - int(t * STACK_SPEED)) as usize
}

/// Draws `text` (live values already filled in) on the whole panel. `seconds` is a steadily rising
/// clock that moves the scrolling styles.
/// The grid is cleared first, so the app always owns the whole panel. Blank text leaves it dark.
/// Any style other than "stacked" or "sideways" draws as ticker.
pub fn draw_text(grid: &mut Grid, text: &str, style: &str, fill_value: i32, seconds: f64) {
    *grid = Grid::new();
    if text.trim().is_empty() {
        return;
    }
    if style == "stacked" {
        let strip = stacked_strip(text, fill_value);
        let start = if strip.len() > HEIGHT { bounce_offset(strip.len() - HEIGHT, seconds) } else { 0 };
        for (y, row) in strip.iter().skip(start).take(HEIGHT).enumerate() {
            for (x, v) in row.iter().enumerate() {
                grid.set(x, y, *v);
            }
        }
        return;
    }
    let strip = if style == "sideways" { sideways_strip(text, fill_value) } else { ticker_strip(text, fill_value) };
    if strip.is_empty() {
        return;
    }
    // SCROLL_SPEED rows a second since the clock started.
    scroll_window(grid, &strip, int(seconds * SCROLL_SPEED) as usize);
}

/// The app's `text` arg: the default when missing, nothing when empty.
/// A setting written as just `text:` (YAML null) means the user cleared it, so the panel stays dark;
/// a number is used as its digits; any other kind of value (such as true) counts as empty.
pub fn text_arg(app: &App) -> String {
    match app.arg("text") {
        None => DEFAULT_TEXT.to_string(),
        Some(serde_yaml::Value::Null) => String::new(),
        Some(_) => app.arg_str("text").unwrap_or_default(),
    }
}

/// The app's `style` arg, ticker when it isn't one of the styles.
/// Returns one of `STYLES`.
pub fn style_arg(app: &App) -> &'static str {
    let style = app.arg_str("style").unwrap_or_default();
    STYLES.iter().copied().find(|s| *s == style).unwrap_or("ticker")
}

/// Fills in the live values and draws: `TextApp.draw` in Python.
/// `seconds` is a steadily rising clock in seconds; it moves the scrolling and bouncing styles.
pub fn draw_with_values(grid: &mut Grid, text: &str, style: &str, fill_value: i32, values: &TextValues, seconds: f64) {
    let style = if STYLES.contains(&style) { style } else { "ticker" };
    draw_text(grid, &fill_tokens(text, values), style, fill_value, seconds);
}

/// The live values from the computer: the clock (with the app's 24-hour setting), the battery, and
/// the whole CPU's use averaged over the last 3 seconds.
/// `readings.clock` gives four digits ("0905") already in the app's 12- or 24-hour form, plus a PM
/// flag; they are turned into "9:05" / "09:05" here. A missing battery or CPU history gives "--%".
pub fn values_from(readings: &mut dyn Readings, app: &App) -> TextValues {
    let fmt_24 = app.arg_bool("fmt_24_hour");
    let (digits, pm) = readings.clock(app);
    let time = match (digits.get(..2).and_then(|h| h.parse::<u32>().ok()), digits.get(2..4)) {
        (Some(hour), Some(minute)) if fmt_24 => format!("{hour:02}:{minute}"),
        (Some(hour), Some(minute)) => {
            // The clock gives 12-hour digits; back to a 24-hour hour for time_text
            let h24 = hour % 12 + if pm { 12 } else { 0 };
            time_text(h24, minute.parse().unwrap_or(0), false)
        }
        _ => String::new(),
    };
    // The history is one reading a second, newest first: the first three that are known.
    let samples: Vec<f64> = readings.cpu_history().into_iter().take(3).flatten().collect();
    let cpu = if samples.is_empty() { None } else { Some(samples.iter().sum::<f64>() / samples.len() as f64) };
    TextValues { time, battery: percent_text(readings.battery().map(|b| b.0)), cpu: percent_text(cpu) }
}

/// The Text app on the whole panel.
/// Called by `render.rs` for the "text" app; draws over the whole panel.
pub fn draw_app(grid: &mut Grid, app: &App, readings: &mut dyn Readings, fill_value: i32) {
    let values = values_from(readings, app);
    let seconds = readings.now();
    draw_with_values(grid, &text_arg(app), style_arg(app), fill_value, &values, seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample live values used by the tests below.
    fn values() -> TextValues {
        TextValues { time: "1:05".into(), battery: "88%".into(), cpu: "--%".into() }
    }

    /// Tokens match in any case, unknown or unclosed braces stay as typed, doubled braces keep the outer
    /// pair, and a non-ASCII character just before a "{" does not cause a panic.
    #[test]
    fn tokens_fill_in_any_case_and_others_stay() {
        assert_eq!(fill_tokens("{TIME} {Battery} {cpu} {other} {", &values()), "1:05 88% --% {other} {");
        assert_eq!(fill_tokens("{{time}}", &values()), "{1:05}");
        assert_eq!(fill_tokens("é{cpu", &values()), "é{cpu");
    }

    /// 12-hour times have no leading zero and show midnight as 12; percents round like Python, are
    /// clamped to 100%, and show "--%" when unknown.
    #[test]
    fn times_and_percents_like_python() {
        assert_eq!(time_text(0, 5, false), "12:05");
        assert_eq!(time_text(13, 47, false), "1:47");
        assert_eq!(time_text(9, 5, true), "09:05");
        assert_eq!(percent_text(Some(0.125)), "12%");
        assert_eq!(percent_text(Some(0.135)), "14%");
        assert_eq!(percent_text(Some(1.7)), "100%");
        assert_eq!(percent_text(None), "--%");
    }

    /// "10:47" becomes "10" over "47", and "87%" becomes "87" over "%".
    #[test]
    fn stacked_rows_split_at_colons() {
        assert_eq!(stacked_lines("10:47 87%"), vec![vec!["10", "47"], vec!["87", "%"]]);
    }

    /// With 8 rows to travel: still at the top for 2.5 s, sliding at 4 rows a second, still at the
    /// bottom, sliding back, and at the top again after one 9-second cycle.
    #[test]
    fn bounce_waits_then_slides() {
        assert_eq!(bounce_offset(8, 1.0), 0);
        assert_eq!(bounce_offset(8, 3.0), 2);
        assert_eq!(bounce_offset(8, 5.0), 8);
        assert_eq!(bounce_offset(8, 7.5), 6);
        assert_eq!(bounce_offset(8, 9.0), 0);
    }
}
