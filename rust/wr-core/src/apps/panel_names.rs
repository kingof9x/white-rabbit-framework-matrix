//! A panel's name (settings.json "panel_names", set in the White Rabbit window), shown first by
//! Identify panels. A port of `led_mon/panel_names.py` and `load_panel_names` in `led_mon/app_settings.py`.
//!
//! Where this sits: when the user presses Identify panels, the monitor's main loop (`wr-monitor`'s
//! `monitor.rs`) calls `load_panel_names` to read settings.json from disk, then `name_grid` to turn
//! each name into a picture that is sent to that panel. The point is to tell the user which physical
//! panel is which, so a panel without a name shows its side ("Left" / "Right") instead.
//!
//! How the picture is made: the panel is 9 LEDs wide and 34 tall, so letters are stacked vertically,
//! one 5 x 6 letter per 7 rows (6 rows of letter plus one blank row), at most 5 letters, centred top
//! to bottom and with two blank columns on each side.

use std::path::Path;

use serde_json::Value;

use crate::grid::{Grid, HEIGHT};
use crate::patterns::{self, NAME_FONT};

/// How many letters of a name fit on a panel.
/// Five letters of 7 rows each use 34 rows (35 minus the gap after the last), which is exactly the
/// panel's height, so a sixth letter would not fit.
pub const LETTERS: usize = 5;
/// The longest name the window keeps.
/// Longer names are cut to this many characters (not bytes) by `panel_names_from_json`.
pub const NAME_MAX: usize = 20;

/// A grid with the first five letters of the name stacked top to bottom (5 x 6 font).
/// Letters are upper-cased first because the name font only has capitals; a character the font
/// lacks is drawn as "?". `fill_value` is the LED brightness (0-255) of the lit pixels; everything
/// else stays 0 (off). An empty name gives an all-dark grid.
pub fn name_grid(name: &str, fill_value: i32) -> Grid {
    let mut grid = Grid::new();
    let letters: Vec<char> = name.to_uppercase().chars().take(LETTERS).collect();
    if letters.is_empty() {
        return grid;
    }
    // Height of the stack: 7 rows per letter minus the blank row after the last one. Half of what
    // is left over goes above it, which centres the letters (rounding down puts an odd row below).
    let top = (HEIGHT - (letters.len() * 7 - 1)) / 2;
    // The "?" glyph always exists in the font, so this unwrap cannot fail at run time.
    let unknown = patterns::find(NAME_FONT, "?").unwrap();
    for (i, letter) in letters.iter().enumerate() {
        let pixels = patterns::find(NAME_FONT, &letter.to_string()).unwrap_or(unknown);
        for y in 0..6 {
            for x in 0..5 {
                if pixels.at(y, x) == 1 {
                    // Column 2 + x centres the 5-wide letter on the 9-wide panel (2 columns each side);
                    // row top + i * 7 + y is row y of letter i.
                    grid.set(2 + x, top + i * 7 + y, fill_value);
                }
            }
        }
    }
    grid
}

/// The names of the left and right panels from settings.json's text, trimmed, with runs of spaces made
/// one. A side without a name says which side it is ("Left", "Right"), so a mix-up shows.
/// Bad input never fails: text that is not JSON, a missing "panel_names" key, or a value that is not
/// a string (for example a number) all count as no name, which gives the side's fallback.
pub fn panel_names_from_json(text: &str) -> [String; 2] {
    let root: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let names = root.get("panel_names");
    ["left", "right"].map(|side| {
        let name = names.and_then(|n| n.get(side)).and_then(Value::as_str).unwrap_or("");
        // split_whitespace drops leading/trailing spaces and joins runs of spaces with one space;
        // then the name is cut to NAME_MAX characters (chars, so accented letters are not split).
        let clean: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_MAX).collect();
        if clean.is_empty() {
            let mut fallback = side.to_string();
            // "left" -> "Left": the side names are plain ASCII, so upper-casing the first byte is safe.
            fallback[..1].make_ascii_uppercase();
            fallback
        } else {
            clean
        }
    })
}

/// The panels' names from settings.json; "Left" and "Right" when it can't be read.
/// Reads the file from disk each time it is called (on the caller's thread, which is the monitor's
/// main loop); a missing or unreadable file counts as empty text and so gives the side names.
pub fn load_panel_names(settings_file: &Path) -> [String; 2] {
    panel_names_from_json(&std::fs::read_to_string(settings_file).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extra spaces are squeezed out, a non-string name falls back to the side's name, text that is not
    /// JSON gives both fallbacks, and a long name is cut to NAME_MAX characters.
    #[test]
    fn names_are_cleaned_and_default_to_the_side() {
        let names = panel_names_from_json(r#"{"panel_names": {"left": "  My   desk  ", "right": 5}}"#);
        assert_eq!(names, ["My desk".to_string(), "Right".to_string()]);
        assert_eq!(panel_names_from_json("nope"), ["Left".to_string(), "Right".to_string()]);
        let long = panel_names_from_json(&format!(r#"{{"panel_names": {{"left": "{}"}}}}"#, "x".repeat(30)));
        assert_eq!(long[0].len(), NAME_MAX);
    }

    /// An empty name lights nothing; a one-letter name lights some LEDs.
    #[test]
    fn an_empty_name_is_dark() {
        assert!(name_grid("", 160).is_dark());
        assert!(!name_grid("a", 160).is_dark());
    }
}
