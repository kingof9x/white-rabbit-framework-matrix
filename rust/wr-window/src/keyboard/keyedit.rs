//! The Keys page: what each keyboard and macropad key does, "Open an app" keys and
//! the macropad's corner combos. Every change is written to the device straight away
//! (VIA's keymap and the f16custom combo settings are saved in the device's EEPROM).

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
const COLOR_CHANGED: Rgb = [255, 110, 20];
const COLOR_DEFAULT: Rgb = [70, 70, 80];
const COLOR_SEE_THROUGH: Rgb = [22, 22, 26];
const COLOR_OFF: Rgb = [0, 0, 0];

#[derive(Default)]
pub struct KeyEdit {
    /// Keyboard: 0 normal, 1 Fn held.
    pub kb_view: u8,
    /// Macropad: 0 layer A, 1 layer B.
    pub pad_view: u8,
    pub sel: Option<(Dev, usize)>,
    /// Codes behind the "A key" list now shown, and which board it is for.
    list_board: Option<Board>,
    list: Vec<u16>,
    /// Combo action lists: index 0 = built-in, then keys, then "Open an app".
    combo_list: Vec<ComboChoice>,
    combo_list_shown: bool,
    last_autostart: Option<bool>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_names() {
        assert_eq!(super::file_name(r"C:\Apps\Arena.exe"), "Arena.exe");
        assert_eq!(super::file_name("\"C:/x/y.lnk\" "), "y.lnk");
        assert_eq!(super::file_name(""), "");
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ComboChoice {
    Builtin,
    Key(u16),
    App,
}

fn board_of(dev: Dev) -> Board {
    match dev {
        Dev::Keyboard => Board::Keyboard,
        Dev::Module => Board::Macropad,
    }
}

fn kind_of(dev: Dev) -> DeviceKind {
    match dev {
        Dev::Keyboard => DeviceKind::Keyboard,
        Dev::Module => DeviceKind::Macropad,
    }
}

fn ready(st: &State, kind: DeviceKind) -> Option<&DeviceStatus> {
    match st.status.slot(kind) {
        Slot::Ready(d) => Some(d),
        _ => None,
    }
}

/// The file name at the end of a Windows path (works the same on any system).
fn file_name(path: &str) -> &str {
    path.trim().trim_matches('"').rsplit(['\\', '/']).next().unwrap_or("")
}

/// The keys the "A key" list offers.
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
    fn view(&self, dev: Dev) -> u8 {
        match dev {
            Dev::Keyboard => self.ke.kb_view,
            Dev::Module => self.ke.pad_view,
        }
    }

    /// The keymap of a device, if it is connected, readable and in the setup.
    fn keymap(&self, dev: Dev) -> Option<&Keymap> {
        if dev == Dev::Module && self.s.setup.side != SideModule::Macropad {
            return None;
        }
        ready(self, kind_of(dev)).and_then(|d| d.keymap.as_ref())
    }

    fn combos(&self) -> Option<Combos> {
        ready(self, DeviceKind::Macropad).and_then(|d| d.combos)
    }

    /// A short name for an "Open an app" key: the program's name.
    fn app_short(&self, slot: usize) -> String {
        let name = file_name(&self.s.apps[slot]);
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        if stem.is_empty() {
            format!("App {}", slot + 1)
        } else {
            stem.chars().take(9).collect()
        }
    }

    fn app_long(&self, slot: usize) -> String {
        match file_name(&self.s.apps[slot]) {
            "" => "Open an app (no program picked yet)".into(),
            name => format!("Open {name}"),
        }
    }

    fn key_short(&self, code: u16) -> String {
        app_slot(code).map_or_else(|| short(code), |s| self.app_short(s))
    }

    fn key_long(&self, code: u16) -> String {
        app_slot(code).map_or_else(|| label(code), |s| self.app_long(s))
    }

    /// An "Open an app" slot no key or combo uses yet.
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

    fn set_combos(&mut self, c: Combos) {
        self.engine.send(Command::SetCombos(c));
        if let Slot::Ready(d) = self.status.slot_mut(DeviceKind::Macropad) {
            d.combos = Some(c);
        }
    }

    /// How many keys (per view) differ from the Framework default.
    fn changed_count(&self) -> usize {
        [Dev::Keyboard, Dev::Module]
            .iter()
            .filter_map(|&d| self.keymap(d))
            .map(|km| (0..km.board.key_count()).map(|k| (0..2).filter(|&v| km.is_changed(v, k)).count()).sum::<usize>())
            .sum()
    }
}

/// Keys page picture: what each key does now, changed keys in orange.
pub fn decorate(st: &State, placed: &mut [PlacedKey]) -> Vec<Rgb> {
    placed
        .iter_mut()
        .map(|k| {
            let board = board_of(k.dev);
            if k.dev == Dev::Module && st.s.setup.side != SideModule::Macropad {
                return COLOR_SEE_THROUGH;
            }
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
            if k.def.main == "Space" && effective == 0x002C {
                k.def.main = String::new();
            }
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
            let (base, mods) = split_mods(effective).unwrap_or((effective, 0));
            let base = if mods != 0 { base } else { effective };
            g.set_key_choice(st.ke.list.iter().position(|&c| c == base).map_or(-1, |i| i as i32));
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
    let c = st.combos();
    g.set_combos_known(c.is_some());
    let c = c.unwrap_or_default();
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

pub fn select(st: &mut State, dev: Dev, key: usize) {
    st.ke.sel = Some((dev, key));
}

pub fn view_chosen(st: &mut State, which: i32, view: i32) {
    let v = view.clamp(0, 1) as u8;
    if which == 0 {
        st.ke.kb_view = v;
    } else {
        st.ke.pad_view = v;
    }
}

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

fn current_mods(st: &State) -> Option<(u16, u16)> {
    let (dev, key) = st.ke.sel?;
    let km = st.keymap(dev)?;
    let code = km.effective(st.view(dev), key);
    Some(split_mods(code).unwrap_or((code, 0)))
}

pub fn choice_chosen(st: &mut State, index: i32) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some(&code) = st.ke.list.get(index as usize) else {
        return;
    };
    let mods = current_mods(st).map_or(0, |(_, m)| m);
    let code = if code <= 0xFF && code != KC_NO { code | mods } else { code };
    st.write_key(dev, key, Some(code));
}

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

fn set_app_path(st: &mut State, slot: usize, path: &str) {
    st.s.apps[slot] = path.to_string();
    st.sync_background();
}

pub fn app_edited(st: &mut State, path: &str) {
    let Some((dev, key)) = st.ke.sel else { return };
    let Some(slot) = st.keymap(dev).and_then(|km| app_slot(km.code(st.view(dev), key))) else {
        return;
    };
    set_app_path(st, slot, path);
}

pub fn app_browse(st: &mut State) {
    if let Some(p) = super::winsys::browse_for_app() {
        app_edited(st, &p);
    }
}

pub fn corner_lights_toggled(st: &mut State, on: bool) {
    if let Some(mut c) = st.combos() {
        c.corner_lights = on;
        st.set_combos(c);
    }
}

pub fn combo_top_row_chosen(st: &mut State, on: bool) {
    if let Some(mut c) = st.combos() {
        c.top_row = on;
        st.set_combos(c);
    }
}

pub fn shortcuts_toggled(st: &mut State, on: bool) {
    st.engine.send(Command::SetShortcuts(on));
    if let Slot::Ready(d) = st.status.slot_mut(DeviceKind::Keyboard) {
        d.shortcuts = Some(on);
    }
}

pub fn combo_toggled(st: &mut State, which: i32, on: bool) {
    let Some(mut c) = st.combos() else { return };
    if which == 0 {
        c.top_on = on;
    } else {
        c.bottom_on = on;
    }
    st.set_combos(c);
}

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

pub fn combo_app_edited(st: &mut State, which: i32, path: &str) {
    let Some(c) = st.combos() else { return };
    let action = if which == 0 { c.top_action } else { c.bottom_action };
    if let Some(slot) = app_slot(action) {
        set_app_path(st, slot, path);
    }
}

pub fn combo_app_browse(st: &mut State, which: i32) {
    if let Some(p) = super::winsys::browse_for_app() {
        combo_app_edited(st, which, &p);
    }
}

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
