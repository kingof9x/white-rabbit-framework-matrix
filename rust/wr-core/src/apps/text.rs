//! The Text app: your own words on a whole panel, with live values filled in. A port of
//! `led_mon/text_styles.py`.
//!
//! `{time}`, `{battery}` and `{cpu}` are filled in each time the panel is drawn. Three styles:
//! ticker (upright letters scrolling up the panel), sideways (letters on their side, also scrolling)
//! and stacked (two letters per row, still when it fits, otherwise sliding down and back up).

use crate::grid::{Grid, HEIGHT, WIDTH};
use crate::layout::App;
use crate::patterns::{self, Pattern, TEXT_FONT};
use crate::pyround::{int, int_round};
use crate::render::Readings;

pub const STYLES: [&str; 3] = ["ticker", "sideways", "stacked"];
pub const DEFAULT_TEXT: &str = "{time} {battery} {cpu}";

/// Rows per second the ticker and sideways styles move.
const SCROLL_SPEED: f64 = 6.0;
/// Empty rows between the end of the text and its next pass.
const LOOP_GAP: usize = 10;
/// Stacked text that doesn't fit: seconds to wait at each end, and rows per second while moving.
const STACK_PAUSE: f64 = 2.5;
const STACK_SPEED: f64 = 4.0;
/// Brightness of the small % under a stacked number and of the dotted line between stacked words.
const DIM: f64 = 0.55;

/// One row of a strip: a value for each of the panel's 9 columns.
type Row = [i32; WIDTH];

/// The letter's 5 x 3 shape; letters the font doesn't have draw as "?".
fn glyph(ch: char) -> Pattern {
    let upper: String = ch.to_uppercase().collect();
    patterns::find(TEXT_FONT, &upper).unwrap_or_else(|| patterns::find(TEXT_FONT, "?").unwrap())
}

// Live values ----------------------------------------------------------------------------------------

/// The strings put in place of `{time}`, `{battery}` and `{cpu}`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextValues {
    pub time: String,
    pub battery: String,
    pub cpu: String,
}

/// The clock as 10:47 (12-hour, no leading zero) or 09:05 (24-hour).
pub fn time_text(hour: u32, minute: u32, fmt_24_hour: bool) -> String {
    if fmt_24_hour {
        format!("{hour:02}:{minute:02}")
    } else {
        let h = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        format!("{h}:{minute:02}")
    }
}

/// A share as 87%, or --% when it isn't known.
pub fn percent_text(ratio: Option<f64>) -> String {
    match ratio {
        Some(r) => format!("{}%", int_round(r.clamp(0.0, 1.0) * 100.0)),
        None => "--%".into(),
    }
}

/// `text` with `{time}`, `{battery}` and `{cpu}` (in any case) replaced. Anything else in braces is
/// left as typed.
pub fn fill_tokens(text: &str, values: &TextValues) -> String {
    let tokens: [(&str, &str); 3] = [("time", &values.time), ("battery", &values.battery), ("cpu", &values.cpu)];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        for (name, value) in tokens {
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

fn empty_rows(strip: &mut Vec<Row>, count: usize) {
    strip.extend(std::iter::repeat_n([0; WIDTH], count));
}

/// Upright letters, one under the other in the middle of the panel.
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
                strip[start + y][2 + x] = g.at(4 - x, y) * fill_value;
            }
        }
    }
    strip
}

/// Each word as rows of up to two letters. A colon starts a new row (10:47 is 10 over 47).
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
pub fn stacked_strip(text: &str, fill_value: i32) -> Vec<Row> {
    let mut strip: Vec<Row> = Vec::new();
    let dim = int_round(fill_value as f64 * DIM) as i32;
    for (w, rows) in stacked_lines(text).iter().enumerate() {
        if w > 0 {
            // The row above is already empty (the gap under the last line), so: dots, then one empty row
            let mut dots = [0; WIDTH];
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
                    line[3 + 2 - y] = dim;
                    strip.push(line);
                }
                empty_rows(&mut strip, 1);
                continue;
            }
            let start = strip.len();
            empty_rows(&mut strip, 6);
            let letters: Vec<char> = row.chars().collect();
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
fn scroll_window(grid: &mut Grid, strip: &[Row], offset: usize) {
    let length = strip.len() + LOOP_GAP;
    for y in 0..HEIGHT {
        let i = (y + offset) % length;
        if let Some(row) = strip.get(i) {
            for (x, v) in row.iter().enumerate() {
                grid.set(x, y, *v);
            }
        }
    }
}

/// For stacked text taller than the panel: wait at the top, slide down, wait, slide back up.
pub fn bounce_offset(overflow: usize, seconds: f64) -> usize {
    let overflow_f = overflow as f64;
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
    scroll_window(grid, &strip, int(seconds * SCROLL_SPEED) as usize);
}

/// The app's `text` arg: the default when missing, nothing when empty.
pub fn text_arg(app: &App) -> String {
    match app.arg("text") {
        None => DEFAULT_TEXT.to_string(),
        Some(serde_yaml::Value::Null) => String::new(),
        Some(_) => app.arg_str("text").unwrap_or_default(),
    }
}

/// The app's `style` arg, ticker when it isn't one of the styles.
pub fn style_arg(app: &App) -> &'static str {
    let style = app.arg_str("style").unwrap_or_default();
    STYLES.iter().copied().find(|s| *s == style).unwrap_or("ticker")
}

/// Fills in the live values and draws: `TextApp.draw` in Python.
pub fn draw_with_values(grid: &mut Grid, text: &str, style: &str, fill_value: i32, values: &TextValues, seconds: f64) {
    let style = if STYLES.contains(&style) { style } else { "ticker" };
    draw_text(grid, &fill_tokens(text, values), style, fill_value, seconds);
}

/// The live values from the computer: the clock (with the app's 24-hour setting), the battery, and
/// the whole CPU's use averaged over the last 3 seconds.
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
    let samples: Vec<f64> = readings.cpu_history().into_iter().take(3).flatten().collect();
    let cpu = if samples.is_empty() { None } else { Some(samples.iter().sum::<f64>() / samples.len() as f64) };
    TextValues { time, battery: percent_text(readings.battery().map(|b| b.0)), cpu: percent_text(cpu) }
}

/// The Text app on the whole panel.
pub fn draw_app(grid: &mut Grid, app: &App, readings: &mut dyn Readings, fill_value: i32) {
    let values = values_from(readings, app);
    let seconds = readings.now();
    draw_with_values(grid, &text_arg(app), style_arg(app), fill_value, &values, seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> TextValues {
        TextValues { time: "1:05".into(), battery: "88%".into(), cpu: "--%".into() }
    }

    #[test]
    fn tokens_fill_in_any_case_and_others_stay() {
        assert_eq!(fill_tokens("{TIME} {Battery} {cpu} {other} {", &values()), "1:05 88% --% {other} {");
        assert_eq!(fill_tokens("{{time}}", &values()), "{1:05}");
        assert_eq!(fill_tokens("é{cpu", &values()), "é{cpu");
    }

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

    #[test]
    fn stacked_rows_split_at_colons() {
        assert_eq!(stacked_lines("10:47 87%"), vec![vec!["10", "47"], vec!["87", "%"]]);
    }

    #[test]
    fn bounce_waits_then_slides() {
        assert_eq!(bounce_offset(8, 1.0), 0);
        assert_eq!(bounce_offset(8, 3.0), 2);
        assert_eq!(bounce_offset(8, 5.0), 8);
        assert_eq!(bounce_offset(8, 7.5), 6);
        assert_eq!(bounce_offset(8, 9.0), 0);
    }
}
