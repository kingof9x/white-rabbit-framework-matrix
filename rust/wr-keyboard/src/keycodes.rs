//! What each key sends: QMK key codes, names for them, and the rules for changing the
//! keyboard's and macropad's keymaps (VIA's dynamic keymap, saved on the device).
//!
//! A QMK key code is a 16-bit number. The low byte is a USB HID key (0x04 = A, 0x3A = F1...); the
//! bits 0x0100..0x1000 add modifiers held with it (Ctrl, Shift, Alt, Super, and "right-hand");
//! higher ranges are QMK features: layer keys (MO(n), 0x5220 + n), lighting keys (0x78xx) and MIDI
//! notes (0x71xx). The generated tables in `keymap_data.rs` hold the names, matrix positions and
//! default keymaps; this file adds the logic.
//!
//! Who uses it: the window's Key mapping and Macropad pages (names, which layers to write), the
//! engine and [`crate::device::set_fn_lock`] (applying writes, repairing the top row), and the
//! simulator (default keymaps). Pure functions; nothing here talks to a device.
//!
//! The keyboard's four layers and Fn lock are explained at [`Keymap::edit_layers`] and
//! [`Keymap::top_row_fixes`].

pub use crate::keymap_data::*;
use crate::keys::note_name;

/// Number of "Open an app" keys. Each sends Ctrl+Shift+Alt+F13 to F24, which the app
/// catches and turns into starting a program.
pub const APP_SLOTS: usize = 12;
/// The modifiers every "Open an app" key holds (Ctrl+Shift+Alt): a combination no normal program
/// uses with F13-F24, so the app's keyboard hook can claim it safely.
const APP_MODS: u16 = MOD_CTRL | MOD_SHIFT | MOD_ALT;

/// Modifier bits inside a QMK key code (QK_MODS range): left Ctrl.
pub const MOD_CTRL: u16 = 0x0100;
/// Left Shift.
pub const MOD_SHIFT: u16 = 0x0200;
/// Left Alt.
pub const MOD_ALT: u16 = 0x0400;
/// Left Super (the Windows key).
pub const MOD_SUPER: u16 = 0x0800;
/// Set together with the others to mean the right-hand modifier. The app treats left and right
/// alike, so it is masked off when showing a key.
const MOD_RIGHT: u16 = 0x1000;
/// All modifier bits (Ctrl, Shift, Alt, Super, right-hand).
const MOD_MASK: u16 = 0x1F00;

/// USB HID usage of F1; F2..F12 follow it one by one.
pub const KC_F1: u16 = 0x003A;
/// USB HID usage of F12.
pub const KC_F12: u16 = 0x0045;

/// A plain F1 to F24 key (no modifiers held).
/// Used by the top-row repair to tell "the F-key job" from "the shortcut job".
pub fn is_f_key(code: u16) -> bool {
    (KC_F1..=KC_F12).contains(&code) || (KC_F13..=KC_F24).contains(&code)
}

/// Key code of an "Open an app" key.
/// `slot` 0..12 gives Ctrl+Shift+Alt+F13 ... F24.
pub fn app_code(slot: usize) -> u16 {
    APP_MODS | (KC_F13 + slot as u16)
}

/// Which "Open an app" key this code is, if it is one.
/// Exactly the Ctrl+Shift+Alt modifiers on F13..F24; any other modifier combination isn't one.
pub fn app_slot(code: u16) -> Option<usize> {
    // Low byte = the key itself, high bits = modifiers.
    let base = code & 0xFF;
    (code & MOD_MASK == APP_MODS && (KC_F13..=KC_F24).contains(&base)).then(|| (base - KC_F13) as usize)
}

/// A plain key with modifiers held: (key, modifier bits). Right-hand modifiers count as left.
/// None for codes above the modifier range (layer keys, lighting, MIDI...), which can't hold
/// modifiers.
pub fn split_mods(code: u16) -> Option<(u16, u16)> {
    // Codes up to QK_MODS_MAX are "basic key + modifier bits"; keep the bits, drop "right-hand".
    (code <= QK_MODS_MAX).then_some((code & 0xFF, code & (MOD_MASK & !MOD_RIGHT)))
}

/// MIDI note number of a macropad MIDI key (MI_C on the macropad is note 48).
pub fn midi_note(code: u16) -> Option<u8> {
    // QMK's MIDI note keys are consecutive codes; the macropad's lowest key (C3) is note 48.
    (MIDI_NOTE_FIRST..=MIDI_NOTE_LAST).contains(&code).then(|| 48 + (code - MIDI_NOTE_FIRST) as u8)
}

/// The key code for MIDI note `note`; None outside the macropad's range (48 up to 119).
pub fn midi_code(note: u8) -> Option<u16> {
    let c = MIDI_NOTE_FIRST + note.checked_sub(48)? as u16;
    (c <= MIDI_NOTE_LAST).then_some(c)
}

/// The entry of the app's key list for `code`, if it has one (a linear search of ~150 entries).
pub fn named(code: u16) -> Option<&'static Named> {
    NAMED.iter().find(|n| n.code == code)
}

/// QMK lighting keys (next effect, brighter...): QK_LIGHTING..=QK_LIGHTING_MAX.
pub fn is_lighting(code: u16) -> bool {
    (QK_LIGHTING..=QK_LIGHTING_MAX).contains(&code)
}

/// The names of the modifiers in `mods`, in the order Ctrl, Shift, Alt, Super.
fn mod_names(mods: u16) -> Vec<&'static str> {
    [(MOD_CTRL, "Ctrl"), (MOD_SHIFT, "Shift"), (MOD_ALT, "Alt"), (MOD_SUPER, "Super")]
        .iter()
        .filter(|(m, _)| mods & m != 0)
        .map(|(_, n)| *n)
        .collect()
}

/// A short legend for a keycap. `KC_TRNS` gives an empty string.
/// Falls back from special keys to named keys to modifier combinations ("C+S+Z") and finally the
/// raw code in hex, so every code shows something.
pub fn short(code: u16) -> String {
    if code == KC_NO {
        return "Off".into();
    }
    if code == KC_TRNS {
        return String::new();
    }
    if let Some(s) = app_slot(code) {
        return format!("App {}", s + 1);
    }
    if let Some(n) = midi_note(code) {
        return note_name(n);
    }
    if let Some(n) = named(code) {
        return n.short.into();
    }
    // MO(n): hold for layer n. On the keyboard MO(1) is the Fn key.
    if (QK_MOMENTARY..QK_MOMENTARY + 32).contains(&code) {
        return if code == QK_MOMENTARY + 1 { "Fn".into() } else { format!("Layer {}", code - QK_MOMENTARY) };
    }
    if let Some((base, mods)) = split_mods(code) {
        if mods != 0 {
            let mut parts: Vec<String> =
                mod_names(mods).iter().map(|m| m.chars().next().unwrap().to_string()).collect();
            parts.push(short(base));
            return parts.join("+");
        }
    }
    format!("{code:04X}")
}

/// A full description, for lists and the selected-key card.
/// Same order of checks as [`short`], with longer words ("Ctrl + Z").
pub fn label(code: u16) -> String {
    if code == KC_NO {
        return "Nothing (turned off)".into();
    }
    if code == KC_TRNS {
        return "Same as without Fn".into();
    }
    if let Some(s) = app_slot(code) {
        return format!("Open app {}", s + 1);
    }
    if let Some(n) = midi_note(code) {
        return format!("MIDI note {n} ({})", note_name(n));
    }
    if let Some(n) = named(code) {
        return n.label.into();
    }
    if code == QK_MOMENTARY + 1 {
        return "Fn".into();
    }
    if let Some((base, mods)) = split_mods(code) {
        if mods != 0 {
            let mut parts: Vec<String> = mod_names(mods).iter().map(|s| s.to_string()).collect();
            parts.push(label(base));
            return parts.join(" + ");
        }
    }
    format!("Key code {code:04X}")
}

// ---------------------------------------------------------------------------
// Keymaps

/// Which device a keymap belongs to (its matrix, layer count and defaults differ).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Board {
    Keyboard,
    Macropad,
}

impl Board {
    /// (matrix row, column) of each key, in the app's key order (index = key number).
    pub fn matrix(self) -> &'static [(u8, u8)] {
        match self {
            Board::Keyboard => &KEYBOARD_MATRIX,
            Board::Macropad => &MACROPAD_MATRIX,
        }
    }
    /// (rows, columns) of the switch matrix: keyboard 8 x 16, macropad 4 x 8.
    pub fn matrix_size(self) -> (u8, u8) {
        match self {
            Board::Keyboard => KEYBOARD_MATRIX_SIZE,
            Board::Macropad => MACROPAD_MATRIX_SIZE,
        }
    }
    /// Keymap layers the app reads and writes.
    /// Keyboard: base, Fn, Fn lock, Fn while locked. Macropad: A (MIDI) and B (numpad).
    pub fn layers(self) -> u8 {
        match self {
            Board::Keyboard => 4,
            Board::Macropad => 2,
        }
    }
    /// Keys the app knows: 78 on the keyboard, 24 on the macropad.
    pub fn key_count(self) -> usize {
        self.matrix().len()
    }
    /// Bytes per layer in VIA's keymap buffer (2 per matrix position, big endian).
    pub fn layer_bytes(self) -> usize {
        let (r, c) = self.matrix_size();
        r as usize * c as usize * 2
    }
    /// The f16custom firmware's default code for `key` on `layer`. Panics on a layer or key number
    /// out of range (callers loop over `layers()` and `key_count()`).
    pub fn default_code(self, layer: u8, key: usize) -> u16 {
        match self {
            Board::Keyboard => KEYBOARD_DEFAULT[layer as usize][key],
            Board::Macropad => MACROPAD_DEFAULT[layer as usize][key],
        }
    }
}

/// The keymap layers read from a device: `layers[layer][key]`, keys in the app's order.
/// The app's copy of a device's keymap. Read once when the device is found and then kept in step
/// with every write the app makes (see [`Keymap::apply`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    /// Which device this is.
    pub board: Board,
    /// One list of key codes per layer, in key order.
    pub layers: Vec<Vec<u16>>,
}

/// One key write: layer, matrix row, matrix column, key code.
/// Turned into [`crate::protocol::set_keycode`] messages; the device saves each one at once.
pub type KeyWrite = (u8, u8, u8, u16);

impl Keymap {
    /// The keymap a freshly flashed device has.
    pub fn defaults(board: Board) -> Self {
        Self {
            board,
            layers: (0..board.layers())
                .map(|l| (0..board.key_count()).map(|k| board.default_code(l, k)).collect())
                .collect(),
        }
    }

    /// Builds the keymap from VIA's raw keymap buffer (all layers back to back).
    /// Each layer is rows x columns x 2 bytes (big endian); only the matrix positions of real keys
    /// are picked out. None if the buffer is shorter than all the layers.
    pub fn from_buffer(board: Board, buf: &[u8]) -> Option<Self> {
        let (_, cols) = board.matrix_size();
        let per = board.layer_bytes();
        if buf.len() < per * board.layers() as usize {
            return None;
        }
        let layers = (0..board.layers() as usize)
            .map(|l| {
                board
                    .matrix()
                    .iter()
                    .map(|&(r, c)| {
                        let at = l * per + (r as usize * cols as usize + c as usize) * 2;
                        u16::from_be_bytes([buf[at], buf[at + 1]])
                    })
                    .collect()
            })
            .collect();
        Some(Self { board, layers })
    }

    /// The layers a change to `key` writes, for what the user sees as `view`
    /// (keyboard: 0 = normal, 1 = with Fn held; macropad: 0 = layer A, 1 = layer B).
    ///
    /// The keyboard has four layers: 0 base, 1 Fn, 2 Fn lock (only F1-F12 set, the rest falls
    /// through to base) and 3 Fn held while Fn lock is on (base's top row, Fn's other keys).
    /// Keeping them in step means Fn lock keeps swapping the two meanings of the top row.
    ///
    /// Which layers each case writes: a top-row key without Fn is layers 0 and 3, with Fn layers 1
    /// and 2; any other key without Fn is layer 0 only, with Fn layers 1 and 3. The first layer in the
    /// list is the one shown for that view.
    pub fn edit_layers(&self, view: u8, key: usize) -> Vec<u8> {
        match self.board {
            Board::Macropad => vec![view.min(1)],
            Board::Keyboard => {
                // Keys 1..=12 in the app's order are F1..F12 (key 0 is Esc).
                let f_row = (1..=12).contains(&key);
                match (view, f_row) {
                    // Without Fn: base, plus layer 3 (Fn while locked shows the base top row).
                    (0, true) => vec![0, 3],
                    (0, false) => vec![0],
                    // With Fn: Fn layer, plus layer 2 (Fn lock shows the Fn top row on its own).
                    (_, true) => vec![1, 2],
                    // Other keys with Fn: Fn layer, plus layer 3 so Fn works the same while Fn lock is on.
                    (_, false) => vec![1, 3],
                }
            }
        }
    }

    /// What `key` does in `view`. On the keyboard's Fn view a see-through key shows as `KC_TRNS`.
    pub fn code(&self, view: u8, key: usize) -> u16 {
        // Every view writes at least one layer, and the first one is the one shown.
        let l = self.edit_layers(view, key)[0];
        self.layers[l as usize][key]
    }

    /// What the key ends up sending in `view` (see-through Fn keys resolve to the normal key).
    pub fn effective(&self, view: u8, key: usize) -> u16 {
        match self.code(view, key) {
            KC_TRNS => self.code(0, key),
            c => c,
        }
    }

    /// Whether `key` differs from the default in `view` (on any of the layers that view writes).
    pub fn is_changed(&self, view: u8, key: usize) -> bool {
        self.edit_layers(view, key).iter().any(|&l| self.layers[l as usize][key] != self.board.default_code(l, key))
    }

    /// Writes that set `key` in `view` to `code`, or back to the default with `None`.
    pub fn writes(&self, view: u8, key: usize, code: Option<u16>) -> Vec<KeyWrite> {
        let (r, c) = self.board.matrix()[key];
        self.edit_layers(view, key)
            .into_iter()
            .map(|l| (l, r, c, code.unwrap_or(self.board.default_code(l, key))))
            .collect()
    }

    /// Writes that put every key on every layer back to the default.
    pub fn reset_writes(&self) -> Vec<KeyWrite> {
        let mut out = vec![];
        for (l, layer) in self.layers.iter().enumerate() {
            for (k, &code) in layer.iter().enumerate() {
                let d = self.board.default_code(l as u8, k);
                if code != d {
                    let (r, c) = self.board.matrix()[k];
                    out.push((l as u8, r, c, d));
                }
            }
        }
        out
    }

    /// Writes that set every changed key (on every layer) to what it is now: a copy of the
    /// user's changes, to put back after something resets the keymap.
    pub fn changes(&self) -> Vec<KeyWrite> {
        let mut out = vec![];
        for (l, layer) in self.layers.iter().enumerate() {
            for (k, &code) in layer.iter().enumerate() {
                if code != self.board.default_code(l as u8, k) {
                    let (r, c) = self.board.matrix()[k];
                    out.push((l as u8, r, c, code));
                }
            }
        }
        out
    }

    /// Keyboard only: writes that put the top row (F1 to F12) the way Fn lock expects it, so
    /// the Top row setting really decides what a key does on its own.
    ///
    /// Fn lock swaps the base and Fn layers for the top row, so they must hold the two jobs the
    /// right way round: the shortcut (volume, brightness...) on the base layer (0) and the
    /// F-key on the Fn layer (1), with layer 2 (Fn lock) a copy of 1 and layer 3 (Fn while
    /// locked) a copy of 0. A key whose F-key ended up on the base layer (changed by hand, or
    /// carried over from older firmware) is swapped back, or gets its default shortcut back
    /// if both layers send an F-key (or the base layer is see-through). Anything else the user
    /// set is kept.
    pub fn top_row_fixes(&self) -> Vec<KeyWrite> {
        if self.board != Board::Keyboard || self.layers.len() < 4 {
            return vec![];
        }
        let mut out = vec![];
        for k in 1..=12 {
            // See-through on the base layer means nothing to fall back to: use the default
            let base = match self.layers[0][k] {
                KC_TRNS => self.board.default_code(0, k),
                c => c,
            };
            // See-through on the Fn layer falls back to the base layer's job.
            let fn_code = match self.layers[1][k] {
                KC_TRNS => base,
                c => c,
            };
            // Three cases: the F-key is on the base layer and the shortcut on Fn -> swap them; both are
            // F-keys -> put the default shortcut back on base; otherwise the order is right -> keep both.
            let (want_base, want_fn) = if is_f_key(base) && !is_f_key(fn_code) {
                (fn_code, base)
            } else if is_f_key(base) {
                (self.board.default_code(0, k), fn_code)
            } else {
                (base, fn_code)
            };
            let (r, c) = self.board.matrix()[k];
            // Layer 2 copies layer 1 and layer 3 copies layer 0; only write what actually differs, so a
            // healthy keymap gives no writes at all.
            for (l, want) in [(0u8, want_base), (1, want_fn), (2, want_fn), (3, want_base)] {
                if self.layers[l as usize][k] != want {
                    out.push((l, r, c, want));
                }
            }
        }
        out
    }

    /// Updates this copy after `writes` were sent to the device. Writes for matrix positions or
    /// layers this copy doesn't have are skipped.
    pub fn apply(&mut self, writes: &[KeyWrite]) {
        for &(l, r, c, code) in writes {
            // Writes carry matrix positions; find which key number that is.
            if let Some(k) = self.board.matrix().iter().position(|&p| p == (r, c)) {
                if let Some(slot) = self.layers.get_mut(l as usize).and_then(|layer| layer.get_mut(k)) {
                    *slot = code;
                }
            }
        }
    }

    /// "Open an app" slots used by this keymap.
    /// Used to decide whether "Start with Windows" is needed and which programs to watch for.
    pub fn app_slots(&self) -> Vec<usize> {
        self.layers.iter().flatten().filter_map(|&c| app_slot(c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `changes()` captures a user change completely, so it can be put back on a fresh keymap.
    #[test]
    fn changes_come_back_after_a_reset() {
        let mut km = Keymap::defaults(Board::Keyboard);
        assert!(km.changes().is_empty());
        let w = km.writes(1, 12, Some(app_code(0)));
        km.apply(&w);
        let saved = km.changes();
        assert_eq!(saved.len(), 2, "Fn + F12 lives on two layers");
        let mut fresh = Keymap::defaults(Board::Keyboard);
        fresh.apply(&saved);
        assert_eq!(fresh, km);
    }
    use crate::keys::{keyboard_keys, macropad_keys};

    /// The generated default keymap agrees with the drawn keyboard: letters, digits, arrows, Fn and
    /// the F1-F12 of the Fn layer.
    #[test]
    fn default_base_layer_matches_the_key_legends() {
        let keys = keyboard_keys();
        for (i, k) in keys.iter().enumerate() {
            let code = KEYBOARD_DEFAULT[0][i];
            let main = k.main.as_str();
            // Letters, digits and arrows must line up exactly; the top row is media keys.
            if main.len() == 1 && main.chars().all(|c| c.is_ascii_alphanumeric())
                || main.starts_with(['\u{2190}', '\u{2191}', '\u{2192}', '\u{2193}'])
            {
                assert_eq!(short(code), main, "key {i}");
            }
        }
        assert_eq!(label(KEYBOARD_DEFAULT[0][0]), "Esc");
        assert_eq!(short(KEYBOARD_DEFAULT[0][13]), "Del");
        let fn_key = keys.iter().position(|k| k.main == "Fn").unwrap();
        assert_eq!(short(KEYBOARD_DEFAULT[0][fn_key]), "Fn");
        // Fn layer top row is F1..F12
        for (f, &code) in KEYBOARD_DEFAULT[1].iter().enumerate().take(13).skip(1) {
            assert_eq!(short(code), format!("F{f}"));
        }
    }

    /// The macropad's default layers agree with its drawn legends (MIDI notes on A, numpad on B).
    #[test]
    fn default_macropad_matches_the_key_legends() {
        for (i, k) in macropad_keys(0).iter().enumerate() {
            assert_eq!(short(MACROPAD_DEFAULT[0][i]), k.main, "key {i}");
        }
        // Layer B: Framework's numpad layout
        assert_eq!(label(MACROPAD_DEFAULT[1][0]), "Esc");
        assert_eq!(label(MACROPAD_DEFAULT[1][4]), "Num Lock");
        assert_eq!(short(MACROPAD_DEFAULT[1][8]), "7");
        assert_eq!(label(MACROPAD_DEFAULT[1][23]), "Numpad Enter");
    }

    /// Every app slot code decodes back to its slot, and no default key looks like an app key.
    #[test]
    fn app_codes_round_trip_and_do_not_clash_with_defaults() {
        for s in 0..APP_SLOTS {
            assert_eq!(app_slot(app_code(s)), Some(s));
        }
        let all = KEYBOARD_DEFAULT.iter().flatten().chain(MACROPAD_DEFAULT.iter().flatten());
        assert!(all.into_iter().all(|&c| app_slot(c).is_none()));
    }

    /// Edits land on the right pairs of layers and a reset puts everything back.
    #[test]
    fn keyboard_edits_keep_fn_lock_layers_in_step() {
        let mut km = Keymap::defaults(Board::Keyboard);
        // Fn + F12 (key 12)
        let w = km.writes(1, 12, Some(app_code(0)));
        assert_eq!(w.iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2]);
        km.apply(&w);
        assert_eq!(km.code(1, 12), app_code(0));
        assert!(km.is_changed(1, 12));
        assert!(!km.is_changed(0, 12));
        // Fn + W (key 30 is W? find it)
        let w_key = keyboard_keys().iter().position(|k| k.main == "W").unwrap();
        assert_eq!(km.edit_layers(1, w_key), vec![1, 3]);
        assert_eq!(short(km.code(1, w_key)), "Light");
        // Reset puts it back
        let r = km.reset_writes();
        assert_eq!(r.len(), 2);
        km.apply(&r);
        assert_eq!(km, Keymap::defaults(Board::Keyboard));
    }

    /// A raw VIA buffer turns into key codes at the right matrix positions.
    #[test]
    fn keymap_from_buffer() {
        let board = Board::Macropad;
        let mut buf = vec![0u8; board.layer_bytes() * 2];
        let (r, c) = board.matrix()[5];
        let at = (r as usize * 8 + c as usize) * 2;
        buf[at..at + 2].copy_from_slice(&0x0004u16.to_be_bytes());
        let km = Keymap::from_buffer(board, &buf).unwrap();
        assert_eq!(km.code(0, 5), 0x0004);
        assert_eq!(km.code(0, 4), KC_NO);
    }

    /// Names for "off", modifier combinations and MIDI notes; the Fn layer has lighting keys.
    #[test]
    fn names() {
        assert_eq!(label(KC_NO), "Nothing (turned off)");
        assert_eq!(short(MOD_CTRL | MOD_SHIFT | 0x1D), "C+S+Z");
        assert_eq!(label(MOD_CTRL | 0x1D), "Ctrl + Z");
        assert_eq!(midi_code(60).and_then(midi_note), Some(60));
        assert_eq!(short(midi_code(60).unwrap()), "C4");
        assert!(is_lighting(KEYBOARD_DEFAULT[1][30]) || is_lighting(KEYBOARD_DEFAULT[1][31]));
    }
}
