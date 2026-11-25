//! The Keys page: what each keyboard and macropad key does, "Open an app" keys and
//! the macropad's corner combos. Every change is written to the device straight away
//! (VIA's keymap and the f16custom combo settings are saved in the device's EEPROM).
//!
//! Where it sits: the functions at the bottom ("What the window asks for") are called by the Slint
//! callbacks wired in `keyboard/mod.rs`, always on the UI thread with the keyboard `State` borrowed;
//! each is followed there by `push`, which copies the result back into the window's `Kb` global.
//! `decorate` is called while drawing the keyboard picture, to put each key's current job on its cap.
//!
//! How a change reaches a device: the page never talks USB itself. It builds a list of key writes
//! (`(layer, matrix row, matrix column, key code)`, see `Keymap::writes`) and sends them as
//! `Command::SetKeys` to the keyboard engine thread, which turns each into a VIA "set keycode" HID
//! request. To keep the page snappy the same writes are applied at once to the copy of the keymap
//! held in `State::status`; the engine's next status update replaces that copy with the device's.
//!
//! Key codes are QMK's 16-bit codes: 0x0000 `KC_NO` (does nothing), 0x0001 `KC_TRNS` ("see-through":
//! use the layer below), 0x0004..0x00FF plain keys, and plain keys with modifier bits added
//! (Ctrl 0x0100, Shift 0x0200, Alt 0x0400, Win 0x0800). An "Open an app" key is Ctrl+Shift+Alt plus
//! F13..F24; White Rabbit's background part catches those and starts the program saved for that slot.
//! The program paths live in the keyboard settings (`State::s.apps`), which `keyboard/mod.rs` saves
//! to disk whenever they differ from the last saved copy.

use super::State;
use crate::AppWindow;
use crate::Kb as App;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use wr_keyboard::deck::{Dev, PlacedKey};
use wr_keyboard::effects::Rgb;
use wr_keyboard::engine::{Command, DeviceStatus, Slot};
use wr_keyboard::keycodes::{
    self as kc, app_code, app_slot, label, midi_code, short, split_mods, Board, Keymap, APP_SLOTS, KC_NO, KC_TRNS,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER,
};
use wr_keyboard::keys::SideModule;
use wr_keyboard::protocol::{Combos, DeviceKind};

/// Key colors in the Keys page picture.
///
/// Orange: a key the person changed from Framework's default.
const COLOR_CHANGED: Rgb = [255, 110, 20];
/// Dim grey: a key that still does its Framework default.
const COLOR_DEFAULT: Rgb = [70, 70, 80];
/// Near black: a see-through key on the Fn view, or a device that is not connected (shown, not editable).
const COLOR_SEE_THROUGH: Rgb = [22, 22, 26];
/// Black: a key the person switched off (set to `KC_NO`).
const COLOR_OFF: Rgb = [0, 0, 0];

/// The Keys page's own state, kept inside the keyboard `State` (field `ke`). Lives on the UI thread.
#[derive(Default)]
pub struct KeyEdit {
    /// Keyboard: 0 normal, 1 Fn held.
    pub kb_view: u8,
    /// Macropad: 0 layer A, 1 layer B.
    pub pad_view: u8,
    /// The key being edited: which device and its index in the app's key order (None: nothing picked).
    pub sel: Option<(Dev, usize)>,
    /// Codes behind the "A key" list now shown, and which board it is for.
    list_board: Option<Board>,
    /// The key code behind each row of the "A key" drop-down, in the same order as the names shown.
    list: Vec<u16>,
    /// Combo action lists: index 0 = built-in, then keys, then "Open an app".
    combo_list: Vec<ComboChoice>,
    /// Whether the combo drop-downs have been filled in the window yet (they never change, so once).
    combo_list_shown: bool,
    /// The start-at-sign-in value last passed to Windows, so it is only set again when it changes.
    last_autostart: Option<bool>,
}

/// Tests for the small path helper (the rest needs a window and devices).
#[cfg(test)]
mod tests {
    /// Backslash and forward-slash paths, surrounding quotes and spaces, and an empty path.
    #[test]
    fn file_names() {
        assert_eq!(super::file_name(r"C:\Apps\Arena.exe"), "Arena.exe");
        assert_eq!(super::file_name("\"C:/x/y.lnk\" "), "y.lnk");
        assert_eq!(super::file_name(""), "");
    }
}

/// One row of a combo's action drop-down.
#[derive(Clone, Copy, PartialEq)]
enum ComboChoice {
    /// The combo's own job (top: switch layer A/B; bottom: lighting controls). Stored as action 0.
    Builtin,
    /// Tap this key code.
    Key(u16),
    /// Open a program: stored as one of the "Open an app" key codes.
    App,
}

/// The keymap layout (`Board`) for a device in the picture. The side module is always the macropad
/// here; the numpad module has no editable keymap.
fn board_of(dev: Dev) -> Board {
    match dev {
        Dev::Keyboard => Board::Keyboard,
        Dev::Module => Board::Macropad,
    }
}

/// The USB device kind for a device in the picture, used to address engine commands.
fn kind_of(dev: Dev) -> DeviceKind {
    match dev {
        Dev::Keyboard => DeviceKind::Keyboard,
        Dev::Module => DeviceKind::Macropad,
    }
}

/// The device's status if it is connected and running this app's firmware (`Slot::Ready`); None when
/// it is missing or running other firmware.
fn ready(st: &State, kind: DeviceKind) -> Option<&DeviceStatus> {
    match st.status.slot(kind) {
        Slot::Ready(d) => Some(d),
        _ => None,
    }
}

/// The file name at the end of a Windows path (works the same on any system).
///
/// Quotes and surrounding spaces are removed first, so a path pasted from Explorer's "Copy as path"
/// works. Both slash kinds count as separators. Returns "" for an empty path.
fn file_name(path: &str) -> &str {
    path.trim().trim_matches('"').rsplit(['\\', '/']).next().unwrap_or("")
}

/// The keys the "A key" list offers.
///
/// Each row is (key code, "Group: Name"). The keyboard gets every named key; the macropad leaves out
/// the "Fn" group (it has no Fn layer), and gets MIDI notes 48 to 119 instead (C3 up to B8, the
/// range the firmware's MIDI keys cover; `midi_code` returns None outside it).
fn key_list(board: Board) -> Vec<(u16, String)> {
    let mut out: Vec<(u16, String)> = kc::NAMED
        .iter()
        .filter(|n| board == Board::Keyboard || n.group != "Fn")
        .map(|n| (n.code, format!("{}: {}", n.group, n.label)))
        .collect();
    if board == Board::Macropad {
        out.extend((48..=119).filter_map(|n| midi_code(n).map(|c| (c, format!("MIDI: {}", label(c))))));
    }
    out
}

/// What a combo can do: its built-in job, a key (tapped), or opening an app.
///
/// Row 0 is the built-in job; its text is filled in later per combo (see `push`), since the top and
/// bottom combos have different built-in jobs. Fn, lighting and modifier keys are left out: a combo
/// taps one key, and tapping a modifier or a lighting code on its own does nothing useful.
fn combo_list() -> Vec<(ComboChoice, String)> {
    let mut out = vec![(ComboChoice::Builtin, String::new())];
    out.extend(
        kc::NAMED
            .iter()
            .filter(|n| !matches!(n.group, "Fn" | "Lighting" | "Modifiers"))
            .map(|n| (ComboChoice::Key(n.code), format!("{}: {}", n.group, n.label))),
    );
    out.push((ComboChoice::App, "Open an app".into()));
    out
}

impl State {
    /// The view shown for a device: keyboard 0 normal / 1 Fn held; macropad 0 layer A / 1 layer B.
    fn view(&self, dev: Dev) -> u8 {
        match dev {
            Dev::Keyboard => self.ke.kb_view,
            Dev::Module => self.ke.pad_view,
        }
    }

    /// The keymap of a device, if it is connected, readable and in the setup.
    ///
    /// None when the side module in the setup is not the macropad (for example the numpad), when the
    /// device is not connected with this app's firmware, or when its keymap couldn't be read.
    fn keymap(&self, dev: Dev) -> Option<&Keymap> {
        if dev == Dev::Module && self.s.setup.side != SideModule::Macropad {
            return None;
        }
        ready(self, kind_of(dev)).and_then(|d| d.keymap.as_ref())
    }

    /// The macropad's corner-combo settings as last read from it, or None if it is not ready.
    fn combos(&self) -> Option<Combos> {
        ready(self, DeviceKind::Macropad).and_then(|d| d.combos)
    }

    /// A short name for an "Open an app" key: the program's name.
    ///
    /// The file name without its extension, cut to 9 characters so it fits on a keycap; "App N" (1-based)
    /// when no program has been picked for that slot.
    fn app_short(&self, slot: usize) -> String {
        let name = file_name(&self.s.apps[slot]);
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        if stem.is_empty() {
            format!("App {}", slot + 1)
        } else {
            stem.chars().take(9).collect()
        }
    }

    /// The longer description of an "Open an app" key for the side card: "Open Arena.exe".
    fn app_long(&self, slot: usize) -> String {
        match file_name(&self.s.apps[slot]) {
            "" => "Open an app (no program picked yet)".into(),
            name => format!("Open {name}"),
        }
    }

    /// Keycap legend for any key code: the program name for "Open an app" keys, QMK's short name otherwise.
    fn key_short(&self, code: u16) -> String {
        app_slot(code).map_or_else(|| short(code), |s| self.app_short(s))
    }

    /// Full description for any key code, for the "Now:" line of the side card.
    fn key_long(&self, code: u16) -> String {
        app_slot(code).map_or_else(|| label(code), |s| self.app_long(s))
    }

    /// An "Open an app" slot no key or combo uses yet.
    ///
    /// Prefers a slot that no key uses and that has no program saved, then any slot no key uses (its old
    /// program path stays until replaced). If all 12 are in use it falls back to slot 0, which then gets
    /// shared with whatever already uses it.
    fn free_app_slot(&self) -> usize {
        let mut used: Vec<usize> =
            [Dev::Keyboard, Dev::Module].iter().filter_map(|&d| self.keymap(d)).flat_map(|k| k.app_slots()).collect();
        if let Some(c) = self.combos() {
            used.extend([c.top_action, c.bottom_action].iter().filter_map(|&a| app_slot(a)));
        }
        (0..APP_SLOTS)
            .find(|s| !used.contains(s) && self.s.apps[*s].is_empty())
            .or_else(|| (0..APP_SLOTS).find(|s| !used.contains(s)))
            .unwrap_or(0)
    }

    /// Tells the hotkey thread the programs, and Windows whether to start the app.
    ///
    /// In White Rabbit most of these calls are no-ops (see `winsys.rs`): the programs reach the
    /// background part through the saved keyboard settings. Only the audio shortcut flag takes effect here.
    pub fn sync_background(&mut self) {
        self.bg.set_apps(self.s.apps.clone());
        self.bg.set_tray(self.s.tray && super::HAS_TRAY);
        self.bg.set_audio_hotkey(self.s.audio_hotkey);
        let auto = self.s.autostart_on() && super::HAS_TRAY;
        if self.ke.last_autostart != Some(auto) {
            super::winsys::set_autostart(auto);
            self.ke.last_autostart = Some(auto);
        }
    }

    /// Sets what `key` does in the current view (`None`: back to Framework's default). Does nothing if the
    /// device's keymap isn't available. The writes may cover two layers (see `Keymap::edit_layers`): on
    /// the keyboard a change also goes to the matching Fn-lock layer so Fn lock keeps working.
    fn write_key(&mut self, dev: Dev, key: usize, code: Option<u16>) {
        let view = self.view(dev);
        if let Some(km) = self.keymap(dev) {
            let writes = km.writes(view, key, code);
            self.engine.send(Command::SetKeys { kind: kind_of(dev), writes: writes.clone() });
            // Show it straight away; the engine confirms on its next status update.
            if let Slot::Ready(d) = self.status.slot_mut(kind_of(dev)) {
                if let Some(km) = d.keymap.as_mut() {
                    km.apply(&writes);
                }
            }
        }
    }

    /// Sends new combo settings to the macropad (the engine writes them with one HID request and the
    /// firmware saves them) and shows them at once in the local copy.
    fn set_combos(&mut self, c: Combos) {
        self.engine.send(Command::SetCombos(c));
        if let Slot::Ready(d) = self.status.slot_mut(DeviceKind::Macropad) {
            d.combos = Some(c);
        }
    }

    /// How many keys (per view) differ from the Framework default.
    ///
    /// Counts each (key, view) pair once, over both devices, for the "N keys changed" line and the Reset
    /// button. A key changed both normally and with Fn counts twice.
    fn changed_count(&self) -> usize {
        [Dev::Keyboard, Dev::Module]
            .iter()
            .filter_map(|&d| self.keymap(d))
            .map(|km| (0..km.board.key_count()).map(|k| (0..2).filter(|&v| km.is_changed(v, k)).count()).sum::<usize>())
            .sum()
    }
}

/// Keys page picture: what each key does now, changed keys in orange.
///
/// `placed` are the keys of the picture in drawing order; this rewrites each cap's legend in place to
/// show its job in the current view and returns one fill color per key, in the same order. A device
/// that isn't connected is drawn with Framework's default keymap and the see-through color.
pub fn decorate(st: &State, placed: &mut [PlacedKey]) -> Vec<Rgb> {
    placed
        .iter_mut()
        .map(|k| {
            let board = board_of(k.dev);
            if k.dev == Dev::Module && st.s.setup.side != SideModule::Macropad {
                return COLOR_SEE_THROUGH;
            }
            // Declared outside the match so the borrowed default keymap lives long enough.
            let fallback;
            let km = match st.keymap(k.dev) {
                Some(km) => km,
                None => {
                    fallback = Keymap::defaults(board);
                    &fallback
                }
            };
            let connected = st.keymap(k.dev).is_some();
            let view = st.view(k.dev);
            let code = km.code(view, k.index);
            let effective = km.effective(view, k.index);
            let name = st.key_short(effective);
            if code == KC_TRNS {
                // Fn view: keys that do the same as without Fn stay blank
                k.def.top = String::new();
                k.def.main = String::new();
            } else if name != k.def.main {
                k.def.top = String::new();
                k.def.main = if name.is_empty() { "Space".into() } else { name };
            }
            // 0x002C is the space bar's own code: keep its cap blank, like the real key.
            if k.def.main == "Space" && effective == 0x002C {
                k.def.main = String::new();
            }
            // Color priority: not connected, then changed (off or another job), then see-through, then
            // default.
            if !connected {
                COLOR_SEE_THROUGH
            } else if km.is_changed(view, k.index) {
                if code == KC_NO {
                    COLOR_OFF
                } else {
                    COLOR_CHANGED
                }
            } else if code == KC_TRNS {
                COLOR_SEE_THROUGH
            } else {
                COLOR_DEFAULT
            }
        })
        .collect()
}

/// The side card's title for a key: "Keyboard · Fn + Q", or the macropad's layer, row and column.
/// The macropad is 4 keys wide, so row and column come from dividing the index by 4 (shown 1-based).
fn key_name(st: &State, dev: Dev, key: usize) -> String {
    match dev {
        Dev::Keyboard => {
            let d = &wr_keyboard::keys::keyboard_keys()[key];
            let main = if d.main.is_empty() { "Space".to_string() } else { d.main.clone() };
            if st.ke.kb_view == 1 {
                format!("Keyboard · Fn + {main}")
            } else {
                format!("Keyboard · {main}")
            }
        }
        Dev::Module => format!(
            "Macropad · layer {} · row {}, column {}",
            if st.ke.pad_view == 0 { "A" } else { "B" },
            key / 4 + 1,
            key % 4 + 1
        ),
    }
}

/// Puts the selected key, the lists and the combos into the window.
///
/// Called after every change on the page (and on start). Only writes to the window's `Kb` global; it
/// changes the state only to cache the drop-down lists, which are rebuilt only when the board changes.
pub fn push(app: &AppWindow, st: &mut State) {
    let g = app.global::<App>();
    g.set_keys_kb_view(st.ke.kb_view as i32);
    g.set_keys_pad_view(st.ke.pad_view as i32);
    g.set_keys_changed(st.changed_count() as i32);
    g.set_has_tray(super::HAS_TRAY);
    g.set_tray(st.s.tray);
    g.set_autostart(st.s.autostart_on());

    match st.ke.sel {
        None => g.set_key_selected(false),
        Some((dev, key)) => {
            g.set_key_selected(true);
            g.set_key_title(key_name(st, dev, key).into());
            let board = board_of(dev);
            let view = st.view(dev);
            let mut note = String::new();
            let mut editable = false;
            // Explain why a key can't be edited, most basic reason first.
            if dev == Dev::Module && st.s.setup.side == SideModule::Numpad {
                note = "The numpad's keys can't be changed by this app.".into();
            } else if ready(st, kind_of(dev)).is_none() {
                note = format!(
                    "Connect the {} to change its keys.",
                    if dev == Dev::Keyboard { "keyboard" } else { "macropad" }
                );
            } else if st.keymap(dev).is_none() {
                note = "Could not read this device's keys. Try Search again on the Firmware page.".into();
            } else {
                editable = true;
            }
            g.set_key_editable(editable);
            g.set_key_note(note.into());
            let km = st.keymap(dev).cloned().unwrap_or_else(|| Keymap::defaults(board));
            let code = km.code(view, key);
            let effective = km.effective(view, key);
            g.set_key_now(format!("Now: {}", st.key_long(effective)).into());
            // The Framework default for this view's main layer; on the Fn view this is often
            // "see-through", which is described via the normal key's job.
            let default_layer = km.edit_layers(view, key)[0];
            let default = board.default_code(default_layer, key);
            g.set_key_default(
                if default == KC_TRNS {
                    format!("Framework default: same as without Fn ({})", label(km.code(0, key)))
                } else {
                    format!("Framework default: {}", label(default))
                }
                .into(),
            );
            // The side card's four radio buttons: 0 default, 1 off, 2 a key, 3 open an app.
            let kind = if !km.is_changed(view, key) {
                0
            } else if code == KC_NO {
                1
            } else if app_slot(code).is_some() {
                3
            } else {
                2
            };
            g.set_key_kind(kind);
            if st.ke.list_board != Some(board) {
                let list = key_list(board);
                st.ke.list = list.iter().map(|(c, _)| *c).collect();
                let names: Vec<SharedString> = list.into_iter().map(|(_, n)| n.into()).collect();
                g.set_key_choices(ModelRc::new(VecModel::from(names)));
                st.ke.list_board = Some(board);
            }
            // Split "Ctrl+C" into the key (C) and the modifier bits, to select C in the list and tick
            // Ctrl. Codes above the modifier range (media, lighting, MIDI) aren't split.
            let (base, mods) = split_mods(effective).unwrap_or((effective, 0));
            let base = if mods != 0 { base } else { effective };
            g.set_key_choice(st.ke.list.iter().position(|&c| c == base).map_or(-1, |i| i as i32));
            // Modifier boxes only make sense for plain keys (codes up to 0xFF).
            g.set_key_mods_ok(base <= 0xFF && base != KC_NO);
            g.set_key_ctrl(mods & MOD_CTRL != 0);
            g.set_key_shift(mods & MOD_SHIFT != 0);
            g.set_key_alt(mods & MOD_ALT != 0);
            g.set_key_super(mods & MOD_SUPER != 0);
            g.set_key_app(app_slot(code).map(|s| st.s.apps[s].clone()).unwrap_or_default().into());
        }
    }

    // Corner combos
    if !st.ke.combo_list_shown {
        let list = combo_list();
        st.ke.combo_list = list.iter().map(|(c, _)| *c).collect();
        let names = |builtin: &str| -> ModelRc<SharedString> {
            let v: Vec<SharedString> =
                list.iter().map(|(c, n)| if *c == ComboChoice::Builtin { builtin.into() } else { n.into() }).collect();
            ModelRc::new(VecModel::from(v))
        };
        g.set_combo_top_choices(names("Switch between layer A and B (built in)"));
        g.set_combo_bottom_choices(names("Lighting controls (built in)"));
        st.ke.combo_list_shown = true;
    }
    // Show the macropad's current combos, or the defaults greyed out (`combos_known` false) when the
    // macropad isn't ready.
    let c = st.combos();
    g.set_combos_known(c.is_some());
    let c = c.unwrap_or_default();
    // Which drop-down row a stored action is: 0 is built in, an app code is the single "Open an
    // app" row, anything else its own key row (-1 if the key isn't in the list).
    let index = |action: u16| -> i32 {
        let choice = if action == 0 {
            ComboChoice::Builtin
        } else if app_slot(action).is_some() {
            ComboChoice::App
        } else {
            ComboChoice::Key(action)
        };
        st.ke.combo_list.iter().position(|&x| x == choice).map_or(-1, |i| i as i32)
    };
    g.set_corner_lights(c.corner_lights);
    g.set_combo_top_row(c.top_row);
    // The keyboard's F12 shortcuts switch is shown here too; it is a keyboard setting, not a combo.
    let sc = ready(st, DeviceKind::Keyboard).and_then(|d| d.shortcuts);
    g.set_shortcuts_known(sc.is_some());
    g.set_shortcuts_on(sc.unwrap_or(false));
    g.set_combo_top_on(c.top_on);
    g.set_combo_bottom_on(c.bottom_on);
    g.set_combo_top_choice(index(c.top_action));
    g.set_combo_bottom_choice(index(c.bottom_action));
    let app_of = |a: u16| app_slot(a).map(|s| st.s.apps[s].clone());
    g.set_combo_top_app_shown(app_of(c.top_action).is_some());
    g.set_combo_bottom_app_shown(app_of(c.bottom_action).is_some());
    g.set_combo_top_app(app_of(c.top_action).unwrap_or_default().into());
    g.set_combo_bottom_app(app_of(c.bottom_action).unwrap_or_default().into());
}

// -------------------------------------------------------------------------
// What the window asks for

/// A key on the picture was clicked: make it the one the side card edits.
pub fn select(st: &mut State, dev: Dev, key: usize) {
    st.ke.sel = Some((dev, key));
}

/// The normal/Fn (keyboard, `which` 0) or layer A/B (macropad, `which` 1) switch moved. Out of range
/// values are clamped to 0..1.
pub fn view_chosen(st: &mut State, which: i32, view: i32) {
    let v = view.clamp(0, 1) as u8;
    if which == 0 {
        st.ke.kb_view = v;
    } else {
        st.ke.pad_view = v;
    }
}

///
/// One of the side card's radio buttons: 0 back to default, 1 off, 2 a key, anything else "Open an
/// app". Writes the change to the device at once. Choosing "a key" keeps the current job when it is
/// a key the list can show, and starts from A otherwise; choosing "Open an app" keeps the key's slot
/// if it already is one, or takes a free slot.
pub fn kind_chosen(st: &mut State, kind: i32) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some(km) = st.keymap(dev) else { return };
    let view = st.view(dev);
    let effective = km.effective(view, key);
    let code = match kind {
        0 => None,
        1 => Some(KC_NO),
        2 => {
            let list = key_list(board_of(dev));
            let base = split_mods(effective).map_or(effective, |(b, m)| if m != 0 { b } else { effective });
            let keep = list.iter().any(|(c, _)| *c == base) || (base <= 0xFF && base > KC_TRNS);
            Some(if keep && app_slot(effective).is_none() {
                effective
            } else {
                0x0004 // A
            })
        }
        _ => Some(match app_slot(km.code(view, key)) {
            Some(s) => app_code(s),
            None => app_code(st.free_app_slot()),
        }),
    };
    st.write_key(dev, key, code);
}

/// The selected key's (plain key, modifier bits) in the current view, or None if nothing is selected or
/// the keymap isn't available. A code that isn't a plain key with modifiers comes back with no bits.
fn current_mods(st: &State) -> Option<(u16, u16)> {
    let (dev, key) = st.ke.sel?;
    let km = st.keymap(dev)?;
    let code = km.effective(st.view(dev), key);
    Some(split_mods(code).unwrap_or((code, 0)))
}

///
/// A row of the "A key" drop-down was picked (`index` into the cached list; a bad index does nothing).
/// Modifier boxes already ticked are kept when the new key is a plain key.
pub fn choice_chosen(st: &mut State, index: i32) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some(&code) = st.ke.list.get(index as usize) else {
        return;
    };
    let mods = current_mods(st).map_or(0, |(_, m)| m);
    let code = if code <= 0xFF && code != KC_NO { code | mods } else { code };
    st.write_key(dev, key, Some(code));
}

///
/// `which` is 0 Ctrl, 1 Shift, 2 Alt, 3 Win (clamped). Ignored unless the key is a plain key.
pub fn mod_toggled(st: &mut State, which: i32, on: bool) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some((base, mods)) = current_mods(st) else {
        return;
    };
    if base > 0xFF || base == KC_NO {
        return;
    }
    let bit = [MOD_CTRL, MOD_SHIFT, MOD_ALT, MOD_SUPER][which.clamp(0, 3) as usize];
    let mods = if on { mods | bit } else { mods & !bit };
    st.write_key(dev, key, Some(base | mods));
}

/// Stores the program for an "Open an app" slot in the settings. Saved to disk by `keyboard/mod.rs` on
/// its next tick (it saves whenever the settings differ from the last saved copy).
fn set_app_path(st: &mut State, slot: usize, path: &str) {
    st.s.apps[slot] = path.to_string();
    st.sync_background();
}

/// The program path box of the selected "Open an app" key was edited. Ignored if the selected key
/// isn't an "Open an app" key.
pub fn app_edited(st: &mut State, path: &str) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some(slot) = st.keymap(dev).and_then(|km| app_slot(km.code(st.view(dev), key))) else {
        return;
    };
    set_app_path(st, slot, path);
}

/// The "Browse…" button for the selected key's program: Windows' Open dialog, then as `app_edited`.
pub fn app_browse(st: &mut State) {
    if let Some(p) = super::winsys::browse_for_app() {
        app_edited(st, &p);
    }
}

/// The macropad's "corner keys show the layer color" switch. Ignored if the macropad isn't ready.
pub fn corner_lights_toggled(st: &mut State, on: bool) {
    if let Some(mut c) = st.combos() {
        c.corner_lights = on;
        st.set_combos(c);
    }
}

/// Whether the top combo is the whole top row (four keys) or just the two top corners.
pub fn combo_top_row_chosen(st: &mut State, on: bool) {
    if let Some(mut c) = st.combos() {
        c.top_row = on;
        st.set_combos(c);
    }
}

/// The keyboard's F12 shortcuts switch: sent to the keyboard (saved there) and shown at once.
pub fn shortcuts_toggled(st: &mut State, on: bool) {
    st.engine.send(Command::SetShortcuts(on));
    if let Slot::Ready(d) = st.status.slot_mut(DeviceKind::Keyboard) {
        d.shortcuts = Some(on);
    }
}

/// Turns the top (`which` 0) or bottom (any other value) combo on or off.
pub fn combo_toggled(st: &mut State, which: i32, on: bool) {
    let Some(mut c) = st.combos() else { return };
    if which == 0 {
        c.top_on = on;
    } else {
        c.bottom_on = on;
    }
    st.set_combos(c);
}

///
/// A row of a combo's action drop-down was picked (top when `which` is 0, else bottom). Choosing
/// "Open an app" keeps the combo's current app slot if it already has one, so its program stays.
pub fn combo_chosen(st: &mut State, which: i32, index: i32) {
    let Some(mut c) = st.combos() else { return };
    let Some(&choice) = st.ke.combo_list.get(index as usize) else {
        return;
    };
    let current = if which == 0 { c.top_action } else { c.bottom_action };
    let action = match choice {
        ComboChoice::Builtin => 0,
        ComboChoice::Key(code) => code,
        ComboChoice::App => match app_slot(current) {
            Some(s) => app_code(s),
            None => app_code(st.free_app_slot()),
        },
    };
    if which == 0 {
        c.top_action = action;
    } else {
        c.bottom_action = action;
    }
    st.set_combos(c);
}

/// The program box under a combo set to "Open an app" was edited; stored in that combo's app slot.
pub fn combo_app_edited(st: &mut State, which: i32, path: &str) {
    let Some(c) = st.combos() else { return };
    let action = if which == 0 { c.top_action } else { c.bottom_action };
    if let Some(slot) = app_slot(action) {
        set_app_path(st, slot, path);
    }
}

/// The "Browse…" button for a combo's program.
pub fn combo_app_browse(st: &mut State, which: i32) {
    if let Some(p) = super::winsys::browse_for_app() {
        combo_app_edited(st, which, &p);
    }
}

///
/// The Reset button: every key on every layer of both devices back to Framework's default, and the
/// combos back to their defaults. Program paths for "Open an app" slots are kept in the settings.
/// Like single changes, the local keymap copies are updated at once and confirmed by the engine later.
pub fn reset_all(st: &mut State) {
    for dev in [Dev::Keyboard, Dev::Module] {
        if let Some(km) = st.keymap(dev) {
            let writes = km.reset_writes();
            if !writes.is_empty() {
                st.engine.send(Command::SetKeys { kind: kind_of(dev), writes });
            }
        }
    }
    if st.combos().is_some_and(|c| c != Combos::default()) {
        st.set_combos(Combos::default());
    }
    for dev in [DeviceKind::Keyboard, DeviceKind::Macropad] {
        if let Slot::Ready(d) = st.status.slot_mut(dev) {
            if let Some(km) = d.keymap.as_mut() {
                let w = km.reset_writes();
                km.apply(&w);
            }
        }
    }
}
