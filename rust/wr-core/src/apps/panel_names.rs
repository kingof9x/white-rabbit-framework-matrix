//! A panel's name (settings.json "panel_names", set in the White Rabbit window), shown first by
//! Identify panels. A port of `led_mon/panel_names.py` and `load_panel_names` in `led_mon/app_settings.py`.

use std::path::Path;

use serde_json::Value;

use crate::grid::{Grid, HEIGHT};
use crate::patterns::{self, NAME_FONT};

/// How many letters of a name fit on a panel.
pub const LETTERS: usize = 5;
/// The longest name the window keeps.
pub const NAME_MAX: usize = 20;

/// A grid with the first five letters of the name stacked top to bottom (5 x 6 font).
pub fn name_grid(name: &str, fill_value: i32) -> Grid {
    let mut grid = Grid::new();
    let letters: Vec<char> = name.to_uppercase().chars().take(LETTERS).collect();
    if letters.is_empty() {
        return grid;
    }
    let top = (HEIGHT - (letters.len() * 7 - 1)) / 2;
    let unknown = patterns::find(NAME_FONT, "?").unwrap();
    for (i, letter) in letters.iter().enumerate() {
        let pixels = patterns::find(NAME_FONT, &letter.to_string()).unwrap_or(unknown);
        for y in 0..6 {
            for x in 0..5 {
                if pixels.at(y, x) == 1 {
                    grid.set(2 + x, top + i * 7 + y, fill_value);
                }
            }
        }
    }
    grid
}

/// The names of the left and right panels from settings.json's text, trimmed, with runs of spaces made
/// one. A side without a name says which side it is ("Left", "Right"), so a mix-up shows.
pub fn panel_names_from_json(text: &str) -> [String; 2] {
    let root: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let names = root.get("panel_names");
    ["left", "right"].map(|side| {
        let name = names.and_then(|n| n.get(side)).and_then(Value::as_str).unwrap_or("");
        let clean: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_MAX).collect();
        if clean.is_empty() {
            let mut fallback = side.to_string();
            fallback[..1].make_ascii_uppercase();
            fallback
        } else {
            clean
        }
    })
}

/// The panels' names from settings.json; "Left" and "Right" when it can't be read.
pub fn load_panel_names(settings_file: &Path) -> [String; 2] {
    panel_names_from_json(&std::fs::read_to_string(settings_file).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_and_default_to_the_side() {
        let names = panel_names_from_json(r#"{"panel_names": {"left": "  My   desk  ", "right": 5}}"#);
        assert_eq!(names, ["My desk".to_string(), "Right".to_string()]);
        assert_eq!(panel_names_from_json("nope"), ["Left".to_string(), "Right".to_string()]);
        let long = panel_names_from_json(&format!(r#"{{"panel_names": {{"left": "{}"}}}}"#, "x".repeat(30)));
        assert_eq!(long[0].len(), NAME_MAX);
    }

    #[test]
    fn an_empty_name_is_dark() {
        assert!(name_grid("", 160).is_dark());
        assert!(!name_grid("a", 160).is_dark());
    }
}
