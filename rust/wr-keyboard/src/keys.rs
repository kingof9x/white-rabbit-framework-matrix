//! Physical key layouts of the Framework Laptop 16 input modules, the LED
//! positions of their firmware, and how keys and LEDs line up.
//!
//! Key positions are in key units (1u = one normal key). The keyboard is
//! 18.25u x 6.75u; the numpad and macropad share one grid of 4.75 x 8.25
//! "module units" that is drawn at the same physical height as the keyboard.

use crate::protocol::LedPoint;

/// One keycap: legends (small top line, main line) and its rectangle in units.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyDef {
    pub top: String,
    pub main: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl KeyDef {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    /// A modifier / non-letter key (QMK's "Alphas / mods" effect colors these differently).
    pub fn is_modifier(&self) -> bool {
        self.main.chars().count() > 1
            && !(self.main.starts_with('F') && self.main[1..].chars().all(|c| c.is_ascii_digit()))
    }
}

fn k(top: &str, main: &str, x: f32, y: f32, w: f32, h: f32) -> KeyDef {
    KeyDef { top: top.into(), main: main.into(), x, y, w, h }
}

pub const KEYBOARD_SIZE: (f32, f32) = (18.25, 6.75);
/// Side module grid size in module units.
pub const MODULE_GRID: (f32, f32) = (4.75, 8.25);

/// Framework 16 ANSI keyboard, Framework's default legends ("Super" for the Windows key).
pub fn keyboard_keys() -> Vec<KeyDef> {
    vec![
        k("", "Esc", 0.0, 0.0, 1.25, 0.5),
        k("", "F1", 1.5, 0.0, 1.0, 0.5),
        k("", "F2", 2.75, 0.0, 1.0, 0.5),
        k("", "F3", 4.0, 0.0, 1.0, 0.5),
        k("", "F4", 5.25, 0.0, 1.0, 0.5),
        k("", "F5", 6.5, 0.0, 1.0, 0.5),
        k("", "F6", 7.75, 0.0, 1.0, 0.5),
        k("", "F7", 9.0, 0.0, 1.0, 0.5),
        k("", "F8", 10.25, 0.0, 1.0, 0.5),
        k("", "F9", 11.5, 0.0, 1.0, 0.5),
        k("", "F10", 12.75, 0.0, 1.0, 0.5),
        k("", "F11", 14.0, 0.0, 1.0, 0.5),
        k("", "F12", 15.25, 0.0, 1.0, 0.5),
        k("", "Delete", 16.5, 0.0, 1.75, 0.5),
        k("~", "`", 0.0, 0.75, 1.0, 1.0),
        k("!", "1", 1.25, 0.75, 1.0, 1.0),
        k("@", "2", 2.5, 0.75, 1.0, 1.0),
        k("#", "3", 3.75, 0.75, 1.0, 1.0),
        k("$", "4", 5.0, 0.75, 1.0, 1.0),
        k("%", "5", 6.25, 0.75, 1.0, 1.0),
        k("^", "6", 7.5, 0.75, 1.0, 1.0),
        k("&", "7", 8.75, 0.75, 1.0, 1.0),
        k("*", "8", 10.0, 0.75, 1.0, 1.0),
        k("(", "9", 11.25, 0.75, 1.0, 1.0),
        k(")", "0", 12.5, 0.75, 1.0, 1.0),
        k("_", "-", 13.75, 0.75, 1.0, 1.0),
        k("+", "=", 15.0, 0.75, 1.0, 1.0),
        k("", "Backspace", 16.25, 0.75, 2.0, 1.0),
        k("", "Tab", 0.0, 2.0, 1.5, 1.0),
        k("", "Q", 1.75, 2.0, 1.0, 1.0),
        k("", "W", 3.0, 2.0, 1.0, 1.0),
        k("", "E", 4.25, 2.0, 1.0, 1.0),
        k("", "R", 5.5, 2.0, 1.0, 1.0),
        k("", "T", 6.75, 2.0, 1.0, 1.0),
        k("", "Y", 8.0, 2.0, 1.0, 1.0),
        k("", "U", 9.25, 2.0, 1.0, 1.0),
        k("", "I", 10.5, 2.0, 1.0, 1.0),
        k("", "O", 11.75, 2.0, 1.0, 1.0),
        k("", "P", 13.0, 2.0, 1.0, 1.0),
        k("{", "[", 14.25, 2.0, 1.0, 1.0),
        k("}", "]", 15.5, 2.0, 1.0, 1.0),
        k("|", "\\", 16.75, 2.0, 1.5, 1.0),
        k("", "Caps", 0.0, 3.25, 1.75, 1.0),
        k("", "A", 2.0, 3.25, 1.0, 1.0),
        k("", "S", 3.25, 3.25, 1.0, 1.0),
        k("", "D", 4.5, 3.25, 1.0, 1.0),
        k("", "F", 5.75, 3.25, 1.0, 1.0),
        k("", "G", 7.0, 3.25, 1.0, 1.0),
        k("", "H", 8.25, 3.25, 1.0, 1.0),
        k("", "J", 9.5, 3.25, 1.0, 1.0),
        k("", "K", 10.75, 3.25, 1.0, 1.0),
        k("", "L", 12.0, 3.25, 1.0, 1.0),
        k(":", ";", 13.25, 3.25, 1.0, 1.0),
        k("\"", "'", 14.5, 3.25, 1.0, 1.0),
        k("", "Enter", 15.75, 3.25, 2.5, 1.0),
        k("", "Shift", 0.0, 4.5, 2.5, 1.0),
        k("", "Z", 2.75, 4.5, 1.0, 1.0),
        k("", "X", 4.0, 4.5, 1.0, 1.0),
        k("", "C", 5.25, 4.5, 1.0, 1.0),
        k("", "V", 6.5, 4.5, 1.0, 1.0),
        k("", "B", 7.75, 4.5, 1.0, 1.0),
        k("", "N", 9.0, 4.5, 1.0, 1.0),
        k("", "M", 10.25, 4.5, 1.0, 1.0),
        k("<", ",", 11.5, 4.5, 1.0, 1.0),
        k(">", ".", 12.75, 4.5, 1.0, 1.0),
        k("?", "/", 14.0, 4.5, 1.0, 1.0),
        k("", "Shift", 15.25, 4.5, 3.0, 1.0),
        k("", "Ctrl", 0.0, 5.75, 1.25, 1.0),
        k("", "Fn", 1.5, 5.75, 1.0, 1.0),
        k("", "Super", 2.75, 5.75, 1.0, 1.0),
        k("", "Alt", 4.0, 5.75, 1.0, 1.0),
        k("", "", 5.25, 5.75, 6.0, 1.0),
        k("", "Alt", 11.5, 5.75, 1.0, 1.0),
        k("", "Ctrl", 12.75, 5.75, 1.0, 1.0),
        k("", "\u{2190}", 14.0, 5.75, 1.25, 1.0),
        k("", "\u{2191}", 15.5, 5.75, 1.25, 0.5),
        k("", "\u{2192}", 17.0, 5.75, 1.25, 1.0),
        k("", "\u{2193}", 15.5, 6.25, 1.25, 0.5),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideModule {
    None,
    #[default]
    Macropad,
    Numpad,
}

const ROWS: [(f32, f32); 6] = [(0.0, 0.75), (1.0, 1.25), (2.5, 1.25), (4.0, 1.25), (5.5, 1.25), (7.0, 1.25)];
const NOTE: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// MIDI note name with middle C = C4 = 60.
pub fn note_name(n: u8) -> String {
    format!("{}{}", NOTE[(n % 12) as usize], n as i32 / 12 - 1)
}

/// What a macropad key sends by default: (layer A, layer B).
pub fn macropad_sends(i: usize) -> (String, String) {
    let n = 48 + i as u8;
    (
        format!("MIDI note {n} ({}), channel 1", note_name(n)),
        crate::keycodes::label(crate::keycodes::MACROPAD_DEFAULT[1][i]),
    )
}

/// RGB macropad keys, legends for layer 0 (MIDI) or 1 (numpad).
pub fn macropad_keys(layer: u8) -> Vec<KeyDef> {
    (0..24)
        .map(|i| {
            let (r, c) = (i / 4, i % 4);
            let (top, main) = if layer == 0 {
                let n = 48 + i as u8;
                (n.to_string(), note_name(n))
            } else {
                (String::new(), crate::keycodes::short(crate::keycodes::MACROPAD_DEFAULT[1][i]))
            };
            KeyDef { top, main, x: c as f32 * 1.25, y: ROWS[r].0, w: 1.0, h: ROWS[r].1 }
        })
        .collect()
}

/// Framework numpad (white backlight), default legends.
pub fn numpad_keys() -> Vec<KeyDef> {
    let l = |main: &str, c: usize, r: usize, w: f32, h: f32| {
        k("", main, c as f32 * 1.25, ROWS[r].0, w, if h > 0.0 { h } else { ROWS[r].1 })
    };
    vec![
        l("Esc", 0, 0, 1.0, 0.0),
        l("Calc", 1, 0, 1.0, 0.0),
        l("=", 2, 0, 1.0, 0.0),
        l("\u{232B}", 3, 0, 1.0, 0.0),
        l("Num", 0, 1, 1.0, 0.0),
        l("/", 1, 1, 1.0, 0.0),
        l("*", 2, 1, 1.0, 0.0),
        l("\u{2212}", 3, 1, 1.0, 0.0),
        l("7", 0, 2, 1.0, 0.0),
        l("8", 1, 2, 1.0, 0.0),
        l("9", 2, 2, 1.0, 0.0),
        l("+", 3, 2, 1.0, 2.75),
        l("4", 0, 3, 1.0, 0.0),
        l("5", 1, 3, 1.0, 0.0),
        l("6", 2, 3, 1.0, 0.0),
        l("1", 0, 4, 1.0, 0.0),
        l("2", 1, 4, 1.0, 0.0),
        l("3", 2, 4, 1.0, 0.0),
        l("Enter", 3, 4, 1.0, 2.75),
        l("0", 0, 5, 2.25, 0.0),
        l(".", 2, 5, 1.0, 0.0),
    ]
}

pub fn module_keys(side: SideModule, layer: u8) -> Vec<KeyDef> {
    match side {
        SideModule::None => vec![],
        SideModule::Macropad => macropad_keys(layer),
        SideModule::Numpad => numpad_keys(),
    }
}

/// Key centre in QMK's 0..224 x 0..64 LED space for a device of `size` units.
pub fn qmk_point(key: &KeyDef, size: (f32, f32)) -> (u8, u8) {
    let (cx, cy) = key.center();
    ((cx / size.0 * 224.0).round().clamp(0.0, 224.0) as u8, (cy / size.1 * 64.0).round().clamp(0.0, 64.0) as u8)
}

/// For every LED, the key it sits under: the key whose rectangle (in QMK
/// space) contains it, else the nearest key centre. Keys can own several
/// LEDs (the space bar has more than one).
pub fn map_leds_to_keys(leds: &[LedPoint], keys: &[KeyDef], size: (f32, f32)) -> Vec<usize> {
    let sx = 224.0 / size.0;
    let sy = 64.0 / size.1;
    leds.iter()
        .map(|p| {
            let (px, py) = (p.x as f32, p.y as f32);
            if let Some(i) = keys.iter().position(|k| {
                px >= k.x * sx - 1.0
                    && px <= (k.x + k.w) * sx + 1.0
                    && py >= k.y * sy - 1.0
                    && py <= (k.y + k.h) * sy + 1.0
            }) {
                return i;
            }
            keys.iter()
                .enumerate()
                .map(|(i, k)| {
                    let (cx, cy) = k.center();
                    (i, (cx * sx - px).powi(2) + (cy * sy - py).powi(2))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .unwrap_or(0)
        })
        .collect()
}

/// LED positions from Framework's QMK v0.3.1 (ansi/ansi.c).
pub const KEYBOARD_LEDS: [(u8, u8); 97] = [
    (11, 23),
    (0, 23),
    (57, 22),
    (41, 22),
    (73, 22),
    (25, 22),
    (121, 22),
    (89, 22),
    (105, 22),
    (82, 10),
    (114, 10),
    (66, 10),
    (50, 10),
    (34, 10),
    (98, 10),
    (18, 10),
    (2, 10),
    (130, 10),
    (56, 0),
    (40, 0),
    (72, 0),
    (24, 0),
    (88, 0),
    (10, 0),
    (104, 0),
    (0, 0),
    (120, 0),
    (38, 47),
    (22, 47),
    (54, 47),
    (5, 48),
    (70, 47),
    (1, 48),
    (86, 47),
    (0, 60),
    (102, 47),
    (30, 34),
    (62, 34),
    (7, 35),
    (94, 34),
    (110, 34),
    (46, 34),
    (13, 36),
    (78, 34),
    (1, 36),
    (8, 60),
    (38, 59),
    (54, 59),
    (174, 34),
    (126, 34),
    (142, 34),
    (158, 34),
    (190, 34),
    (205, 34),
    (202, 22),
    (219, 22),
    (223, 36),
    (185, 22),
    (137, 22),
    (153, 22),
    (169, 22),
    (162, 10),
    (194, 10),
    (178, 10),
    (209, 11),
    (222, 11),
    (181, 0),
    (136, 0),
    (168, 0),
    (146, 10),
    (199, 0),
    (186, 0),
    (213, 0),
    (224, 0),
    (152, 0),
    (166, 47),
    (118, 47),
    (134, 47),
    (150, 47),
    (182, 47),
    (197, 48),
    (202, 58),
    (203, 48),
    (217, 48),
    (150, 59),
    (108, 60),
    (121, 60),
    (134, 60),
    (166, 59),
    (183, 64),
    (202, 62),
    (220, 64),
    (222, 48),
    (22, 59),
    (70, 60),
    (83, 60),
    (95, 60),
];

/// LED positions from Framework's QMK v0.3.1 (macropad/macropad.c).
pub const MACROPAD_LEDS: [(u8, u8); 24] = [
    (73, 10),
    (73, 24),
    (73, 0),
    (73, 37),
    (0, 10),
    (0, 0),
    (0, 37),
    (0, 24),
    (0, 64),
    (0, 50),
    (73, 64),
    (73, 50),
    (223, 64),
    (223, 50),
    (150, 64),
    (150, 50),
    (223, 24),
    (223, 0),
    (223, 10),
    (223, 37),
    (150, 10),
    (150, 24),
    (150, 0),
    (150, 37),
];

/// LED tables as LedPoints (flags = 4, "key light").
pub fn firmware_leds(table: &[(u8, u8)]) -> Vec<LedPoint> {
    table.iter().map(|&(x, y)| LedPoint { x, y, flags: 4 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_has_78_keys_inside_its_outline() {
        let keys = keyboard_keys();
        assert_eq!(keys.len(), 78);
        for k in &keys {
            assert!(k.x >= 0.0 && k.x + k.w <= KEYBOARD_SIZE.0 + 1e-3, "{}", k.main);
            assert!(k.y >= 0.0 && k.y + k.h <= KEYBOARD_SIZE.1 + 1e-3, "{}", k.main);
        }
        assert!(keys.iter().any(|k| k.main == "Super"));
        assert!(!keys.iter().any(|k| k.main == "Win"));
    }

    #[test]
    fn modules_have_their_key_counts() {
        assert_eq!(macropad_keys(0).len(), 24);
        assert_eq!(numpad_keys().len(), 21);
        assert!(module_keys(SideModule::None, 0).is_empty());
        assert_eq!(macropad_keys(0)[0].main, "C3");
        assert_eq!(macropad_keys(0)[23].top, "71");
        assert_eq!(macropad_keys(1)[0].main, "Esc");
        assert_eq!(macropad_keys(1)[8].main, "7");
        assert_eq!(macropad_sends(4).1, "Num Lock");
    }

    #[test]
    fn every_keyboard_key_gets_an_led() {
        let keys = keyboard_keys();
        let map = map_leds_to_keys(&firmware_leds(&KEYBOARD_LEDS), &keys, KEYBOARD_SIZE);
        assert_eq!(map.len(), 97);
        for (i, key) in keys.iter().enumerate() {
            assert!(map.contains(&i), "key {} has no LED", key.main);
        }
    }

    #[test]
    fn every_macropad_key_gets_one_led() {
        let keys = macropad_keys(0);
        let map = map_leds_to_keys(&firmware_leds(&MACROPAD_LEDS), &keys, MODULE_GRID);
        let mut seen = map.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 24, "{map:?}");
    }

    #[test]
    fn modifiers_are_detected() {
        let keys = keyboard_keys();
        let find = |m: &str| keys.iter().find(|k| k.main == m).unwrap().is_modifier();
        assert!(find("Shift"));
        assert!(!find("A"));
        assert!(!find("F5"));
    }
}
