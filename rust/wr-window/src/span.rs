//! How one picture is spread over both LED panels (Games and Math), saved as "span" in settings.json
//! (`white_rabbit/span.py`):
//!
//! - side_by_side: the panels touch, the picture is 18 columns wide, 9 on each panel
//! - keyboard_gap: the picture is wider and the columns hidden behind the keyboard are left out
//! - mirror: the picture is one panel wide; the right panel shows it and the left panel mirrors it
//!
//! Games and Math draw into one wide picture (`Pic`, brightness 0..1 per pixel) of the width
//! `picture_width` asks for, then call `to_panels` to get the 18 columns the two panels actually show
//! (9 left, 9 right). With one panel (or more than two) the picture is simply one or more panels
//! wide and passed through unchanged. Read from settings.json when a game or Math picture starts; the
//! layout choice on the Games and Math pages writes it.

use serde_json::{json, Value};

use crate::games::Pic;
use crate::settings;

/// LED columns on one panel.
pub const PANEL_WIDTH: usize = 9;
/// The setting's value for each mode, as stored in settings.json.
pub const SIDE_BY_SIDE: &str = "side_by_side";
pub const KEYBOARD_GAP: &str = "keyboard_gap";
pub const MIRROR: &str = "mirror";
/// (setting value, name shown in the window) for each mode, in the order of the drop-down list.
pub const MODES: [(&str, &str); 3] = [
    (SIDE_BY_SIDE, "Side by side"),
    (KEYBOARD_GAP, "Side by side, skip the keyboard"),
    (MIRROR, "Mirror (left copies right)"),
];
/// Columns hidden behind the keyboard in `keyboard_gap` mode: the default, and the allowed range.
pub const DEFAULT_GAP: i64 = 30;
pub const MIN_GAP: i64 = 0;
pub const MAX_GAP: i64 = 120;

/// The span setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// One of `SIDE_BY_SIDE`, `KEYBOARD_GAP`, `MIRROR` (anything else is replaced on loading)
    pub mode: String,
    /// Picture columns between the two panels that are not shown, 0..=120 (keyboard_gap only)
    pub gap_columns: usize,
}

impl Default for Span {
    fn default() -> Span {
        Span { mode: SIDE_BY_SIDE.into(), gap_columns: DEFAULT_GAP as usize }
    }
}

impl Span {
    /// The settings with every key filled in and in range.
    ///
    /// An unknown mode becomes side by side. The gap may be stored as a whole number, a decimal (cut to a
    /// whole number) or text; anything unreadable becomes 30, and the result is clamped to 0..=120.
    pub fn from_value(v: Option<&Value>) -> Span {
        let mode = v.and_then(|v| v.get("mode")).and_then(Value::as_str).unwrap_or(SIDE_BY_SIDE);
        let mode = if MODES.iter().any(|(k, _)| *k == mode) { mode } else { SIDE_BY_SIDE };
        let gap = v
            .and_then(|v| v.get("gap_columns"))
            .and_then(|g| {
                g.as_i64().or_else(|| g.as_f64().map(|f| f as i64)).or_else(|| g.as_str()?.trim().parse().ok())
            })
            .unwrap_or(DEFAULT_GAP);
        Span { mode: mode.into(), gap_columns: gap.clamp(MIN_GAP, MAX_GAP) as usize }
    }

    /// The span setting from settings.json (defaults if it is missing).
    pub fn load() -> Span {
        Span::from_value(settings::get("span").as_ref())
    }

    /// Writes the setting to settings.json; false (and a log line) if that failed.
    pub fn save(&self) -> bool {
        settings::set("span", json!({"mode": self.mode, "gap_columns": self.gap_columns}))
    }

    /// This mode's position in `MODES`, for the drop-down list.
    pub fn mode_index(&self) -> usize {
        MODES.iter().position(|(k, _)| *k == self.mode).unwrap_or(0)
    }

    /// How wide a picture to make for this many panels (one panel is always 9 columns).
    ///
    /// Two panels: side by side 18, mirror 9, keyboard gap 18 + gap. No panels counts as one.
    pub fn picture_width(&self, panel_count: usize) -> usize {
        if panel_count < 2 {
            return PANEL_WIDTH * panel_count.max(1);
        }
        match self.mode.as_str() {
            MIRROR => PANEL_WIDTH,
            KEYBOARD_GAP => 2 * PANEL_WIDTH + self.gap_columns,
            _ => PANEL_WIDTH * panel_count,
        }
    }

    /// The columns the panels show (9 per panel, left to right) for a picture made picture_width() wide.
    ///
    /// Only two panels get rearranged; any other count gets the picture back unchanged.
    pub fn to_panels(&self, picture: &Pic, panel_count: usize) -> Pic {
        if panel_count != 2 {
            return picture.clone();
        }
        match self.mode.as_str() {
            MIRROR => {
                let mut out = Pic::new(2 * PANEL_WIDTH);
                for y in 0..crate::games::HEIGHT {
                    for x in 0..PANEL_WIDTH {
                        // Column x of the picture goes to column x of the right panel and, flipped, to column
                        // 8 - x of the left one, so the two panels look like a mirror image around the gap
                        // between them.
                        let v = picture.get(x, y);
                        out.set(PANEL_WIDTH + x, y, v);
                        out.set(PANEL_WIDTH - 1 - x, y, v);
                    }
                }
                out
            }
            KEYBOARD_GAP => {
                let mut out = Pic::new(2 * PANEL_WIDTH);
                // The left panel shows the picture's first 9 columns, the right panel its last 9; the gap
                // columns in between are simply never shown.
                let right = picture.w.saturating_sub(PANEL_WIDTH);
                for y in 0..crate::games::HEIGHT {
                    for x in 0..PANEL_WIDTH {
                        out.set(x, y, picture.get(x, y));
                        out.set(PANEL_WIDTH + x, y, picture.get(right + x, y));
                    }
                }
                out
            }
            _ => picture.clone(),
        }
    }
}

/// Tests for spreading one picture over two panels.
#[cfg(test)]
mod tests {
    use super::*;

    /// Bad settings are fixed on loading, and each mode asks for the right picture width.
    #[test]
    fn widths_and_bad_settings() {
        let s = Span::from_value(Some(&json!({"mode": "nope", "gap_columns": 500})));
        assert_eq!(s, Span { mode: SIDE_BY_SIDE.into(), gap_columns: 120 });
        assert_eq!(s.picture_width(1), 9);
        assert_eq!(s.picture_width(2), 18);
        let gap = Span { mode: KEYBOARD_GAP.into(), gap_columns: 30 };
        assert_eq!(gap.picture_width(2), 48);
        assert_eq!(Span { mode: MIRROR.into(), gap_columns: 30 }.picture_width(2), 9);
    }

    /// Mirror copies one panel's picture to both, flipped on the left; keyboard gap drops the hidden middle.
    #[test]
    fn mirror_and_gap() {
        let mut p = Pic::new(9);
        p.set(0, 0, 1.0);
        let m = Span { mode: MIRROR.into(), gap_columns: 0 }.to_panels(&p, 2);
        assert_eq!((m.w, m.get(9, 0), m.get(8, 0)), (18, 1.0, 1.0));
        let mut wide = Pic::new(48);
        wide.set(47, 3, 0.5);
        wide.set(20, 3, 1.0);
        let g = Span { mode: KEYBOARD_GAP.into(), gap_columns: 30 }.to_panels(&wide, 2);
        assert_eq!(g.get(17, 3), 0.5);
        assert!(g.v.iter().all(|v| *v != 1.0));
    }
}
