//! How one picture is spread over both LED panels (Games and Math), saved as "span" in settings.json
//! (`white_rabbit/span.py`):
//!
//! - side_by_side: the panels touch, the picture is 18 columns wide, 9 on each panel
//! - keyboard_gap: the picture is wider and the columns hidden behind the keyboard are left out
//! - mirror: the picture is one panel wide; the right panel shows it and the left panel mirrors it

use serde_json::{json, Value};

use crate::games::Pic;
use crate::settings;

pub const PANEL_WIDTH: usize = 9;
pub const SIDE_BY_SIDE: &str = "side_by_side";
pub const KEYBOARD_GAP: &str = "keyboard_gap";
pub const MIRROR: &str = "mirror";
pub const MODES: [(&str, &str); 3] = [
    (SIDE_BY_SIDE, "Side by side"),
    (KEYBOARD_GAP, "Side by side, skip the keyboard"),
    (MIRROR, "Mirror (left copies right)"),
];
pub const DEFAULT_GAP: i64 = 30;
pub const MIN_GAP: i64 = 0;
pub const MAX_GAP: i64 = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub mode: String,
    pub gap_columns: usize,
}

impl Default for Span {
    fn default() -> Span {
        Span { mode: SIDE_BY_SIDE.into(), gap_columns: DEFAULT_GAP as usize }
    }
}

impl Span {
    /// The settings with every key filled in and in range.
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

    pub fn load() -> Span {
        Span::from_value(settings::get("span").as_ref())
    }

    pub fn save(&self) -> bool {
        settings::set("span", json!({"mode": self.mode, "gap_columns": self.gap_columns}))
    }

    pub fn mode_index(&self) -> usize {
        MODES.iter().position(|(k, _)| *k == self.mode).unwrap_or(0)
    }

    /// How wide a picture to make for this many panels (one panel is always 9 columns).
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
    pub fn to_panels(&self, picture: &Pic, panel_count: usize) -> Pic {
        if panel_count != 2 {
            return picture.clone();
        }
        match self.mode.as_str() {
            MIRROR => {
                let mut out = Pic::new(2 * PANEL_WIDTH);
                for y in 0..crate::games::HEIGHT {
                    for x in 0..PANEL_WIDTH {
                        let v = picture.get(x, y);
                        out.set(PANEL_WIDTH + x, y, v);
                        out.set(PANEL_WIDTH - 1 - x, y, v);
                    }
                }
                out
            }
            KEYBOARD_GAP => {
                let mut out = Pic::new(2 * PANEL_WIDTH);
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

#[cfg(test)]
mod tests {
    use super::*;

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
