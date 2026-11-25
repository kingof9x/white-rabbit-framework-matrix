//! The Framework 16 RGB keyboard and RGB macropad pages (Lighting, Audio show, Key mapping,
//! Macropad, Keyboard & macropad setup and the keyboard firmware), brought over from the Framework 16
//! Keyboard Control app. The devices, effects and picture live in the wr-keyboard crate.
//!
//! How it fits together, all on the window's UI thread unless said otherwise:
//!
//! `wire` runs once when the window opens. It asks White Rabbit's background part (the monitor
//! process, over `crate::link`) to let go of the keyboard while the window is open, starts the
//! wr-keyboard "engine" (its own thread that owns the USB HID handles and talks to the devices),
//! loads the keyboard settings file, builds one `State`, hooks every Slint callback of the `Kb`
//! global to a closure that changes that state, and starts a 50 ms repeating timer that calls `tick`.
//! `stop` runs when the window closes and undoes all that: it shuts the engine down, saves the
//! settings and hands the keyboard back to the background part.
//!
//! The UI thread and the engine thread only meet in two places: commands go to the engine through a
//! channel (`EngineHandle::send`), and the engine publishes a `Status` snapshot in a mutex that `tick`
//! copies every frame. The engine wakes the window's event loop when something changes. Other
//! background threads (firmware update, firmware check, microphone search) hand their answers back
//! the same way: they put them in an `Arc<Mutex<…>>` "inbox" and wake the event loop, and `tick`
//! empties the inbox. The UI thread never blocks on a device or the network.
//!
//! The state is kept in an `Rc<RefCell<State>>`: Slint callbacks and the timer all run on the UI
//! thread, so a `RefCell` is enough; the timer uses `try_borrow_mut` and simply skips a frame if a
//! callback is still holding the state (which can happen if a callback opens a modal dialog).
//!
//! Lighting choices (`Saved`, field `s`) are sent live to the devices without saving them on the
//! device ("live"), at most every 80 ms while a slider moves, and written to the device's own memory
//! only when the person presses "Save as default". The settings file is rewritten whenever `s` differs
//! from the last saved copy, checked at the end of every `tick`.

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
///
/// Each row is (group heading, firmware effect ids). The ids are the f16custom firmware's built-in
/// effect numbers 1 to 44; their names come from `EFFECT_NAMES`. Every id appears once.
const GROUPS: &[(&str, &[u8])] = &[
    ("Solid", &[1, 2, 5]),
    ("Gradients and bands", &[3, 4, 6, 7, 8, 9, 10, 11]),
    ("Rainbow cycles", &[12, 13, 14, 15, 16, 17, 18, 19]),
    ("Beacons and hues", &[20, 21, 22, 25, 26, 27]),
    ("Random", &[23, 24, 28, 29, 30, 32]),
    ("React to typing", &[31, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44]),
];

/// Everything the keyboard pages know, owned by the UI thread (see the file header).
pub struct State {
    /// The connection to the engine thread: send commands, read its published status.
    engine: EngineHandle,
    /// Stand-in for the old background hotkey part; mostly no-ops (see `winsys.rs`).
    bg: winsys::Background,
    /// The Keys page's own state (selected key, cached lists).
    ke: keyedit::KeyEdit,
    /// Firmware update in progress (which device), and what its thread reports.
    flashing: Option<DeviceKind>,
    flash_inbox: Arc<Mutex<Vec<flash::Progress>>>,
    /// Keys the user changed, saved before a firmware update (new firmware resets the keymap)
    /// and put back once the device is back: device, the keys, when the update finished.
    restore_keys: Option<(DeviceKind, Vec<wr_keyboard::keycodes::KeyWrite>, Option<Instant>)>,
    /// When the Firmware page last looked for a device waiting in flashing mode.
    last_drive_scan: Instant,
    /// The last Check for updates' answer for (keyboard, macropad), and where its thread puts it.
    fw_newest: [Option<Result<update::Newest, String>>; 2],
    fw_inbox: Arc<Mutex<Option<[Result<update::Newest, String>; 2]>>>,
    /// A Check for updates thread is running (the button is disabled meanwhile).
    fw_checking: bool,
    /// `--demo` mode: fake devices, no network, no hand-over with the background part.
    demo: bool,
    /// The settings being edited: setup, looks, painted keys, audio choices, app paths.
    s: Saved,
    /// The settings as last written to disk, to notice changes cheaply.
    last_saved: Saved,
    /// The animated preview of the effects for the keyboard picture (runs the effects on the PC).
    preview: Preview,
    /// The keys of the last drawn picture, with positions, for turning a click into a key.
    keys: Vec<PlacedKey>,
    /// The picture's width in key units; clicks arrive as 0..1 fractions of the width and are scaled.
    deck_width_units: f32,
    /// The macropad layer last chosen here, used until the macropad reports its own.
    local_layer: u8,
    /// The key selected on the Macropad page (0..23, row by row, 4 per row).
    pad_key: usize,
    /// An effect drawn by the PC ("linked" effect) is being sent to the devices by this window.
    pc_live: bool,
    /// A message for the bottom bar after sending ("No device connected", "Saved on the devices…").
    notice: String,
    /// Devices whose running effect has been read into the Lighting page already (done once each).
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
    /// The engine's latest status, copied each frame; also patched locally right after a change.
    status: Status,
    /// The key labels drawn over the picture; one model kept for the window's lifetime.
    legends: Rc<VecModel<Legend>>,
}

impl State {
    /// Starts a firmware update of the keyboard (`which` 0) or macropad (any other value) on a new
    /// thread, unless one is already running (a second press is ignored).
    ///
    /// Before starting, the device's changed keys are copied, because new firmware starts with the
    /// default keymap; `flash_answers` writes them back once the updated device is back. The thread
    /// runs `flash::run`, whose restart request goes to the engine as `Command::Bootloader` (the engine
    /// owns the USB handles) and whose progress goes into `flash_inbox`, with a wake-up of the event loop.
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
                    // `true`: started by the app, so the engine words its message as "Updating…".
                    // A send error means the engine has stopped (window closing); nothing to do.
                    let _ = tx.send(Command::Bootloader(kind, true));
                },
                |p| {
                    // The lock is held only for the push; `unwrap` would only fail if the UI thread
                    // had panicked while holding it, in which case the window is gone anyway.
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
            // Pretend the last drive scan was long ago, so the next tick looks for drives at once.
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
                // The device hasn't come back with a readable keymap in 90 s: give up on restoring.
                self.restore_keys = None;
            }
        }
        // While the Firmware page is open, look every 2 s for a device waiting in flashing mode, so
        // the page can offer "Install on drive E:". Only on that page, as it checks every drive letter.
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

/// A typed color code ("#FF8800" or "ff8800") as QMK hue and saturation (each 0..255).
///
/// Brightness is dropped because the pickers only choose hue and saturation (brightness is its own
/// slider). Anything that is not exactly six hex digits gives None, which the window shows as "-1:
/// ignore".
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

/// An RGB triple as a Slint color for the window.
fn color(c: [u8; 3]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

/// An RGB triple as "#RRGGBB" text for the color pickers' code box.
fn hex(c: [u8; 3]) -> SharedString {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]).into()
}

/// The firmware on each device, and what the last Check for updates says about it.
///
/// Runs every tick. First it takes a finished check from `fw_inbox`. Then, per device, it builds
/// three texts (installed firmware, the check's verdict, the update button's label) and only writes
/// them to the window when they changed, so Slint doesn't re-lay out the page 20 times a second.
fn show_firmware_builds(app: &AppWindow, st: &mut State, status: &Status) {
    let g = app.global::<App>();
    if let Some(found) = st.fw_inbox.lock().unwrap().take() {
        st.fw_newest = found.map(Some);
        st.fw_checking = false;
    }
    if g.get_fw_checking() != st.fw_checking {
        g.set_fw_checking(st.fw_checking);
    }
    for (i, kind) in [DeviceKind::Keyboard, DeviceKind::Macropad].into_iter().enumerate() {
        let slot = status.slot(kind);
        let installed = match slot {
            Slot::Missing => "".to_string(),
            Slot::Stock => "Installed: not this app's firmware (Framework's own, or a very old one).".into(),
            Slot::Ready(d) => format!("Installed: {}.", d.info.build.describe()),
        };
        let build = match slot {
            Slot::Ready(d) => Some(d.info.build),
            _ => None,
        };
        let (check, label) = match (&st.fw_newest[i], slot) {
            (_, Slot::Missing) | (None, _) => (String::new(), String::new()),
            (Some(Err(e)), _) => (format!("Couldn't check: {e}"), String::new()),
            (Some(Ok(n)), _) => {
                let (text, newer) = update::compare(build, n);
                let label = match (newer, n.build) {
                    (true, Some(wr_keyboard::protocol::FirmwareBuild::Build(m))) => format!("Update to build {m}"),
                    _ => String::new(),
                };
                (text, label)
            }
        };
        let (gi, gc, gl) = if i == 0 {
            (g.get_kb_fw_installed(), g.get_kb_fw_check(), g.get_kb_fw_update_label())
        } else {
            (g.get_mod_fw_installed(), g.get_mod_fw_check(), g.get_mod_fw_update_label())
        };
        if gi != installed.as_str() || gc != check.as_str() || gl != label.as_str() {
            if i == 0 {
                g.set_kb_fw_installed(installed.into());
                g.set_kb_fw_check(check.into());
                g.set_kb_fw_update_label(label.into());
            } else {
                g.set_mod_fw_installed(installed.into());
                g.set_mod_fw_check(check.into());
                g.set_mod_fw_update_label(label.into());
            }
        }
    }
}

/// The device's status if it is connected with this app's firmware; None if missing or "stock"
/// (Framework's firmware, which can't do most of what these pages offer).
fn ready(status: &Status, kind: DeviceKind) -> Option<&engine::DeviceStatus> {
    match status.slot(kind) {
        Slot::Ready(d) => Some(d),
        _ => None,
    }
}

impl State {
    /// The macropad's active layer (0 = layer A, the MIDI layer; 1 = layer B, F-keys): what the
    /// macropad reports if it can, otherwise the last one chosen here.
    fn layer(&self) -> u8 {
        ready(&self.status, DeviceKind::Macropad).and_then(|d| d.info.layer).unwrap_or(self.local_layer)
    }

    /// "Numpad" or "Macropad" for sentences, from the setup (no side module also says "Macropad").
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

    /// The firmware lighting state for a look: its built-in effect, or for "painted keys only" and
    /// PC-drawn looks the device's own "off, painted keys only" effect id (it differs by firmware).
    fn state_for(look: &Look, d: &engine::DeviceStatus) -> LightState {
        let mode = match look.choice {
            Choice::Builtin(id) => id,
            _ => d.info.paint_only_effect,
        };
        LightState { enabled: true, mode, hue: look.hue, sat: look.sat, val: look.val, speed: look.speed }
    }

    /// Painted keys for one device, one entry per LED.
    ///
    /// The device reports its LEDs' positions; `map_leds_to_keys` matches each LED to the nearest key
    /// in the app's key list, and the painted color of that key (or None) goes in the LED's slot.
    fn paint_for(&self, dev: Dev, d: &engine::DeviceStatus) -> Vec<Paint> {
        let (defs, size) = match dev {
            Dev::Keyboard => (keys::keyboard_keys(), KEYBOARD_SIZE),
            Dev::Module => (keys::macropad_keys(self.layer()), MODULE_GRID),
        };
        keys::map_leds_to_keys(&d.leds, &defs, size).into_iter().map(|k| self.s.paint.get(&(dev, k)).copied()).collect()
    }

    /// Both devices with whether they have RGB lights in the current setup (a white-backlit keyboard
    /// or a numpad get no colors).
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
    ///
    /// Joined: the keyboard's look (which both share). Otherwise whichever device the "target"
    /// buttons picked.
    fn edit_look(&self) -> Look {
        if self.joined() || !self.s.edit_module {
            self.s.kb
        } else {
            self.s.module
        }
    }

    /// Stores an edited look on the device(s) it belongs to (both when joined).
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
    ///
    /// Two cases. A PC-drawn effect (`Choice::Pc`) is computed by the engine on the PC and streamed
    /// frame by frame to both devices, so only its parameters and the macropad's side are sent. A
    /// firmware effect is sent as a lighting state per ready device, plus the "span": when joined,
    /// each device is told its slice of one shared surface so an effect flows across both.
    /// Painted keys are then sent to each RGB device. With `save` the devices also store it all.
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
                // Stop streaming the PC-drawn effect before handing the lights back to the firmware.
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
            // Join only when both devices are ready and the side module really is the macropad.
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

    /// The audio show's choices from the settings, in the form the engine wants.
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
    ///
    /// Listing audio devices can take a moment on Windows, hence the thread. The answer goes into
    /// `mics_inbox`; there is no wake-up, the next 50 ms tick finds it.
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
    ///
    /// None when the engine isn't drawing audio. Otherwise one color per placed key (black for keys
    /// with no LED), mapping each device's LED colors to keys the same way `paint_for` does.
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
    ///
    /// Does nothing during the white test or the audio show, which own the lights meanwhile.
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
        // At least half brightness, so the two devices can be compared by eye.
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
                // No painted keys during the test: every key plain white.
                self.engine.send(Command::SetPaint { kind, leds: vec![], mode: self.s.paint_mode, save: false });
            }
        }
    }

    /// What the keyboard picture should show for `page`: the setup, the looks, painted keys, the
    /// macropad layer, and which key to outline as selected (Macropad and Keys pages only).
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
///
/// Writes every setting the pages show into the `Kb` global (the drop-downs and sliders use small
/// integer codes; the matches below are the code tables), then the derived texts via `push_derived`.
fn push_all(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let s = &st.s;
    // Keyboard type codes: 0 clear RGB, 1 black RGB, 2 white backlight.
    g.set_kb_type(match s.setup.kb {
        KbType::ClearRgb => 0,
        KbType::BlackRgb => 1,
        KbType::White => 2,
    });
    // Side module codes: 0 none, 1 RGB macropad, 2 numpad.
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
    // Lighting target codes: 0 both (joined), 1 keyboard, 2 side module.
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
///
/// Called after almost every change. Colors use QMK's HSV (all three 0..255) converted to RGB.
fn push_derived(app: &AppWindow, st: &State) {
    let g = app.global::<App>();
    let s = &st.s;
    // For a color picker: the hue at full color (the hue strip), the picked color, and its code.
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
    // One line for the menu: what each device shows now.
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
    // Devices with a single white backlight can only dim; say so once on the Setup page.
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
    // What the key sends on layer A and on layer B; the macropad is 4 keys wide (row/column 1-based).
    let (a, b) = keys::macropad_sends(st.pad_key);
    g.set_pad_key(format!("Row {}, column {}", st.pad_key / 4 + 1, st.pad_key % 4 + 1).into());
    g.set_pad_a(a.into());
    g.set_pad_b(b.into());
    g.set_pad_note(
        match st.pad_key {
            // Keys 0 and 3 are the top corners, 20 and 23 the bottom corners of the 4 x 6 grid.
            0 | 3 => "Top corner: press both top corners together to switch layers. Pressed alone it still works, about 40 ms later.",
            20 | 23 => "Bottom corner: press both bottom corners together for the lighting controls.",
            _ => "An ordinary key.",
        }
        .into(),
    );
}

/// Reads the window's choices back into the state.
///
/// The opposite of `push_all` for the setup and the edited look. Slider values are clamped to 0..255
/// and unknown codes fall back to the first choice, so odd values from the window can't break the state.
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

/// Fills the effect pickers once at start: the grouped list (keys "paint", "b<id>" for built-in
/// effects, and the PC-drawn effects' keys) and the plain numbered name list 1..44.
fn effect_models(app: &AppWindow) {
    let g = app.global::<App>();
    let item = |key: String, number: String, label: &str| EffectItem {
        key: key.into(),
        number: number.into(),
        label: label.into(),
    };
    // "Off" first, then the firmware's effects in `GROUPS`, then the PC-drawn effects.
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

/// One frame of the keyboard part, every 50 ms from the timer started in `wire`.
///
/// In order: show firmware-update progress; act on the audio show shortcut; copy the engine's
/// status; the first time each device appears, read its running effect into the Lighting page; follow
/// a side module being swapped and the keyboard's remembered type; notice effect changes made on the
/// keyboard itself (Fn+F12 shortcuts); send pending live lighting changes (at most every 80 ms) and
/// save calibration once its sliders rest; update the device list, status texts, audio meter and
/// firmware texts; redraw the keyboard picture when due; and save the settings file if they changed.
/// Window setters are mostly guarded by "only if different", because each set can make Slint redo
/// layout work.
fn tick(app: &AppWindow, st: &mut State) {
    let g = app.global::<App>();
    st.flash_answers(app);
    // Take and clear the flag in one step, so a press is acted on exactly once.
    if AUDIO_SHORTCUT.swap(false, std::sync::atomic::Ordering::AcqRel) {
        let on = !st.audio_on;
        st.set_audio(on);
    }
    // Copy the snapshot and release the engine's lock at once, so the engine is never kept waiting
    // while the window works through the frame.
    st.status = st.engine.status.lock().unwrap().clone();
    let status = st.status.clone();

    // Start the Lighting page from what each device is running.
    // `adopted_any` collects "a setting changed behind the page's back", to refresh it once at the end.
    let mut adopted_any = false;
    for kind in [DeviceKind::Keyboard, DeviceKind::Macropad] {
        if st.adopted.contains(&kind) {
            continue;
        }
        if let Some(d) = ready(&status, kind) {
            st.adopted.push(kind);
            let look = if kind == DeviceKind::Keyboard { &mut st.s.kb } else { &mut st.s.module };
            let ds = d.state;
            // A firmware effect (ids 1..44) becomes the page's look; the device's "painted keys
            // only" id becomes that choice; any other mode (e.g. a PC stream) is left as saved.
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
    // A macropad counts even with other firmware (`Stock`): it is plugged in, just not usable yet.
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
    // The firmware can't tell clear, black and white keyboards apart, so the person says once in
    // Setup and the answer is remembered by the keyboard's USB serial number.
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
            // Count it as a change made on the keyboard only if: it changed since last frame; it
            // is not what this window last asked for; the PC isn't streaming to it; the audio show
            // is off; this window sent nothing for 1.5 s (its own sends take a moment to show up
            // in the status); and it is a firmware effect.
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
    // Sliders can fire dozens of times a second; sending at most every 80 ms (about 12 a second)
    // keeps the USB traffic down while still looking live. `dirty` keeps the last change pending.
    if st.dirty && st.last_live.elapsed() >= Duration::from_millis(80) {
        st.dirty = false;
        st.last_live = Instant::now();
        st.send_current(false);
    }
    // Save LED calibration on the device once the sliders have been still for a moment
    for (i, kind) in [DeviceKind::Keyboard, DeviceKind::Macropad].into_iter().enumerate() {
        // Calibration is already sent live (unsaved) as the slider moves; it is written to the
        // device's memory only after 800 ms without changes, to spare the flash/EEPROM writes.
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
        // List the macropad row unless a numpad is plugged in and no macropad is (then the numpad
        // row below replaces it).
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
    // Replace the model only when a row changed, so the list doesn't flicker or lose hover state.
    let same = g.get_devices().iter().eq(rows.iter().cloned());
    if !same {
        g.set_devices(ModelRc::new(VecModel::from(rows)));
    }
    // "Live" in the bottom bar: the PC is streaming frames to the devices (this window or the engine).
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
    // On the keyboard the reported "layer" is Fn lock (0 off, 1 on); -1 tells the page it's unknown.
    g.set_fn_lock(ready(&status, DeviceKind::Keyboard).and_then(|d| d.info.layer).map_or(-1, |l| l as i32));
    // Audio mode
    if st.audio_on && !status.audio && status.audio_problem.is_some() {
        // The engine tried and reported a problem (no sound device, for example): show it as off.
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
    show_firmware_builds(app, st, &status);

    // The keyboard picture, on the pages that show it
    let page = g.get_page().to_string();
    // These pages show device-dependent key details, which can change as devices come and go.
    if matches!(page.as_str(), "keys" | "home" | "macropad") {
        keyedit::push(app, st);
    }
    let visible = app.window().is_visible() && !app.window().is_minimized();
    // Redraw the picture only as often as this PC keeps up with, so a slow machine never
    // spends all its time drawing and stops answering clicks; and at most about 15 times a second,
    // which is plenty for a preview and leaves the window free to follow moves and resizes.
    let due = st.last_draw.elapsed() >= st.draw_cost.mul_f32(3.0).max(Duration::from_millis(66));
    if due && visible && matches!(page.as_str(), "lighting" | "audio" | "macropad" | "setup" | "keys" | "home") {
        let started = Instant::now();
        let scale = app.window().scale_factor();
        // Draw at the screen's real pixel size (logical width times the scale factor), within
        // 300..1400 pixels so a tiny or huge window can't make drawing silly or slow.
        let width = ((g.get_deck_width() * scale).round() as u32).clamp(300, 1400);
        // Take the preview out of the state for the duration of the frame, because computing its
        // colors needs `&mut preview` while `st.scene` borrows the rest of the state. Put back below.
        let mut preview = std::mem::take(&mut st.preview);
        let scene = st.scene(&page);
        let mut placed = deck::place_keys(&scene.setup, scene.macropad_layer);
        let shell = deck::shell(&scene.setup);
        // Key colors: the Keys page's changed/default coloring, else the audio show's last frame
        // when it runs, else the simulated effect.
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
        // `render::draw` produces premultiplied RGBA (color already multiplied by alpha), which is
        // what Slint's fastest image path expects.
        g.set_deck_image(slint::Image::from_rgba8_premultiplied(buffer));
        // Key labels are drawn by Slint as text on top of the picture (crisper than drawing text
        // into the pixels); their positions are fractions of the picture's size.
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
        // The small caption under the picture, joined with " · ".
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
        // Remember how long this took, for the "redraw only as often as the PC keeps up" rule.
        st.draw_cost = started.elapsed();
        st.last_draw = Instant::now();
    }

    // Save the settings file (keyboard.txt, a small text file) whenever anything in them changed this
    // frame; comparing to the last saved copy avoids needless writes. Write errors are ignored inside
    // `settings::save`: the settings simply stay unsaved until the next change.
    if st.s != st.last_saved {
        settings::save(&crate::paths::keyboard_file(), &st.s);
        st.last_saved = st.s.clone();
    }
}

/// Audio mode's colors into the Audio page.
///
/// The color scheme becomes an index 0..5 for the radio buttons. A single color is stored as hue and
/// saturation fractions 0..1 and shown on the 0..255 sliders; for the other schemes the sliders keep
/// what they had, so switching back to "one color" remembers it.
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
///
/// Row 0 is always "Default microphone" (an empty `audio_device` in the settings). The music page's
/// source list gets the same names, after its own "what the PC plays" entry.
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
    let mut sources: Vec<SharedString> = vec![crate::pages::music::PLAYBACK_SOURCE.into()];
    sources.extend(names.iter().cloned());
    g.set_audio_sources(ModelRc::new(VecModel::from(sources)));
    g.set_audio_devices(ModelRc::new(VecModel::from(names)));
    g.set_audio_device(index as i32);
    g.set_audio_input(st.s.audio_mic as i32);
}

/// Puts one device's LED calibration into the window's sliders.
///
/// When the device can't report calibration (older firmware), neutral values are shown (current 104,
/// all gains 255) and the sliders are marked unknown so the page greys them out.
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
///
/// None when the device's calibration is unknown (so nothing is sent). Values are clamped to 1..255:
/// a current or gain of 0 would turn a color, or the whole device, off. `default_current` is not
/// sent to the device, so 0 is a placeholder.
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

/// The app theme's accent color as RGB, for the selected-key outline in the picture.
fn g_accent(app: &AppWindow) -> [u8; 3] {
    let c = app.global::<Theme>().get_accent();
    [c.red(), c.green(), c.blue()]
}

thread_local! {
    /// The keyboard part's state and its frame timer, while the window is open.
    ///
    /// Thread-local because Slint objects (the timer) and the `Rc` state belong to the UI thread.
    /// Keeping the timer here keeps it alive; dropping it (in `stop`) stops the ticks.
    static RUNNING: RefCell<Option<(Rc<RefCell<State>>, slint::Timer)>> = const { RefCell::new(None) };
}

/// Pretend keyboard and macropad (no hardware needed): `--demo`, or WHITE_RABBIT_FAKE_KEYBOARD=1.
///
/// In demo mode the engine invents devices, firmware checks and updates are simulated, and the
/// background part is not asked to hand over the keyboard. Used for screenshots and trying the pages.
pub fn demo() -> bool {
    std::env::args().any(|a| a == "--demo") || std::env::var_os("WHITE_RABBIT_FAKE_KEYBOARD").is_some()
}

/// Closing the window: hand the lights back to the devices' saved effects, save the settings, and give
/// the keyboard to White Rabbit's background part, which carries on with an app-drawn effect or the
/// audio show.
pub fn stop() {
    if let Some((st, timer)) = RUNNING.with(|r| r.borrow_mut().take()) {
        // Stop the ticks first so nothing touches the state while it is being shut down.
        drop(timer);
        // If a callback still holds the state (closing from inside one), skip the clean-up rather
        // than panic; the process is about to end anyway.
        let Ok(mut st) = st.try_borrow_mut() else { return };
        st.bg.close();
        // Stops the engine thread; per the doc above, the devices go back to their saved effects.
        st.engine.shutdown();
        if st.s != st.last_saved {
            settings::save(&crate::paths::keyboard_file(), &st.s);
            st.last_saved = st.s.clone();
        }
        if !st.demo {
            // Tell the background part it may drive the keyboard again, and whether the audio show
            // was on so it can carry it on. Waits up to 2 s; no answer (monitor not running) is fine.
            crate::link::request(
                serde_json::json!({"cmd": "keyboard", "hold": false, "audio": st.audio_on}),
                Duration::from_secs(2),
            );
        }
    }
}

/// Asks White Rabbit's background part to stop driving the keyboard while the window is open.
/// True when its audio show was on, so the window carries on with it.
///
/// Sends `{"cmd":"keyboard","hold":true,"pid":…}` to the monitor over the local link and waits up to
/// 2 s. Our process id lets the monitor take the keyboard back if this window dies without saying so.
/// No reply (monitor not running) counts as "audio show was off".
fn take_keyboard() -> bool {
    let reply = crate::link::request(
        serde_json::json!({"cmd": "keyboard", "hold": true, "pid": std::process::id()}),
        Duration::from_secs(2),
    );
    reply.and_then(|r| r.get("audio").and_then(|a| a.as_bool())).unwrap_or(false)
}

/// Ctrl+Alt+Shift+M was pressed (set by the shortcut watcher, picked up by `tick`).
///
/// An atomic flag because the shortcut is seen on the keyboard hook's thread, not the UI thread.
static AUDIO_SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts the keyboard part: finds the devices, fills the keyboard pages and answers them.
///
/// Called once from `main` after the window is built. See the file header for the overall design.
/// Side effects: talks to the monitor, starts the engine thread and the shortcut hook, reads the
/// keyboard settings file, and starts a background microphone search.
pub fn wire(app: &AppWindow) {
    let demo = demo();
    // Take the keyboard from the background part first, so two programs never drive it at once.
    let audio_was_on = !demo && take_keyboard();
    // The shortcut callback runs on another thread, so it only sets a flag; `tick` acts on it.
    wr_keyboard::shortcut::watch(|| AUDIO_SHORTCUT.store(true, std::sync::atomic::Ordering::Release));
    let weak = app.as_weak();
    let engine = engine::start(
        move || {
            // The engine calls this from its thread when its status changes: an empty job queued on
            // the UI thread just wakes the event loop. Fails harmlessly if the window is gone.
            let _ = weak.upgrade_in_event_loop(|_| {});
        },
        demo,
    );
    // keyboard.txt in White Rabbit's settings folder. If it doesn't exist yet, `settings::load` copies
    // in the stand-alone keyboard app's settings, or starts from the defaults.
    let s = settings::load(&crate::paths::keyboard_file());
    let bg = winsys::Background::start();
    let st = Rc::new(RefCell::new(State {
        engine,
        bg,
        ke: keyedit::KeyEdit::default(),
        flashing: None,
        flash_inbox: Arc::new(Mutex::new(vec![])),
        restore_keys: None,
        // "Long ago", so the first tick on the Firmware page scans for drives at once.
        last_drive_scan: Instant::now() - Duration::from_secs(60),
        fw_newest: [None, None],
        fw_inbox: Arc::new(Mutex::new(None)),
        fw_checking: false,
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
    // Fill the lists that never change, then the current settings, before the first tick.
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
        // The background part had the audio show running: carry it on in the window seamlessly.
        if audio_was_on {
            st.set_audio(true);
        }
    }
    let g = app.global::<App>();
    g.set_legends(ModelRc::from(st.borrow().legends.clone()));
    if cfg!(windows) {
        // "This app" for an Open an app key: White Rabbit, which opens this window
        g.set_this_app_path(crate::paths::app_dir().join("WhiteRabbit.exe").display().to_string().into());
    }

    // A callback that reads the window, changes the state and refreshes what depends on it.
    // Every callback has the same shape: upgrade the weak window handle (the window may be closing),
    // borrow the state mutably for the duration of the body, run it. Holding only weak handles avoids
    // a reference cycle between the window and the closures it stores.
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

    // Setup page: keyboard type and side module changed.
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
    // Lighting page: effect, color, brightness or speed changed. Sent on the next tick (`dirty`).
    on!(on_show_changed, |app, st| {
        pull_all(&app, st);
        push_derived(&app, st);
        st.dirty = true;
    });
    // "One effect across both" switched; joining copies the keyboard's look to the macropad.
    on!(on_join_toggled, |app, st, on: bool| {
        pull_all(&app, st);
        st.s.join = on;
        if on {
            st.s.module = st.s.kb;
        }
        push_all(&app, st);
        st.dirty = true;
    });
    // Which device the Lighting page edits: 0 both (joined), 1 keyboard, 2 side module.
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
    // How painted keys behave while an effect runs: 0 steady color, 1 follow the effect's
    // brightness, 2 tint the effect's color (see `PaintMode`).
    on!(on_paint_mode_chosen, |app, st, mode: i32| {
        st.s.paint_mode = match mode {
            0 => PaintMode::Steady,
            2 => PaintMode::Tint,
            _ => PaintMode::Follow,
        };
        push_all(&app, st);
        st.dirty = true;
    });
    // "Save as default" (save true) or a direct send: sent at once rather than on the next tick.
    on!(on_send_show, |app, st, save: bool| {
        pull_all(&app, st);
        st.send_current(save);
    });
    // Stop the PC-drawn effect; the devices go back to what their firmware runs.
    on!(on_stop_pc, |_app, st| {
        st.engine.send(Command::SetLinked(None));
        st.pc_live = false;
    });
    // "Paint an example": colors W, A, S, D and the four arrow keys with the brush.
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
    // "Clear painting": forget every painted key on both devices.
    on!(on_paint_clear, |app, st| {
        st.s.paint.clear();
        push_derived(&app, st);
        st.dirty = true;
    });
    // A click or drag on the picture. `xf`, `yf` are fractions of the picture's width (both, so the
    // scale is the same on both axes); `kind` 0 is a left-button press, 1 a move while pressed
    // (only painting follows drags).
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
    // Macropad layer A/B buttons; remembered locally too, for when the macropad can't report it.
    on!(on_layer_chosen, |_app, st, layer: i32| {
        st.local_layer = layer as u8;
        st.engine.send(Command::SetLayer(layer as u8));
    });
    // Fn lock on: the keyboard's top row sends F1..F12 without Fn; off: media and brightness keys.
    on!(on_fn_lock_chosen, |_app, st, on: bool| {
        st.engine.send(Command::SetFnLock(on));
    });
    // Turns the Fn lighting keys off/on so they can't change the lights by accident.
    on!(on_lock_toggled, |_app, st, on: bool| {
        st.engine.send(Command::SetHotkeyLock(on));
    });
    // "Restart into flashing mode" for installing firmware by hand; `false` means not part of an
    // app update, so the engine tells the person to drag the .uf2 onto the drive.
    on!(on_bootloader, |_app, st, which: i32| {
        st.engine
            .send(Command::Bootloader(if which == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad }, false));
    });
    // Firmware page, "Update firmware": the newest GitHub build onto the running device.
    on!(on_flash_latest, |app, st, which: i32| {
        st.start_flash(&app, which, flash::Source::Latest, flash::Target::Running);
    });
    // Firmware page, "Install a file…": a .uf2 picked on this computer onto the running device.
    on!(on_flash_file, |app, st, which: i32| {
        if let Some(p) = winsys::browse_for_firmware() {
            st.start_flash(&app, which, flash::Source::File(p.into()), flash::Target::Running);
        }
    });
    // "Install on drive E:": the newest GitHub firmware onto a device already in flashing mode. If
    // the drive has gone in the meantime, just clear the offer.
    on!(on_flash_waiting, |app, st, which: i32| {
        let Some(drive) = flash::waiting_drives().into_iter().next() else {
            app.global::<App>().set_waiting_drive("".into());
            return;
        };
        st.start_flash(&app, which, flash::Source::Latest, flash::Target::Drive(drive));
    });
    // "Check for updates": both devices' newest builds are looked up on a background thread (two
    // downloads); the answer goes into `fw_inbox` and `show_firmware_builds` shows it.
    on!(on_check_firmware, |app, st| {
        if st.fw_checking {
            return;
        }
        st.fw_checking = true;
        app.global::<App>().set_fw_checking(true);
        let inbox = st.fw_inbox.clone();
        let wake = app.as_weak();
        let demo = st.demo;
        std::thread::spawn(move || {
            let found = if demo {
                let n = update::Newest {
                    label: "the newest firmware build 14".into(),
                    build: Some(wr_keyboard::protocol::FirmwareBuild::Build(14)),
                };
                [Ok(n.clone()), Ok(n)]
            } else {
                [update::newest(DeviceKind::Keyboard), update::newest(DeviceKind::Macropad)]
            };
            *inbox.lock().unwrap() = Some(found);
            let _ = wake.upgrade_in_event_loop(|_| {});
        });
    });
    // "Search again": the engine closes and reopens the USB devices.
    on!(on_rescan, |_app, st| {
        st.engine.send(Command::Rescan);
    });
    // Matching the two devices' brightness
    // A calibration slider moved: sent live (unsaved) now, saved by `tick` once it rests 800 ms.
    on!(on_cal_changed, |app, st, which: i32| {
        let i = which.clamp(0, 1) as usize;
        let kind = if i == 0 { DeviceKind::Keyboard } else { DeviceKind::Macropad };
        if let Some(cal) = cal_from_window(&app, i) {
            st.engine.send(Command::SetLedCal { kind, cal, save: false });
            st.cal_dirty[i] = Some(Instant::now());
        }
    });
    // Calibration back to the firmware's own current and full gains, saved at once.
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
    // Plain white on both devices while matching their brightness; off resumes the chosen look.
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
        // Plain functions of the text, no state needed; -1 means "not a color code".
        g.on_hex_hue(|t| parse_hex(&t).map_or(-1, |(h, _)| h as i32));
        g.on_hex_sat(|t| parse_hex(&t).map_or(-1, |(_, s)| s as i32));
    }
    // Keys page
    // The Keys page's callbacks below each forward to `keyedit` and then refresh the page with
    // `keyedit::push`. The two "edited" text boxes skip the refresh so typing isn't disturbed.
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
    // The audio show's on/off switch: the engine starts or stops listening and drawing.
    on!(on_audio_toggled, |app, st, on: bool| {
        st.set_audio(on);
        app.global::<App>().set_audio_on(on);
    });
    // The page's own shortcut handler (used while the window has focus): toggles the show.
    on!(on_audio_shortcut, |app, st| {
        let on = !st.audio_on;
        st.set_audio(on);
        app.global::<App>().set_audio_on(on);
    });
    // Look and sensitivity (1..10, 5 normal); a running show restarts with them.
    on!(on_audio_changed, |app, st| {
        let g = app.global::<App>();
        if let Some(l) = wr_keyboard::audio::AudioLook::from_key(&g.get_audio_look()) {
            st.s.audio_look = l;
        }
        st.s.audio_sensitivity = g.get_audio_sensitivity().clamp(1, 10) as u8;
        st.audio_settings_changed();
    });
    // What to listen to: 0 the sound the PC plays, 1 a microphone.
    on!(on_audio_input_chosen, |app, st, i: i32| {
        st.s.audio_mic = i == 1;
        push_mics(&app, st);
        st.audio_settings_changed();
    });
    // Row 0 is the default microphone, stored as an empty name.
    on!(on_audio_device_chosen, |app, st, i: i32| {
        let names = app.global::<App>().get_audio_devices();
        st.s.audio_device = match names.row_data(i.max(0) as usize) {
            Some(n) if i > 0 => n.to_string(),
            _ => String::new(),
        };
        st.audio_settings_changed();
    });
    // "Refresh" next to the microphone list: search again in the background.
    on!(on_audio_devices_refresh, |_app, st| {
        st.find_microphones();
    });
    // Radio button index 0..5 to a color scheme; "one color" takes the sliders as 0..1 fractions.
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
    // How keys react to typing during the show; an out-of-range index gives the default.
    on!(on_audio_keys_chosen, |app, st, i: i32| {
        st.s.audio_keys = KeyReact::ALL.get(i.max(0) as usize).copied().unwrap_or_default();
        app.global::<App>().set_audio_keys(i);
        st.audio_settings_changed();
    });
    // The Ctrl+Alt+Shift+M switch: saved in the settings and flipped in the shortcut hook at once.
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
        // 50 ms = 20 ticks a second. `try_borrow_mut` skips a tick if a callback is still running
        // (for example one waiting on a modal file dialog) instead of panicking.
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
