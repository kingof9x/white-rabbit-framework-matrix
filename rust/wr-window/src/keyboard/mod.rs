//! The Framework 16 RGB keyboard and RGB macropad pages (Lighting, Audio show, Key mapping,
//! Macropad, Keyboard & macropad setup and the keyboard firmware), brought over from the Framework 16
//! Keyboard Control app. The devices, effects and picture live in the wr-keyboard crate.

mod flash;
mod keyedit;
mod update;
mod winsys;

/// The keyboard part keeps no notification-area icon of its own: White Rabbit's tray is the one.
pub const HAS_TRAY: bool = false;

use crate::{AppWindow, Kb as App, Legend, Theme};
use crate::{AudioLookItem, DeviceRow, EffectGroup, EffectItem};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wr_keyboard::audio::{AudioColors, KeyReact};
use wr_keyboard::builtin::qmk_hsv;
use wr_keyboard::deck::{self, Choice, Dev, KbType, Look, PlacedKey, Preview, Scene};
use wr_keyboard::effects::LinkedEffect;
use wr_keyboard::engine::{self, Command, EngineHandle, Slot, Status};
use wr_keyboard::keys::{self, SideModule, KEYBOARD_SIZE, MODULE_GRID};
use wr_keyboard::protocol::{DeviceKind, LightState, Paint, PaintMode, EFFECT_NAMES};
use wr_keyboard::settings::{self, Saved};
use wr_keyboard::{audio_src, render};

/// Built-in effects grouped for the Show page.
const GROUPS: &[(&str, &[u8])] = &[
    ("Solid", &[1, 2, 5]),
    ("Gradients and bands", &[3, 4, 6, 7, 8, 9, 10, 11]),
    ("Rainbow cycles", &[12, 13, 14, 15, 16, 17, 18, 19]),
    ("Beacons and hues", &[20, 21, 22, 25, 26, 27]),
    ("Random", &[23, 24, 28, 29, 30, 32]),
    ("React to typing", &[31, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44]),
];

pub struct State {
    engine: EngineHandle,
    bg: winsys::Background,
    ke: keyedit::KeyEdit,
    /// Firmware update in progress (which device), and what its thread reports.
    flashing: Option<DeviceKind>,
    flash_inbox: Arc<Mutex<Vec<flash::Progress>>>,
    /// Keys the user changed, saved before a firmware update (new firmware resets the keymap)
    /// and put back once the device is back: device, the keys, when the update finished.
    restore_keys: Option<(DeviceKind, Vec<wr_keyboard::keycodes::KeyWrite>, Option<Instant>)>,
    /// When the Firmware page last looked for a device waiting in flashing mode.
    last_drive_scan: Instant,
    demo: bool,
    s: Saved,
    last_saved: Saved,
    preview: Preview,
    keys: Vec<PlacedKey>,
    deck_width_units: f32,
    local_layer: u8,
    pad_key: usize,
    pc_live: bool,
    notice: String,
    adopted: Vec<DeviceKind>,
    /// A lighting change waiting to go to the devices (sent live, a few times a second).
    dirty: bool,
    last_live: Instant,
    /// LED calibration waiting to be saved on a device: (keyboard, macropad) last change time.
    cal_dirty: [Option<Instant>; 2],
    /// Plain white on both devices while matching their brightness.
    white_test: bool,
    /// The keyboard's effect at the last look (to notice the F12 shortcuts).
    last_kb_state: Option<LightState>,
    /// How long the last picture took to draw, and when it was drawn.
    draw_cost: Duration,
    last_draw: Instant,
    /// The microphones found (index 0 of the list is "Default microphone"), and the
    /// answer of a background look for them.
    mics: Vec<String>,
    mics_inbox: Arc<Mutex<Option<Vec<String>>>>,
    /// Audio mode is on (asked for; the engine says whether it really draws).
    audio_on: bool,
    /// What the app found plugged in last time (to follow module swaps).
    last_detected: Option<SideModule>,
    status: Status,
    legends: Rc<VecModel<Legend>>,
}

impl State {
    fn start_flash(&mut self, app: &AppWindow, which: i32, source: flash::Source, target: flash::Target) {
        if self.flashing.is_some() {
            return;
        }
        let kind = if which == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad };
        self.flashing = Some(kind);
        // (A device already in flashing mode has no keymap to save: keep what an earlier,
        // unfinished update saved.)
        if let Some(d) = ready(&self.status, kind) {
            self.restore_keys =
                d.keymap.as_ref().map(|km| km.changes()).filter(|w| !w.is_empty()).map(|w| (kind, w, None));
        }
        let g = app.global::<App>();
        g.set_flash_which(which);
        g.set_flash_busy(true);
        g.set_flash_failed(false);
        g.set_flash_status("Starting…".into());
        let inbox = self.flash_inbox.clone();
        let tx = self.engine.tx.clone();
        let demo = self.demo;
        let wake = app.as_weak();
        std::thread::spawn(move || {
            flash::run(
                kind,
                source,
                target,
                demo,
                || {
                    let _ = tx.send(Command::Bootloader(kind, true));
                },
                |p| {
                    inbox.lock().unwrap().push(p);
                    let _ = wake.upgrade_in_event_loop(|_| {});
                },
            );
        });
    }

    /// Shows what the firmware update thread reports, and looks for devices waiting in
    /// flashing mode while the Firmware page is open.
    fn flash_answers(&mut self, app: &AppWindow) {
        let g = app.global::<App>();
        let msgs: Vec<_> = self.flash_inbox.lock().unwrap().drain(..).collect();
        for m in msgs {
            let (failed, s) = match m {
                flash::Progress::Step(s) => {
                    g.set_flash_status(s.into());
                    continue;
                }
                flash::Progress::Done(s) => (false, s),
                flash::Progress::Failed(s) => (true, s),
            };
            if let Some(r) = self.restore_keys.as_mut().filter(|_| !failed) {
                r.2 = Some(Instant::now());
            }
            g.set_flash_status(s.into());
            g.set_flash_failed(failed);
            g.set_flash_busy(false);
            self.flashing = None;
            self.last_drive_scan = Instant::now() - Duration::from_secs(60);
            // Pick the device up again (and its new features) straight away.
            self.engine.send(Command::Rescan);
        }
        // Put the user's key changes back once the updated device is back.
        if let Some((kind, writes, Some(done))) = &self.restore_keys {
            let (kind, done) = (*kind, *done);
            if ready(&self.status, kind).is_some_and(|d| d.keymap.is_some()) {
                let n = writes.len();
                let writes = self.restore_keys.take().unwrap().1;
                self.engine.send(Command::SetKeys { kind, writes });
                let msg = format!(
                    "{} Your remapped keys were put back ({n} key setting{}).",
                    g.get_flash_status(),
                    if n == 1 { "" } else { "s" }
                );
                g.set_flash_status(msg.into());
            } else if done.elapsed() > Duration::from_secs(90) {
                self.restore_keys = None;
            }
        }
        if g.get_page() == "firmware"
            && self.flashing.is_none()
            && self.last_drive_scan.elapsed() > Duration::from_secs(2)
        {
            self.last_drive_scan = Instant::now();
            let drive = flash::waiting_drives().first().map(|d| flash::drive_name(d)).unwrap_or_default();
            if g.get_waiting_drive() != drive.as_str() {
                g.set_waiting_drive(drive.into());
            }
        }
    }
}

fn parse_hex(t: &str) -> Option<(u8, u8)> {
    let t = t.trim().trim_start_matches('#');
    if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    let rgb = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
    let (h, s, _) = deck::rgb_to_qmk_hsv(rgb);
    Some((h, s))
}

fn color(c: [u8; 3]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

fn hex(c: [u8; 3]) -> SharedString {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]).into()
}

fn ready(status: &Status, kind: DeviceKind) -> Option<&engine::DeviceStatus> {
    match status.slot(kind) {
        Slot::Ready(d) => Some(d),
        _ => None,
    }
}

impl State {
    fn layer(&self) -> u8 {
        ready(&self.status, DeviceKind::Macropad).and_then(|d| d.info.layer).unwrap_or(self.local_layer)
    }

    fn module_name(&self) -> &'static str {
        if self.s.setup.side == SideModule::Numpad {
            "Numpad"
        } else {
            "Macropad"
        }
    }

    // ---------------------------------------------------------------------
    // Sending to the devices
    // ---------------------------------------------------------------------

    fn state_for(look: &Look, d: &engine::DeviceStatus) -> LightState {
        let mode = match look.choice {
            Choice::Builtin(id) => id,
            _ => d.info.paint_only_effect,
        };
        LightState { enabled: true, mode, hue: look.hue, sat: look.sat, val: look.val, speed: look.speed }
    }

    /// Painted keys for one device, one entry per LED.
    fn paint_for(&self, dev: Dev, d: &engine::DeviceStatus) -> Vec<Paint> {
        let (defs, size) = match dev {
            Dev::Keyboard => (keys::keyboard_keys(), KEYBOARD_SIZE),
            Dev::Module => (keys::macropad_keys(self.layer()), MODULE_GRID),
        };
        keys::map_leds_to_keys(&d.leds, &defs, size).into_iter().map(|k| self.s.paint.get(&(dev, k)).copied()).collect()
    }

    fn targets(&self) -> Vec<(DeviceKind, Dev, bool)> {
        vec![
            (DeviceKind::Keyboard, Dev::Keyboard, self.s.setup.kb.rgb()),
            (DeviceKind::Macropad, Dev::Module, self.s.setup.side == SideModule::Macropad),
        ]
    }

    /// Joined: one effect across both devices.
    fn joined(&self) -> bool {
        self.s.join && self.s.setup.side != SideModule::None
    }

    /// The look the Lighting page edits.
    fn edit_look(&self) -> Look {
        if self.joined() || !self.s.edit_module {
            self.s.kb
        } else {
            self.s.module
        }
    }

    fn set_edit_look(&mut self, l: Look) {
        if self.joined() {
            self.s.kb = l;
            self.s.module = l;
        } else if self.s.edit_module {
            self.s.module = l;
        } else {
            self.s.kb = l;
        }
    }

    /// Sends the looks, the joined surface and painted keys to the devices.
    fn send_looks(&mut self, kb: Look, module: Look, join: bool, save: bool) {
        let status = self.status.clone();
        if let Choice::Pc(e) = kb.choice {
            let side = if self.s.setup.module_left {
                wr_keyboard::layout::MacropadSide::Left
            } else {
                wr_keyboard::layout::MacropadSide::Right
            };
            self.engine.send(Command::SetSide(side));
            self.engine.send(Command::SetLinked(Some(kb.pc_params(e))));
            self.pc_live = true;
        } else {
            if self.pc_live {
                self.engine.send(Command::SetLinked(None));
                self.pc_live = false;
            }
            for (kind, look) in [(DeviceKind::Keyboard, kb), (DeviceKind::Macropad, module)] {
                if let Some(d) = ready(&status, kind) {
                    self.engine.send(Command::SetState { kind, state: Self::state_for(&look, d), save });
                }
            }
            let both = ready(&status, DeviceKind::Keyboard).is_some() && ready(&status, DeviceKind::Macropad).is_some();
            let (ks, ms) = deck::span_slices(&self.s.setup);
            let on = join && both && self.s.setup.side == SideModule::Macropad;
            self.engine.send(Command::SetSpan { keyboard: on.then_some(ks), macropad: on.then_some(ms) });
        }
        for (kind, dev, rgb) in self.targets() {
            if let (Some(d), true) = (ready(&status, kind), rgb) {
                let leds = self.paint_for(dev, d);
                self.engine.send(Command::SetPaint { kind, leds, mode: self.s.paint_mode, save });
            }
        }
        let any = self.targets().iter().any(|(kind, _, _)| ready(&status, *kind).is_some());
        self.notice = if !any {
            "No device connected.".into()
        } else if save {
            "Saved on the devices: they start this way after a restart.".into()
        } else {
            String::new()
        };
    }

    fn audio_settings(&self) -> engine::AudioSettings {
        engine::AudioSettings {
            look: self.s.audio_look,
            sensitivity: self.s.audio_sensitivity,
            microphone: self.s.audio_mic,
            device: self.s.audio_device.clone(),
            colors: self.s.audio_colors,
            keys: self.s.audio_keys,
        }
    }

    /// Looks for the PC's microphones in the background; `tick` picks up the answer.
    fn find_microphones(&self) {
        let inbox = self.mics_inbox.clone();
        std::thread::spawn(move || {
            *inbox.lock().unwrap() = Some(audio_src::microphones());
        });
    }

    /// Restarts listening with the current choices, if audio mode is on.
    fn audio_settings_changed(&mut self) {
        if self.audio_on {
            let set = self.audio_settings();
            self.engine.send(Command::SetAudio(Some(set)));
        }
    }

    /// Audio mode on or off. Off puts back what the Lighting page shows.
    fn set_audio(&mut self, on: bool) {
        self.audio_on = on;
        self.engine.send(Command::SetAudio(on.then(|| self.audio_settings())));
        if !on {
            self.send_current(false);
        }
    }

    /// Per-key colors of audio mode's last frame, for the picture.
    fn audio_key_colors(&self, placed: &[PlacedKey]) -> Option<Vec<wr_keyboard::effects::Rgb>> {
        if !self.status.audio {
            return None;
        }
        let mut out = vec![[0u8; 3]; placed.len()];
        for (n, (kind, dev)) in
            [(DeviceKind::Keyboard, Dev::Keyboard), (DeviceKind::Macropad, Dev::Module)].into_iter().enumerate()
        {
            let Some(d) = ready(&self.status, kind) else {
                continue;
            };
            let colors = &self.status.audio_colors[n];
            let (defs, size) = match dev {
                Dev::Keyboard => (keys::keyboard_keys(), KEYBOARD_SIZE),
                Dev::Module => (keys::macropad_keys(self.layer()), MODULE_GRID),
            };
            for (led, key) in keys::map_leds_to_keys(&d.leds, &defs, size).into_iter().enumerate() {
                if let (Some(c), Some(i)) =
                    (colors.get(led), placed.iter().position(|p| p.dev == dev && p.index == key))
                {
                    out[i] = *c;
                }
            }
        }
        Some(out)
    }

    /// Sends what the Lighting page shows (live, or saved with `save`).
    fn send_current(&mut self, save: bool) {
        if self.white_test || self.audio_on {
            return;
        }
        let join = self.joined();
        let (kb, m) = (self.s.kb, if join { self.s.kb } else { self.s.module });
        self.send_looks(kb, m, join, save);
    }

    /// Plain white on both devices, for matching their brightness by eye.
    fn send_white(&mut self) {
        let status = self.status.clone();
        if self.pc_live {
            self.engine.send(Command::SetLinked(None));
            self.pc_live = false;
        }
        let val = self.s.kb.val.max(128);
        for kind in [DeviceKind::Keyboard, DeviceKind::Macropad] {
            if ready(&status, kind).is_some() {
                self.engine.send(Command::SetState {
                    kind,
                    state: LightState {
                        enabled: true,
                        mode: 1, // solid color
                        hue: 0,
                        sat: 0,
                        val,
                        speed: 128,
                    },
                    save: false,
                });
                self.engine.send(Command::SetPaint { kind, leds: vec![], mode: self.s.paint_mode, save: false });
            }
        }
    }

    fn scene(&self, page: &str) -> Scene<'_> {
        let joined = self.joined();
        let (kb, module) = if joined { (self.s.kb, self.s.kb) } else { (self.s.kb, self.s.module) };
        Scene {
            setup: self.s.setup,
            joined,
            kb,
            module,
            paint: &self.s.paint,
            paint_mode: self.s.paint_mode,
            macropad_layer: self.layer(),
            show_paint_marks: page == "lighting",
            selected: match page {
                "macropad" => Some((Dev::Module, self.pad_key)),
                "keys" => self.ke.sel,
                _ => None,
            },
        }
    }
}

// -------------------------------------------------------------------------
// Window <-> state
// -------------------------------------------------------------------------

/// Puts the saved choices into the window.
fn push_all(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let s = &st.s;
    g.set_kb_type(match s.setup.kb {
        KbType::ClearRgb => 0,
        KbType::BlackRgb => 1,
        KbType::White => 2,
    });
    g.set_side(match s.setup.side {
        SideModule::None => 0,
        SideModule::Macropad => 1,
        SideModule::Numpad => 2,
    });
    g.set_module_left(s.setup.module_left);
    let look = st.edit_look();
    g.set_show_effect(settings::choice_str(look.choice).into());
    g.set_show_hue(look.hue as i32);
    g.set_show_sat(look.sat as i32);
    g.set_show_val(look.val as i32);
    g.set_show_speed(look.speed as i32);
    g.set_paint_mode(match s.paint_mode {
        PaintMode::Steady => 0,
        PaintMode::Follow => 1,
        PaintMode::Tint => 2,
    });
    g.set_brush_hue(s.brush.0 as i32);
    g.set_brush_sat(s.brush.1 as i32);
    g.set_join(s.join);
    g.set_target(if st.joined() {
        0
    } else if s.edit_module && s.setup.side != SideModule::None {
        2
    } else {
        1
    });
    g.set_audio_look(s.audio_look.key().into());
    g.set_audio_sensitivity(s.audio_sensitivity as i32);
    g.set_audio_hotkey(s.audio_hotkey);
    push_derived(app, st);
}

/// Values worked out from the choices: colors, names, notes.
fn push_derived(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let s = &st.s;
    let picker = |h: u8, sat: u8| {
        (
            color(qmk_hsv(h as i32, 255, 255)),
            color(qmk_hsv(h as i32, sat as i32, 255)),
            hex(qmk_hsv(h as i32, sat as i32, 255)),
        )
    };
    let look = st.edit_look();
    let (f, p, x) = picker(look.hue, look.sat);
    g.set_show_full(f);
    g.set_show_picked(p);
    g.set_show_hex(x);
    let (f, p, x) = picker(s.brush.0, s.brush.1);
    g.set_brush_full(f);
    g.set_brush_picked(p);
    g.set_brush_hex(x);
    g.set_show_effect_name(look.choice.name().into());

    let has = s.setup.side != SideModule::None;
    g.set_has_module(has);
    g.set_module_name(st.module_name().into());
    g.set_kb_rgb(s.setup.kb.rgb());
    g.set_mod_rgb(s.setup.side == SideModule::Macropad);
    g.set_kb_about(
        match s.setup.kb {
            KbType::ClearRgb => "Clear RGB keys, 97 LEDs.",
            KbType::BlackRgb => "Black RGB keys, 97 LEDs.",
            KbType::White => "Black keys, white backlight.",
        }
        .into(),
    );
    g.set_mod_about(
        if s.setup.side == SideModule::Macropad { "Clear RGB keys, 24 LEDs." } else { "White backlight." }.into(),
    );
    let summary = |l: &Look| l.choice.name();
    g.set_effects_summary(
        if s.join && has {
            format!("Joined: {}", summary(&s.kb))
        } else if has {
            format!("Keyboard: {}  ·  {}: {}", summary(&s.kb), st.module_name(), summary(&s.module))
        } else {
            format!("Keyboard: {}", summary(&s.kb))
        }
        .into(),
    );
    let kb = match s.setup.kb {
        KbType::ClearRgb => "Clear RGB keyboard",
        KbType::BlackRgb => "Black RGB keyboard",
        KbType::White => "Black keyboard with white backlight",
    };
    let side = match s.setup.side {
        SideModule::None => "no side module".to_string(),
        SideModule::Macropad => format!("RGB macropad on the {}", if s.setup.module_left { "left" } else { "right" }),
        SideModule::Numpad => format!("numpad on the {}", if s.setup.module_left { "left" } else { "right" }),
    };
    g.set_setup_summary(format!("{kb}, {side}.").into());
    let mut notes = vec![];
    if s.setup.kb == KbType::White {
        notes.push("This keyboard has one white light for all keys: it can only change brightness, so it shows plain white and sits out of effects.");
    }
    if s.setup.side == SideModule::Numpad {
        notes.push("The numpad has one white light for all keys: it can only change brightness, so it shows plain white and sits out of effects.");
    }
    g.set_setup_note(notes.join(" ").into());
    let n = s.paint.len();
    g.set_paint_count(format!("{n} {} painted", if n == 1 { "key" } else { "keys" }).into());
    // macropad key info
    let (a, b) = keys::macropad_sends(st.pad_key);
    g.set_pad_key(format!("Row {}, column {}", st.pad_key / 4 + 1, st.pad_key % 4 + 1).into());
    g.set_pad_a(a.into());
    g.set_pad_b(b.into());
    g.set_pad_note(
        match st.pad_key {
            0 | 3 => "Top corner: press both top corners together to switch layers. Pressed alone it still works, about 40 ms later.",
            20 | 23 => "Bottom corner: press both bottom corners together for the lighting controls.",
            _ => "An ordinary key.",
        }
        .into(),
    );
}

/// Reads the window's choices back into the state.
fn pull_all(app: &AppWindow, st: &mut State) {
    let g = app.global::<App>();
    let b = |v: i32| v.clamp(0, 255) as u8;
    {
        let s = &mut st.s;
        s.setup.kb = match g.get_kb_type() {
            1 => KbType::BlackRgb,
            2 => KbType::White,
            _ => KbType::ClearRgb,
        };
        s.setup.side = match g.get_side() {
            0 => SideModule::None,
            2 => SideModule::Numpad,
            _ => SideModule::Macropad,
        };
        s.setup.module_left = g.get_module_left();
        s.brush = (b(g.get_brush_hue()), b(g.get_brush_sat()));
    }
    let mut look = st.edit_look();
    if let Some(c) = settings::parse_choice(&g.get_show_effect()) {
        look.choice = c;
    }
    look.hue = b(g.get_show_hue());
    look.sat = b(g.get_show_sat());
    look.val = b(g.get_show_val());
    look.speed = b(g.get_show_speed());
    st.set_edit_look(look);
}

fn effect_models(app: &AppWindow) {
    let g = app.global::<App>();
    let item = |key: String, number: String, label: &str| EffectItem {
        key: key.into(),
        number: number.into(),
        label: label.into(),
    };
    let mut groups = vec![EffectGroup {
        name: "Off".into(),
        items: ModelRc::new(VecModel::from(vec![item("paint".into(), String::new(), "Off (painted keys only)")])),
    }];
    for (name, ids) in GROUPS {
        let items: Vec<EffectItem> =
            ids.iter().map(|&id| item(format!("b{id}"), id.to_string(), EFFECT_NAMES[id as usize])).collect();
        groups.push(EffectGroup { name: (*name).into(), items: ModelRc::new(VecModel::from(items)) });
    }
    let pc: Vec<EffectItem> =
        LinkedEffect::ALL.iter().map(|e| item(settings::choice_str(Choice::Pc(*e)), String::new(), e.name())).collect();
    groups.push(EffectGroup {
        name: "Drawn by the app (needs the app running)".into(),
        items: ModelRc::new(VecModel::from(pc)),
    });
    g.set_effect_groups(ModelRc::new(VecModel::from(groups)));
    let mut names: Vec<SharedString> = vec!["Off (painted keys only)".into()];
    names.extend((1..=44).map(|i| SharedString::from(format!("{i}. {}", EFFECT_NAMES[i]))));
    g.set_effect_names(ModelRc::new(VecModel::from(names)));
}

// -------------------------------------------------------------------------
// Every frame
// -------------------------------------------------------------------------

fn tick(app: &AppWindow, st: &mut State) {
    let g = app.global::<App>();
    st.flash_answers(app);
    if AUDIO_SHORTCUT.swap(false, std::sync::atomic::Ordering::AcqRel) {
        let on = !st.audio_on;
        st.set_audio(on);
    }
    st.status = st.engine.status.lock().unwrap().clone();
    let status = st.status.clone();

    // Start the Lighting page from what each device is running.
    let mut adopted_any = false;
    for kind in [DeviceKind::Keyboard, DeviceKind::Macropad] {
        if st.adopted.contains(&kind) {
            continue;
        }
        if let Some(d) = ready(&status, kind) {
            st.adopted.push(kind);
            let look = if kind == DeviceKind::Keyboard { &mut st.s.kb } else { &mut st.s.module };
            let ds = d.state;
            if (1..=44).contains(&ds.mode) {
                *look =
                    Look { choice: Choice::Builtin(ds.mode), hue: ds.hue, sat: ds.sat, val: ds.val, speed: ds.speed };
            } else if ds.mode == d.info.paint_only_effect {
                look.choice = Choice::PaintOnly;
            }
            if st.s.join && kind == DeviceKind::Keyboard {
                st.s.module = st.s.kb;
            }
            push_cal(app, &status, kind);
            adopted_any = true;
        }
    }
    // Follow what is plugged in: macropad (our firmware or not) or Framework's numpad.
    let detected = if !matches!(status.macropad, Slot::Missing) {
        Some(SideModule::Macropad)
    } else if status.numpad {
        Some(SideModule::Numpad)
    } else {
        None
    };
    if detected != st.last_detected {
        st.last_detected = detected;
        if let Some(side) = detected {
            if st.s.setup.side != side {
                st.s.setup.side = side;
                adopted_any = true;
            }
        }
    }
    // Remember clear / black / white per keyboard (by its serial number).
    if let Some(serial) = ready(&status, DeviceKind::Keyboard).and_then(|d| d.serial.clone()) {
        match st.s.kb_types.iter().find(|(s, _)| *s == serial) {
            Some((_, t)) if *t != st.s.setup.kb => {
                st.s.setup.kb = *t;
                adopted_any = true;
            }
            Some(_) => {}
            None => st.s.kb_types.push((serial, st.s.setup.kb)),
        }
    }
    if adopted_any {
        push_all(app, st);
    }

    // The keyboard changed its own effect (F12 shortcuts): show it, and give a joined
    // macropad the same. Ignored right after the app itself sent something.
    if let Some(d) = ready(&status, DeviceKind::Keyboard) {
        let now = d.state;
        let ours = State::state_for(&st.s.kb, d);
        if let Some(prev) = st.last_kb_state {
            if now != prev
                && now != ours
                && !d.streaming
                && !st.audio_on
                && st.last_live.elapsed() > Duration::from_millis(1500)
                && (1..=44).contains(&now.mode)
            {
                st.s.kb = Look {
                    choice: Choice::Builtin(now.mode),
                    hue: now.hue,
                    sat: now.sat,
                    val: now.val,
                    speed: now.speed,
                };
                if st.joined() {
                    st.s.module = st.s.kb;
                }
                push_all(app, st);
                st.dirty = true;
            }
        }
        st.last_kb_state = Some(now);
    }

    // Live: send lighting changes a few times a second while they happen
    if st.dirty && st.last_live.elapsed() >= Duration::from_millis(80) {
        st.dirty = false;
        st.last_live = Instant::now();
        st.send_current(false);
    }
    // Save LED calibration on the device once the sliders have been still for a moment
    for (i, kind) in [DeviceKind::Keyboard, DeviceKind::Macropad].into_iter().enumerate() {
        if st.cal_dirty[i].is_some_and(|t| t.elapsed() >= Duration::from_millis(800)) {
            st.cal_dirty[i] = None;
            if let Some(cal) = cal_from_window(app, i) {
                st.engine.send(Command::SetLedCal { kind, cal, save: true });
            }
        }
    }

    // Devices at the bottom of the menu
    let mut rows: Vec<DeviceRow> = [DeviceKind::Keyboard, DeviceKind::Macropad]
        .iter()
        .filter(|&&kind| kind == DeviceKind::Keyboard || !status.numpad || !matches!(status.macropad, Slot::Missing))
        .map(|&kind| {
            let (text, ok) = match status.slot(kind) {
                Slot::Missing => ("Not found".to_string(), false),
                Slot::Stock => ("Needs the f16custom firmware".to_string(), false),
                Slot::Ready(d) => {
                    (if d.streaming { "Connected · the PC is drawing".into() } else { "Connected".into() }, true)
                }
            };
            DeviceRow { name: kind.name().into(), status: text.into(), ok, checked: false }
        })
        .collect();
    if status.numpad {
        rows.push(DeviceRow {
            name: "Numpad".into(),
            status: "Connected · backlight only".into(),
            ok: true,
            checked: false,
        });
    }
    let same = g.get_devices().iter().eq(rows.iter().cloned());
    if !same {
        g.set_devices(ModelRc::new(VecModel::from(rows)));
    }
    let live = st.pc_live || status.linked;
    g.set_live(live);
    g.set_pc_live(st.pc_live);
    g.set_bar_status(if live {
        "Live: the PC is drawing on your devices.".into()
    } else if !st.notice.is_empty() {
        st.notice.clone().into()
    } else {
        "Live: changes show on the devices as you make them. Save as default keeps them after a restart.".into()
    });
    g.set_status(status.message.clone().unwrap_or_default().into());
    g.set_hotkey_lock(
        [DeviceKind::Keyboard, DeviceKind::Macropad]
            .iter()
            .any(|k| ready(&status, *k).is_some_and(|d| d.info.hotkey_lock)),
    );
    g.set_layer(st.layer() as i32);
    // Audio mode
    if st.audio_on && !status.audio && status.audio_problem.is_some() {
        st.audio_on = false; // it couldn't start
    }
    g.set_audio_on(st.audio_on);
    g.set_audio_level(if st.audio_on { status.audio_level } else { 0.0 });
    if let Some(found) = st.mics_inbox.lock().unwrap().take() {
        st.mics = found;
        push_mics(app, st);
    }
    g.set_audio_status(
        if let Some(p) = &status.audio_problem {
            p.clone()
        } else if st.audio_on {
            match &status.audio_device {
                Some(d) => format!("On: listening to {d}."),
                None => "On: waiting for sound…".into(),
            }
        } else {
            "Off. The lights follow whatever the PC is playing, across the keyboard and side module.".into()
        }
        .into(),
    );
    g.set_kb_present(!matches!(status.keyboard, Slot::Missing));
    g.set_mod_present(!matches!(status.macropad, Slot::Missing));

    // The keyboard picture, on the pages that show it
    let page = g.get_page().to_string();
    if matches!(page.as_str(), "keys" | "home" | "macropad") {
        keyedit::push(app, st);
    }
    let visible = app.window().is_visible();
    // Redraw the picture only as often as this PC keeps up with, so a slow machine never
    // spends all its time drawing and stops answering clicks.
    let due = st.last_draw.elapsed() >= st.draw_cost.mul_f32(2.0);
    if due && visible && matches!(page.as_str(), "lighting" | "audio" | "macropad" | "setup" | "keys" | "home") {
        let started = Instant::now();
        let scale = app.window().scale_factor();
        let width = ((g.get_deck_width() * scale).round() as u32).clamp(300, 1400);
        let mut preview = std::mem::take(&mut st.preview);
        let scene = st.scene(&page);
        let mut placed = deck::place_keys(&scene.setup, scene.macropad_layer);
        let shell = deck::shell(&scene.setup);
        let colors = if page == "keys" {
            keyedit::decorate(st, &mut placed)
        } else if let Some(c) = st.audio_key_colors(&placed) {
            c
        } else {
            preview.colors(&scene, &placed)
        };
        let accent = g_accent(app);
        let pic = render::draw(&scene, &placed, &colors, &shell, width, accent);
        let typing = preview.typing(&scene);
        let joined = scene.joined && scene.setup.side != SideModule::None;
        st.preview = preview;
        st.deck_width_units = shell.size.0;
        st.keys = placed;
        let buffer =
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pic.pixels, pic.width, pic.height);
        g.set_deck_image(slint::Image::from_rgba8_premultiplied(buffer));
        g.set_deck_aspect(pic.height as f32 / pic.width as f32);
        let legends: Vec<Legend> = pic
            .legends
            .into_iter()
            .map(|l| Legend {
                x: l.x,
                y: l.y,
                w: l.w,
                h: l.h,
                text: l.text.into(),
                color: Color::from_argb_u8(l.color[3], l.color[0], l.color[1], l.color[2]),
                fs: l.fs,
                left: l.left,
            })
            .collect();
        // Keep the same model and only touch rows that changed, so the window doesn't rebuild
        // every legend 30 times a second.
        let same_layout = st.legends.row_count() == legends.len()
            && st.legends.iter().zip(&legends).all(|(a, b)| a.text == b.text && a.x == b.x && a.y == b.y && a.w == b.w);
        if same_layout {
            for (i, l) in legends.into_iter().enumerate() {
                if st.legends.row_data(i).as_ref() != Some(&l) {
                    st.legends.set_row_data(i, l);
                }
            }
        } else {
            st.legends.set_vec(legends);
        }
        let mut caption = vec!["Preview".to_string()];
        if page == "keys" {
            caption = vec!["Orange keys are changed from the Framework default".to_string()];
        }
        if page != "keys" {
            caption.push(if joined { "joined: one surface".into() } else { "separate devices".into() });
        }
        if typing {
            caption.push("pretend typing on".into());
        }
        g.set_deck_caption(caption.join("  ·  ").into());
        st.draw_cost = started.elapsed();
        st.last_draw = Instant::now();
    }

    if st.s != st.last_saved {
        settings::save(&crate::paths::keyboard_file(), &st.s);
        st.last_saved = st.s.clone();
    }
}

/// Audio mode's colors into the Audio page.
fn push_audio_colors(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let (i, h, s) = match st.s.audio_colors {
        AudioColors::Auto => (0, None, None),
        AudioColors::Rainbow => (1, None, None),
        AudioColors::Meter => (2, None, None),
        AudioColors::Fire => (3, None, None),
        AudioColors::Ocean => (4, None, None),
        AudioColors::One(h, s) => (5, Some(h), Some(s)),
    };
    g.set_audio_colors(i);
    if let (Some(h), Some(s)) = (h, s) {
        g.set_audio_hue((h * 255.0).round() as i32);
        g.set_audio_sat((s * 255.0).round() as i32);
    }
    let (h, s) = (g.get_audio_hue(), g.get_audio_sat());
    g.set_audio_full(color(qmk_hsv(h, 255, 255)));
    g.set_audio_picked(color(qmk_hsv(h, s, 255)));
    g.set_audio_hex(hex(qmk_hsv(h, s, 255)));
}

/// The microphone list and the chosen one, into the Audio page.
fn push_mics(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let mut names: Vec<SharedString> = vec!["Default microphone".into()];
    names.extend(st.mics.iter().map(|m| SharedString::from(m.as_str())));
    // A chosen microphone that isn't plugged in right now stays in the list
    if !st.s.audio_device.is_empty() && !st.mics.contains(&st.s.audio_device) {
        names.push(st.s.audio_device.as_str().into());
    }
    let index = if st.s.audio_device.is_empty() {
        0
    } else {
        names.iter().position(|n| n.as_str() == st.s.audio_device).unwrap_or(0)
    };
    g.set_audio_devices(ModelRc::new(VecModel::from(names)));
    g.set_audio_device(index as i32);
    g.set_audio_input(st.s.audio_mic as i32);
}

/// Puts one device's LED calibration into the window's sliders.
fn push_cal(app: &AppWindow, status: &Status, kind: DeviceKind) {
    let g = app.global::<App>();
    let cal = ready(status, kind).and_then(|d| d.led_cal);
    let c = cal.unwrap_or(wr_keyboard::protocol::LedCal { current: 104, gains: [255; 3], default_current: 104 });
    if kind == DeviceKind::Keyboard {
        g.set_kb_cal_known(cal.is_some());
        g.set_kb_cal_current(c.current as i32);
        g.set_kb_cal_r(c.gains[0] as i32);
        g.set_kb_cal_g(c.gains[1] as i32);
        g.set_kb_cal_b(c.gains[2] as i32);
    } else {
        g.set_mod_cal_known(cal.is_some());
        g.set_mod_cal_current(c.current as i32);
        g.set_mod_cal_r(c.gains[0] as i32);
        g.set_mod_cal_g(c.gains[1] as i32);
        g.set_mod_cal_b(c.gains[2] as i32);
    }
}

/// One device's LED calibration as the window's sliders show it (0 keyboard, 1 macropad).
fn cal_from_window(app: &AppWindow, which: usize) -> Option<wr_keyboard::protocol::LedCal> {
    let g = app.global::<App>();
    let b = |v: i32| v.clamp(1, 255) as u8;
    let (known, cur, r, gr, bl) = if which == 0 {
        (g.get_kb_cal_known(), g.get_kb_cal_current(), g.get_kb_cal_r(), g.get_kb_cal_g(), g.get_kb_cal_b())
    } else {
        (g.get_mod_cal_known(), g.get_mod_cal_current(), g.get_mod_cal_r(), g.get_mod_cal_g(), g.get_mod_cal_b())
    };
    known.then_some(wr_keyboard::protocol::LedCal { current: b(cur), gains: [b(r), b(gr), b(bl)], default_current: 0 })
}

fn g_accent(app: &AppWindow) -> [u8; 3] {
    let c = app.global::<Theme>().get_accent();
    [c.red(), c.green(), c.blue()]
}

thread_local! {
    /// The keyboard part's state and its frame timer, while the window is open.
    static RUNNING: RefCell<Option<(Rc<RefCell<State>>, slint::Timer)>> = const { RefCell::new(None) };
}

/// Pretend keyboard and macropad (no hardware needed): `--demo`, or WHITE_RABBIT_FAKE_KEYBOARD=1.
pub fn demo() -> bool {
    std::env::args().any(|a| a == "--demo") || std::env::var_os("WHITE_RABBIT_FAKE_KEYBOARD").is_some()
}

/// Closing the window: hand the lights back to the devices' saved effects, save the settings, and give
/// the keyboard to White Rabbit's background part, which carries on with an app-drawn effect or the
/// audio show.
pub fn stop() {
    if let Some((st, timer)) = RUNNING.with(|r| r.borrow_mut().take()) {
        drop(timer);
        let Ok(mut st) = st.try_borrow_mut() else { return };
        st.bg.close();
        st.engine.shutdown();
        if st.s != st.last_saved {
            settings::save(&crate::paths::keyboard_file(), &st.s);
            st.last_saved = st.s.clone();
        }
        if !st.demo {
            crate::link::request(
                serde_json::json!({"cmd": "keyboard", "hold": false, "audio": st.audio_on}),
                Duration::from_secs(2),
            );
        }
    }
}

/// Asks White Rabbit's background part to stop driving the keyboard while the window is open.
/// True when its audio show was on, so the window carries on with it.
fn take_keyboard() -> bool {
    let reply = crate::link::request(
        serde_json::json!({"cmd": "keyboard", "hold": true, "pid": std::process::id()}),
        Duration::from_secs(2),
    );
    reply.and_then(|r| r.get("audio").and_then(|a| a.as_bool())).unwrap_or(false)
}

/// Ctrl+Alt+Shift+M was pressed (set by the shortcut watcher, picked up by `tick`).
static AUDIO_SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts the keyboard part: finds the devices, fills the keyboard pages and answers them.
pub fn wire(app: &AppWindow) {
    let demo = demo();
    let audio_was_on = !demo && take_keyboard();
    wr_keyboard::shortcut::watch(|| AUDIO_SHORTCUT.store(true, std::sync::atomic::Ordering::Release));
    let weak = app.as_weak();
    let engine = engine::start(
        move || {
            let _ = weak.upgrade_in_event_loop(|_| {});
        },
        demo,
    );
    let s = settings::load(&crate::paths::keyboard_file());
    let bg = winsys::Background::start();
    let st = Rc::new(RefCell::new(State {
        engine,
        bg,
        ke: keyedit::KeyEdit::default(),
        flashing: None,
        flash_inbox: Arc::new(Mutex::new(vec![])),
        restore_keys: None,
        last_drive_scan: Instant::now() - Duration::from_secs(60),
        demo,
        last_saved: s.clone(),
        s,
        preview: Preview::default(),
        keys: vec![],
        deck_width_units: 1.0,
        local_layer: 0,
        pad_key: 0,
        pc_live: false,
        notice: String::new(),
        adopted: vec![],
        dirty: false,
        last_live: Instant::now(),
        cal_dirty: [None, None],
        white_test: false,
        last_detected: None,
        audio_on: false,
        mics: vec![],
        mics_inbox: Arc::new(Mutex::new(None)),
        last_kb_state: None,
        draw_cost: Duration::ZERO,
        last_draw: Instant::now(),
        status: Status::default(),
        legends: Rc::new(VecModel::default()),
    }));
    effect_models(app);
    {
        let looks: Vec<AudioLookItem> = wr_keyboard::audio::AudioLook::ALL
            .iter()
            .map(|l| AudioLookItem { key: l.key().into(), name: l.name().into(), about: l.about().into() })
            .collect();
        app.global::<App>().set_audio_looks(ModelRc::new(VecModel::from(looks)));
    }
    {
        let mut st = st.borrow_mut();
        push_all(app, &st);
        st.sync_background();
        keyedit::push(app, &mut st);
        push_mics(app, &st);
        push_audio_colors(app, &st);
        app.global::<App>()
            .set_audio_keys(KeyReact::ALL.iter().position(|k| *k == st.s.audio_keys).unwrap_or(0) as i32);
        st.find_microphones();
        if audio_was_on {
            st.set_audio(true);
        }
    }
    let g = app.global::<App>();
    g.set_legends(ModelRc::from(st.borrow().legends.clone()));

    // A callback that reads the window, changes the state and refreshes what depends on it.
    macro_rules! on {
        ($setter:ident, |$app:ident, $st:ident $(, $arg:ident : $ty:ty)*| $body:block) => {{
            let weak = app.as_weak();
            let st = st.clone();
            g.$setter(move |$($arg: $ty),*| {
                let Some($app) = weak.upgrade() else { return };
                let mut guard = st.borrow_mut();
                let $st: &mut State = &mut guard;
                $body
            });
        }};
    }

    on!(on_setup_changed, |app, st| {
        pull_all(&app, st);
        // Remember the keyboard type for the keyboard that is plugged in
        if let Some(serial) = ready(&st.status, DeviceKind::Keyboard).and_then(|d| d.serial.clone()) {
            let t = st.s.setup.kb;
            st.s.kb_types.retain(|(s, _)| *s != serial);
            st.s.kb_types.push((serial, t));
        }
        push_derived(&app, st);
        st.dirty = true;
    });
    on!(on_show_changed, |app, st| {
        pull_all(&app, st);
        push_derived(&app, st);
        st.dirty = true;
    });
    on!(on_join_toggled, |app, st, on: bool| {
        pull_all(&app, st);
        st.s.join = on;
        if on {
            st.s.module = st.s.kb;
        }
        push_all(&app, st);
        st.dirty = true;
    });
    on!(on_target_chosen, |app, st, target: i32| {
        pull_all(&app, st);
        st.s.join = target == 0;
        st.s.edit_module = target == 2;
        if st.s.join {
            st.s.module = st.s.kb;
        }
        push_all(&app, st);
        st.dirty = true;
    });
    on!(on_paint_mode_chosen, |app, st, mode: i32| {
        st.s.paint_mode = match mode {
            0 => PaintMode::Steady,
            2 => PaintMode::Tint,
            _ => PaintMode::Follow,
        };
        push_all(&app, st);
        st.dirty = true;
    });
    on!(on_send_show, |app, st, save: bool| {
        pull_all(&app, st);
        st.send_current(save);
    });
    on!(on_stop_pc, |_app, st| {
        st.engine.send(Command::SetLinked(None));
        st.pc_live = false;
    });
    on!(on_paint_example, |app, st| {
        pull_all(&app, st);
        for (i, k) in keys::keyboard_keys().iter().enumerate() {
            if ["W", "A", "S", "D", "\u{2190}", "\u{2191}", "\u{2192}", "\u{2193}"].contains(&k.main.as_str()) {
                st.s.paint.insert((Dev::Keyboard, i), st.s.brush);
            }
        }
        push_derived(&app, st);
        st.dirty = true;
    });
    on!(on_paint_clear, |app, st| {
        st.s.paint.clear();
        push_derived(&app, st);
        st.dirty = true;
    });
    on!(on_deck_pointer, |app, st, xf: f32, yf: f32, kind: i32| {
        let g = app.global::<App>();
        let (x, y) = (xf * st.deck_width_units, yf * st.deck_width_units);
        let Some(k) = deck::key_at(&st.keys, x, y).cloned() else {
            return;
        };
        match g.get_page().as_str() {
            "lighting" if k.rgb => {
                pull_all(&app, st);
                if g.get_paint_tool() == 1 {
                    st.s.paint.remove(&(k.dev, k.index));
                } else {
                    // a painted key takes the new color too
                    st.s.paint.insert((k.dev, k.index), st.s.brush);
                }
                push_derived(&app, st);
                st.dirty = true;
            }
            "macropad" if k.dev == Dev::Module && kind == 0 => {
                st.pad_key = k.index;
                push_derived(&app, st);
            }
            "keys" if kind == 0 => {
                keyedit::select(st, k.dev, k.index);
                keyedit::push(&app, st);
            }
            _ => {}
        }
    });
    on!(on_layer_chosen, |_app, st, layer: i32| {
        st.local_layer = layer as u8;
        st.engine.send(Command::SetLayer(layer as u8));
    });
    on!(on_lock_toggled, |_app, st, on: bool| {
        st.engine.send(Command::SetHotkeyLock(on));
    });
    on!(on_bootloader, |_app, st, which: i32| {
        st.engine
            .send(Command::Bootloader(if which == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad }, false));
    });
    on!(on_flash_latest, |app, st, which: i32| {
        st.start_flash(&app, which, flash::Source::Latest, flash::Target::Running);
    });
    on!(on_flash_file, |app, st, which: i32| {
        if let Some(p) = winsys::browse_for_firmware() {
            st.start_flash(&app, which, flash::Source::File(p.into()), flash::Target::Running);
        }
    });
    on!(on_flash_waiting, |app, st, which: i32| {
        let Some(drive) = flash::waiting_drives().into_iter().next() else {
            app.global::<App>().set_waiting_drive("".into());
            return;
        };
        st.start_flash(&app, which, flash::Source::Latest, flash::Target::Drive(drive));
    });
    on!(on_rescan, |_app, st| {
        st.engine.send(Command::Rescan);
    });
    // Matching the two devices' brightness
    on!(on_cal_changed, |app, st, which: i32| {
        let i = which.clamp(0, 1) as usize;
        let kind = if i == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad };
        if let Some(cal) = cal_from_window(&app, i) {
            st.engine.send(Command::SetLedCal { kind, cal, save: false });
            st.cal_dirty[i] = Some(Instant::now());
        }
    });
    on!(on_cal_reset, |app, st, which: i32| {
        let i = which.clamp(0, 1) as usize;
        let kind = if i == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad };
        if let Some(d) = ready(&st.status, kind) {
            if let Some(mut cal) = d.led_cal {
                cal.current = cal.default_current;
                cal.gains = [255; 3];
                st.engine.send(Command::SetLedCal { kind, cal, save: true });
                if let Slot::Ready(d) = st.status.slot_mut(kind) {
                    d.led_cal = Some(cal);
                }
                push_cal(&app, &st.status, kind);
            }
        }
    });
    on!(on_white_test_toggled, |app, st, on: bool| {
        st.white_test = on;
        app.global::<App>().set_white_test(on);
        if on {
            st.send_white();
        } else {
            st.send_current(false);
        }
    });
    // Color codes typed or pasted into a color picker
    {
        let g = app.global::<App>();
        g.on_hex_hue(|t| parse_hex(&t).map_or(-1, |(h, _)| h as i32));
        g.on_hex_sat(|t| parse_hex(&t).map_or(-1, |(_, s)| s as i32));
    }
    // Keys page
    on!(on_keys_view_chosen, |app, st, which: i32, view: i32| {
        keyedit::view_chosen(st, which, view);
        keyedit::push(&app, st);
    });
    on!(on_key_kind_chosen, |app, st, kind: i32| {
        keyedit::kind_chosen(st, kind);
        keyedit::push(&app, st);
    });
    on!(on_key_choice_chosen, |app, st, i: i32| {
        keyedit::choice_chosen(st, i);
        keyedit::push(&app, st);
    });
    on!(on_key_mod_toggled, |app, st, which: i32, on: bool| {
        keyedit::mod_toggled(st, which, on);
        keyedit::push(&app, st);
    });
    on!(on_key_app_edited, |_app, st, path: SharedString| {
        keyedit::app_edited(st, &path);
    });
    on!(on_key_app_browse, |app, st| {
        keyedit::app_browse(st);
        keyedit::push(&app, st);
    });
    on!(on_keys_reset, |app, st| {
        keyedit::reset_all(st);
        keyedit::push(&app, st);
    });
    on!(on_combo_toggled, |app, st, which: i32, on: bool| {
        keyedit::combo_toggled(st, which, on);
        keyedit::push(&app, st);
    });
    on!(on_combo_choice_chosen, |app, st, which: i32, i: i32| {
        keyedit::combo_chosen(st, which, i);
        keyedit::push(&app, st);
    });
    on!(on_combo_app_edited, |_app, st, which: i32, path: SharedString| {
        keyedit::combo_app_edited(st, which, &path);
    });
    on!(on_combo_app_browse, |app, st, which: i32| {
        keyedit::combo_app_browse(st, which);
        keyedit::push(&app, st);
    });
    on!(on_combo_top_row_chosen, |app, st, on: bool| {
        keyedit::combo_top_row_chosen(st, on);
        keyedit::push(&app, st);
    });
    on!(on_shortcuts_toggled, |app, st, on: bool| {
        keyedit::shortcuts_toggled(st, on);
        keyedit::push(&app, st);
    });
    on!(on_corner_lights_toggled, |app, st, on: bool| {
        keyedit::corner_lights_toggled(st, on);
        keyedit::push(&app, st);
    });
    // Audio mode
    on!(on_audio_toggled, |app, st, on: bool| {
        st.set_audio(on);
        app.global::<App>().set_audio_on(on);
    });
    on!(on_audio_shortcut, |app, st| {
        let on = !st.audio_on;
        st.set_audio(on);
        app.global::<App>().set_audio_on(on);
    });
    on!(on_audio_changed, |app, st| {
        let g = app.global::<App>();
        if let Some(l) = wr_keyboard::audio::AudioLook::from_key(&g.get_audio_look()) {
            st.s.audio_look = l;
        }
        st.s.audio_sensitivity = g.get_audio_sensitivity().clamp(1, 10) as u8;
        st.audio_settings_changed();
    });
    on!(on_audio_input_chosen, |app, st, i: i32| {
        st.s.audio_mic = i == 1;
        push_mics(&app, st);
        st.audio_settings_changed();
    });
    on!(on_audio_device_chosen, |app, st, i: i32| {
        let names = app.global::<App>().get_audio_devices();
        st.s.audio_device = match names.row_data(i.max(0) as usize) {
            Some(n) if i > 0 => n.to_string(),
            _ => String::new(),
        };
        st.audio_settings_changed();
    });
    on!(on_audio_devices_refresh, |_app, st| {
        st.find_microphones();
    });
    on!(on_audio_colors_changed, |app, st| {
        let g = app.global::<App>();
        let (h, s) = (g.get_audio_hue().clamp(0, 255), g.get_audio_sat().clamp(0, 255));
        st.s.audio_colors = match g.get_audio_colors() {
            1 => AudioColors::Rainbow,
            2 => AudioColors::Meter,
            3 => AudioColors::Fire,
            4 => AudioColors::Ocean,
            5 => AudioColors::One(h as f32 / 255.0, s as f32 / 255.0),
            _ => AudioColors::Auto,
        };
        push_audio_colors(&app, st);
        st.audio_settings_changed();
    });
    on!(on_audio_keys_chosen, |app, st, i: i32| {
        st.s.audio_keys = KeyReact::ALL.get(i.max(0) as usize).copied().unwrap_or_default();
        app.global::<App>().set_audio_keys(i);
        st.audio_settings_changed();
    });
    on!(on_audio_hotkey_toggled, |app, st, on: bool| {
        st.s.audio_hotkey = on;
        app.global::<App>().set_audio_hotkey(on);
        st.sync_background();
    });

    // ~30 frames a second for the live picture
    let timer = slint::Timer::default();
    {
        let weak = app.as_weak();
        let st = st.clone();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            if let Some(app) = weak.upgrade() {
                if let Ok(mut st) = st.try_borrow_mut() {
                    tick(&app, &mut st);
                }
            }
        });
    }
    RUNNING.with(|r| *r.borrow_mut() = Some((st, timer)));
}
