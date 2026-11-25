//! Background thread that owns the USB devices: applies settings, streams
//! Linked frames and keeps a status snapshot for the UI.
//!
//! Who uses it: the window (while it is open) and the monitor's keyboard job (while the window is
//! closed) each call [`start`], which spawns one thread named `hid-engine`. Only one program may
//! drive the devices at a time; the monitor's "keyboard" control command hands them over.
//!
//! How it talks to the rest of the program:
//!
//! * In: [`Command`]s over an `mpsc` channel ([`EngineHandle::send`]). Commands never block the
//!   caller; the engine works through every queued command at the start of each loop.
//! * Out: a [`Status`] snapshot behind an `Arc<Mutex<..>>` that the UI reads whenever it likes,
//!   plus a `repaint` callback the engine calls after changing it, so the UI knows to redraw.
//!   The lock is only held for short copies, never while talking to USB.
//!
//! The loop (see `Engine::run`), every 33 ms while the PC draws the lights, else every 50 ms:
//!
//! 1. handle queued commands (a [`Command::Shutdown`] or a dropped handle ends the thread);
//! 2. if a device is missing, scan USB for it, at most every 2.5 s (unplugging is noticed when a
//!    message to the device fails; see `Engine::fail`);
//! 3. draw and send one frame if the audio show or a Linked (PC-drawn) effect is running;
//! 4. while the devices are joined, line up their effect clocks every 10 s;
//! 5. once a second, read each device's INFO and STATE into the status snapshot.
//!
//! Everything that talks to a device blocks this thread (up to 250 ms per answer), which is why it
//! has a thread of its own: the window never waits on USB. Any device error is treated as an
//! unplug: the device is dropped from `open` and found again by the next scan.
//!
//! When the engine stops (window closed, `Shutdown`), it turns host mode off so both devices go
//! back to their own saved effects.

use crate::audio::{AudioColors, AudioLook, AudioShow, KeyReact};
use crate::audio_src::Listener;
use crate::device::{self, Device, Found};
use crate::effects::{EffectParams, Renderer, Rgb};
use crate::keycodes::{KeyWrite, Keymap};
use crate::layout::{CombinedLayout, MacropadSide, Point};
use crate::protocol::{self as proto, DeviceInfo, DeviceKind, LedPoint, LightState};
use crate::sim::SimPort;
use hidapi::HidApi;
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Loop period while the PC is drawing (Linked effects or the audio show): about 30 frames a
/// second, enough for smooth motion within the USB message budget (16 reports per frame).
const FRAME_INTERVAL: Duration = Duration::from_millis(33); // ~30 fps
/// Loop period otherwise: commands are picked up within 50 ms, so sliders feel immediate.
const IDLE_INTERVAL: Duration = Duration::from_millis(50);
/// How often the status snapshot is re-read from the devices (INFO + STATE), in case the user
/// changed something on the device itself (Fn + Esc, lighting keys).
const REFRESH_INTERVAL: Duration = Duration::from_millis(1000);
/// How often to look for a missing device. Scanning enumerates every HID device, so not too often.
const SCAN_INTERVAL: Duration = Duration::from_millis(2500);
/// How often joined devices get their effect clocks lined up again.
const CLOCK_SYNC_INTERVAL: Duration = Duration::from_secs(10);

/// A request to the engine thread, sent with [`EngineHandle::send`]. Commands for a device that is
/// not connected are quietly ignored. Failures show up in [`Status::message`].
pub enum Command {
    /// Built-in effect settings for one device. `save` = write to EEPROM.
    SetState {
        kind: DeviceKind,
        state: LightState,
        save: bool,
    },
    /// Hotkey lock on every connected device.
    SetHotkeyLock(bool),
    /// Macropad layer: 0 = MIDI, 1 = numpad.
    SetLayer(u8),
    /// Keyboard Fn lock: true = the top row sends F1-F12 (media keys with Fn), false = media keys
    /// (F1-F12 with Fn). The same as pressing Fn + Esc; the keyboard remembers it.
    SetFnLock(bool),
    /// Some = PC renders this effect on both devices, None = back to built-in effects.
    SetLinked(Option<EffectParams>),
    SetSide(MacropadSide),
    /// Restart a device into flashing mode (works on stock firmware too).
    /// `by_app`: the app copies the firmware on itself (Firmware page "Update firmware").
    Bootloader(DeviceKind, bool),
    /// Joined surface: Some((x_start, x_end)) per device, None = separate.
    /// While joined, the effect clocks are kept in step.
    SetSpan {
        keyboard: Option<(u8, u8)>,
        macropad: Option<(u8, u8)>,
    },
    /// Painted keys for one device (one entry per LED). `save` = keep after unplugging.
    SetPaint {
        kind: DeviceKind,
        leds: Vec<proto::Paint>,
        mode: proto::PaintMode,
        save: bool,
    },
    /// LED current and color gains. `save` = keep after a restart.
    SetLedCal {
        kind: DeviceKind,
        cal: proto::LedCal,
        save: bool,
    },
    /// Audio mode: Some = draw this audio show on both devices, None = stop.
    SetAudio(Option<AudioSettings>),
    /// Change keys in a device's keymap (saved on the device right away).
    SetKeys {
        kind: DeviceKind,
        writes: Vec<KeyWrite>,
    },
    /// Macropad corner combos (saved on the macropad).
    SetCombos(proto::Combos),
    /// The keyboard's F12 shortcuts on or off (saved on the keyboard).
    SetShortcuts(bool),
    /// Look for devices now instead of waiting for the next scan.
    Rescan,
    /// Give the lights back to the devices and end the thread (see [`EngineHandle::shutdown`]).
    Shutdown,
}

/// Everything the audio show needs; the window builds it from the Audio page and settings.
#[derive(Clone, PartialEq, Debug)]
pub struct AudioSettings {
    /// Which show to draw.
    pub look: AudioLook,
    /// 1..10, 5 = normal.
    pub sensitivity: u8,
    /// Listen to a microphone instead of what the PC plays.
    pub microphone: bool,
    /// Which microphone ("" = the default one).
    pub device: String,
    /// The palette the show is drawn in.
    pub colors: AudioColors,
    /// How key presses show on top of the show.
    pub keys: KeyReact,
}

impl AudioSettings {
    /// Starts listening as these settings say. `demo` uses made-up music instead of real sound.
    /// The error is a sentence for the Audio page (no sound device, capture failed...).
    fn listener(&self, demo: bool) -> Result<Listener, String> {
        Listener::start(demo, self.sensitivity, self.microphone, &self.device)
    }
}

/// A running audio show: the settings it started with, what it listens to, the show's state, and
/// when the last frame was drawn (to work out the time step).
struct AudioRun {
    settings: AudioSettings,
    listener: Listener,
    show: AudioShow,
    last: Instant,
}

/// What the UI knows about one connected device running our firmware, refreshed about once a
/// second. Cloned out of the engine, so the UI can keep it as long as it likes.
#[derive(Clone)]
pub struct DeviceStatus {
    /// The latest INFO answer (Fn lock / layer, hotkey lock, LED count, firmware build).
    pub info: DeviceInfo,
    /// The device's own effect settings, as read back (shows the host effect while the PC draws).
    pub state: LightState,
    /// The device says the PC is drawing its lights.
    pub streaming: bool,
    /// LED positions, used to match keys to LEDs.
    pub leds: Vec<LedPoint>,
    /// What each key sends; None if it could not be read.
    pub keymap: Option<Keymap>,
    /// Macropad corner combos; None on the keyboard and on older macropad firmware.
    pub combos: Option<proto::Combos>,
    /// LED calibration; None on older firmware.
    pub led_cal: Option<proto::LedCal>,
    /// Keyboard F12 shortcuts on/off; None on the macropad and older firmware.
    pub shortcuts: Option<bool>,
    /// USB serial number, used to remember each keyboard's type (clear / black / white keycaps).
    pub serial: Option<String>,
}

/// One device's place in the status: missing, wrong firmware, or ready.
#[derive(Clone, Default)]
pub enum Slot {
    /// Not plugged in (or not found yet).
    #[default]
    Missing,
    /// Found, but it is not running the f16custom firmware.
    Stock,
    Ready(DeviceStatus),
}

/// The engine's snapshot for the UI. Written only by the engine thread, read by anyone holding
/// [`EngineHandle::status`]. Keep reads short: the engine waits for the lock.
#[derive(Clone, Default)]
pub struct Status {
    pub keyboard: Slot,
    pub macropad: Slot,
    /// A Linked (PC-drawn) effect is running. False while the audio show runs.
    pub linked: bool,
    /// Linked mode preview: LED position + color.
    pub preview: Vec<(Point, Rgb)>,
    /// Width and height of the combined layout, in key units (see [`CombinedLayout`]).
    pub layout_size: (f32, f32),
    /// Frames actually sent per second in Linked mode (measured, updated once a second).
    pub fps: f32,
    /// The last thing worth telling the user (saved, disconnected, could not update...). Stays
    /// until replaced.
    pub message: Option<String>,
    /// A Framework numpad is plugged in.
    pub numpad: bool,
    /// Audio mode is drawing.
    pub audio: bool,
    /// What audio mode listens to, or why it can't.
    /// Name of the sound device being listened to.
    pub audio_device: Option<String>,
    /// Why the audio show can't hear anything, if it can't.
    pub audio_problem: Option<String>,
    /// How loud the sound is right now (0..1), for the meter on the Audio page.
    pub audio_level: f32,
    /// Audio mode's last frame per device (LED order): [keyboard, macropad].
    pub audio_colors: [Vec<Rgb>; 2],
}

impl Status {
    /// The slot of one device.
    pub fn slot(&self, kind: DeviceKind) -> &Slot {
        match kind {
            DeviceKind::Keyboard => &self.keyboard,
            DeviceKind::Macropad => &self.macropad,
        }
    }
    /// The slot of one device, for changing.
    pub fn slot_mut(&mut self, kind: DeviceKind) -> &mut Slot {
        match kind {
            DeviceKind::Keyboard => &mut self.keyboard,
            DeviceKind::Macropad => &mut self.macropad,
        }
    }
}

/// One device the engine has open, plus what it read from it when it was found. `keymap`,
/// `combos`, `led_cal` and `shortcuts` are the engine's copies, updated after each successful
/// write so the status doesn't need to re-read them from the device.
struct Open {
    dev: Device,
    info: DeviceInfo,
    leds: Vec<LedPoint>,
    keymap: Option<Keymap>,
    combos: Option<proto::Combos>,
    led_cal: Option<proto::LedCal>,
    shortcuts: Option<bool>,
}

/// What [`start`] returns: the way in (commands) and the way out (status). Dropping it shuts the
/// engine down and waits for its thread to finish.
pub struct EngineHandle {
    /// Command channel into the engine thread.
    pub tx: Sender<Command>,
    /// The engine's status snapshot, shared with the engine thread.
    pub status: Arc<Mutex<Status>>,
    /// The engine thread; taken (set to None) once it has been joined.
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Queues a command. Never blocks. If the engine thread has already ended the command is
    /// dropped silently (nothing could act on it anyway).
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// Restores the built-in effects and stops the thread.
    /// Blocks until the thread has finished its last messages to the devices (normally a few
    /// milliseconds). Safe to call more than once.
    pub fn shutdown(&mut self) {
        // The send fails only if the thread already ended; then there is nothing to stop.
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            // A panic on the engine thread shows up here as Err; there is nothing useful to do with it
            // while closing, so it is ignored.
            let _ = t.join();
        }
    }
}

impl Drop for EngineHandle {
    // Runs when the window closes: hands the lights back to the devices' saved effects.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `demo = true` uses the built-in simulator instead of USB devices.
///
/// Spawns the `hid-engine` thread and returns at once; devices are found on its first loop.
/// `repaint` is called from the engine thread whenever [`Status`] changed, so it must only
/// schedule a redraw (for example post to the UI's event loop), not draw itself.
/// Panics only if the operating system can't create a thread.
pub fn start(repaint: impl Fn() + Send + 'static, demo: bool) -> EngineHandle {
    let (tx, rx) = mpsc::channel();
    let status = Arc::new(Mutex::new(Status::default()));
    let st = status.clone();
    let thread = thread::Builder::new()
        .name("hid-engine".into())
        .spawn(move || Engine::new(rx, st, Box::new(repaint), demo).run())
        .expect("spawn engine thread");
    EngineHandle { tx, status, thread: Some(thread) }
}

/// Pretend devices for `--demo`.
/// Built fresh each time a scan finds them missing; `skip` leaves out the ones already open.
fn demo_devices(skip: &[DeviceKind]) -> Vec<Found> {
    [(DeviceKind::Keyboard, SimPort::keyboard()), (DeviceKind::Macropad, SimPort::macropad())]
        .into_iter()
        .filter(|(kind, _)| !skip.contains(kind))
        .map(|(kind, port)| device::identify(Device::new(kind, Box::new(port))))
        .collect()
}

/// The engine's state, owned by the `hid-engine` thread (nothing else can see it).
struct Engine {
    /// Commands from the UI.
    rx: Receiver<Command>,
    /// The snapshot shared with the UI.
    status: Arc<Mutex<Status>>,
    /// Tells the UI to redraw after the status changed.
    repaint: Box<dyn Fn() + Send>,
    /// hidapi context; None in demo mode or if USB HID access could not start.
    api: Option<HidApi>,
    /// Use the simulator (and pretend music) instead of real devices.
    demo: bool,
    /// Devices open and running our firmware. A BTreeMap so the keyboard (0) always goes first.
    open: BTreeMap<u8, Open>, // key: 0 keyboard, 1 macropad
    /// Devices found on the last scan that run other firmware.
    stock: Vec<DeviceKind>,
    /// Both devices are set to one joined surface (their effect clocks are kept in step).
    joined: bool,
    /// When the effect clocks were last sent.
    last_clock_sync: Instant,
    /// Zero point of the shared effect clock (engine start).
    t0_clock: Instant,
    /// The Linked effect being drawn, if any.
    linked: Option<EffectParams>,
    /// The audio show being drawn, if any. At most one of `linked` and `audio` is set.
    audio: Option<AudioRun>,
    /// Draws the Linked effects and keeps their ripples.
    renderer: Renderer,
    /// Which side of the keyboard the macropad is on (for the combined layout).
    side: MacropadSide,
    /// Both devices' LED positions in one space, in key units.
    layout: CombinedLayout,
    /// Start of the current Linked effect (its time 0).
    t0: Instant,
    /// When USB was last scanned; None = scan on the next loop.
    last_scan: Option<Instant>,
    /// When the status was last read from the devices.
    last_refresh: Instant,
    /// Frames sent since `fps_timer`, for the fps figure.
    frames: u32,
    fps_timer: Instant,
}

/// The key of a device in `Engine::open`.
fn key(kind: DeviceKind) -> u8 {
    match kind {
        DeviceKind::Keyboard => 0,
        DeviceKind::Macropad => 1,
    }
}

impl Engine {
    /// Runs on the engine thread. If hidapi can't start, the engine still runs (it just never finds
    /// anything) and the reason is shown in the status.
    fn new(rx: Receiver<Command>, status: Arc<Mutex<Status>>, repaint: Box<dyn Fn() + Send>, demo: bool) -> Self {
        // No hidapi in demo mode: the simulator stands in for USB.
        let api = match if demo { None } else { Some(HidApi::new()) } {
            None => None,
            Some(Ok(a)) => Some(a),
            Some(Err(e)) => {
                status.lock().unwrap().message = Some(format!("Could not start USB HID access: {e}"));
                None
            }
        };
        Self {
            rx,
            status,
            repaint,
            api,
            demo,
            open: BTreeMap::new(),
            stock: Vec::new(),
            joined: false,
            last_clock_sync: Instant::now(),
            t0_clock: Instant::now(),
            linked: None,
            audio: None,
            renderer: Renderer::default(),
            side: MacropadSide::default(),
            layout: CombinedLayout::default(),
            t0: Instant::now(),
            last_scan: None,
            last_refresh: Instant::now(),
            frames: 0,
            fps_timer: Instant::now(),
        }
    }

    /// Shows `m` in the status and asks the UI to redraw.
    fn set_message(&self, m: impl Into<String>) {
        self.status.lock().unwrap().message = Some(m.into());
        (self.repaint)();
    }

    /// The engine's main loop (see the file header). Returns, ending the thread, on `Shutdown` or
    /// when every [`EngineHandle`] sender has been dropped.
    fn run(mut self) {
        loop {
            let tick = Instant::now();
            // Step 1: drain every queued command without blocking (try_recv), so a burst of slider
            // changes is applied in one loop rather than one per 50 ms.
            loop {
                match self.rx.try_recv() {
                    Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                        self.stop_audio();
                        self.stop_linked();
                        return;
                    }
                    Ok(c) => self.handle(c),
                    Err(TryRecvError::Empty) => break,
                }
            }

            // Step 2: scan only while a device is missing, and not more often than SCAN_INTERVAL.
            // `last_scan == None` (start, or after Rescan) scans straight away.
            let all_ready = self.open.len() == 2;
            if !all_ready && self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
                self.scan();
            }
            // Step 3: the audio show wins over a Linked effect (starting one stops the other anyway).
            if self.audio.is_some() {
                self.render_audio();
            } else if self.linked.is_some() {
                self.render_frame();
            }
            // Step 4: keep joined effects in step (device clocks drift apart slowly).
            if self.joined && self.last_clock_sync.elapsed() >= CLOCK_SYNC_INTERVAL {
                self.sync_clocks();
            }
            // Step 5: re-read the devices for the status (and notice an unplug even when idle).
            if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh();
            }

            // Sleep for whatever is left of this loop's period. If the work took longer than the period
            // (slow USB), go round again at once rather than falling further behind.
            let interval = if self.linked.is_some() || self.audio.is_some() { FRAME_INTERVAL } else { IDLE_INTERVAL };
            if let Some(rest) = interval.checked_sub(tick.elapsed()) {
                thread::sleep(rest);
            }
        }
    }

    /// Carries out one command. Every device write goes through `transact` (wait for the answer);
    /// any error marks that device as gone (`fail`).
    fn handle(&mut self, c: Command) {
        match c {
            Command::SetState { kind, state, save } => {
                if let Some(o) = self.open.get(&key(kind)) {
                    // Validate the effect id first (no message is sent for a bad one), then send and wait.
                    let res = proto::set_state(&state, save, o.info.effect_count)
                        .map_err(device::DeviceError::from)
                        .and_then(|r| o.dev.transact(&r));
                    match res {
                        Ok(_) => {
                            if save {
                                self.set_message(format!("{} settings saved to the device.", kind.name()));
                            }
                        }
                        Err(e) => self.fail(kind, e),
                    }
                }
            }
            Command::SetHotkeyLock(on) => {
                // Collect the kinds first: `fail` needs `&mut self`, which can't happen while
                // iterating `open`.
                let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
                for kind in kinds {
                    let r = self.open[&key(kind)].dev.transact(&proto::set_hotkey_lock(on));
                    if let Err(e) = r {
                        self.fail(kind, e);
                    }
                }
                self.refresh();
            }
            Command::SetLayer(layer) => {
                if let Some(o) = self.open.get(&key(DeviceKind::Macropad)) {
                    if let Err(e) = o.dev.transact(&proto::set_layer(layer)) {
                        self.fail(DeviceKind::Macropad, e);
                    }
                    self.refresh();
                }
            }
            Command::SetFnLock(on) => {
                if let Some(o) = self.open.get_mut(&key(DeviceKind::Keyboard)) {
                    match device::set_fn_lock(&o.dev, o.keymap.as_mut(), on) {
                        // The firmware is too old to set Fn lock, or it didn't stick.
                        Ok(r) if !r.took => self.set_message(
                            "The keyboard didn't change its top row. Update the keyboard firmware on the Firmware page, then try again.",
                        ),
                        Ok(r) if r.fixed_keys > 0 => self.set_message(format!(
                            "Top row set. {} top-row key{} had the shortcut and the F-key the wrong way round, so they were put back.",
                            r.fixed_keys,
                            if r.fixed_keys == 1 { "" } else { "s" }
                        )),
                        Ok(_) => {}
                        Err(e) => {
                            self.fail(DeviceKind::Keyboard, e);
                            return;
                        }
                    }
                    self.refresh();
                }
            }
            // Already running: only restart the listener when what it listens to changed (restarting
            // drops a moment of sound); the look, colors and key style just change in place.
            Command::SetAudio(Some(settings)) => match self.audio.as_mut() {
                Some(run) => {
                    if run.settings.sensitivity != settings.sensitivity
                        || run.settings.microphone != settings.microphone
                        || run.settings.device != settings.device
                    {
                        run.listener = match settings.listener(self.demo) {
                            Ok(l) => l,
                            Err(e) => {
                                self.audio_failed(e);
                                return;
                            }
                        };
                    }
                    run.show.look = settings.look;
                    run.show.colors = settings.colors;
                    run.show.keys = settings.keys;
                    run.settings = settings;
                }
                // Not running yet: start listening first, and only take over the lights if that worked.
                None => match settings.listener(self.demo) {
                    Ok(listener) => {
                        // Linked and audio both use host mode; stop the Linked effect so only one draws.
                        self.stop_linked();
                        let mut show = AudioShow::new(settings.look);
                        show.colors = settings.colors;
                        show.keys = settings.keys;
                        self.audio = Some(AudioRun { settings, listener, show, last: Instant::now() });
                        self.start_linked();
                        let mut st = self.status.lock().unwrap();
                        // start_linked() set `linked` for the status; for the audio show the UI
                        // shows "audio" instead.
                        st.linked = false;
                        st.audio = true;
                        st.audio_problem = None;
                    }
                    Err(e) => self.audio_failed(e),
                },
            },
            Command::SetAudio(None) => self.stop_audio(),
            Command::SetLinked(Some(params)) => {
                // Switching to a Linked effect ends the audio show (host mode is turned off and on again).
                self.stop_audio();
                self.renderer.params = params;
                // Already drawing: just swap the parameters, the next frame uses them.
                if self.linked.is_none() {
                    self.linked = Some(params);
                    self.start_linked();
                } else {
                    self.linked = Some(params);
                }
            }
            Command::SetLinked(None) => self.stop_linked(),
            Command::SetSide(side) => {
                self.side = side;
                self.rebuild_layout();
            }
            Command::SetSpan { keyboard, macropad } => {
                // Joined only when both get a slice; a device that isn't joined gets its full 0..=224 back.
                self.joined = keyboard.is_some() && macropad.is_some();
                for (kind, span) in [(DeviceKind::Keyboard, keyboard), (DeviceKind::Macropad, macropad)] {
                    if let Some(o) = self.open.get(&key(kind)) {
                        let (x0, x1) = span.unwrap_or((0, 224));
                        if let Err(e) = o.dev.transact(&proto::set_span(span.is_some(), x0, x1)) {
                            self.fail(kind, e);
                        }
                    }
                }
                self.sync_clocks();
            }
            Command::SetPaint { kind, leds, mode, save } => {
                if let Some(o) = self.open.get(&key(kind)) {
                    // All paint messages, then the mode, then (optionally) save, sent back to back.
                    let mut msgs = proto::encode_paint(&leds);
                    msgs.push(proto::paint_ctrl(mode.op()));
                    if save {
                        msgs.push(proto::paint_ctrl(proto::PaintOp::Save));
                    }
                    if let Err(e) = o.dev.transact_many(&msgs) {
                        self.fail(kind, e);
                    }
                }
            }
            Command::SetKeys { kind, writes } => {
                if let Some(o) = self.open.get_mut(&key(kind)) {
                    let reqs: Vec<_> =
                        writes.iter().map(|&(l, r, c, code)| proto::set_keycode(l, r, c, code)).collect();
                    match o.dev.transact_many(&reqs) {
                        // Mirror the writes in the engine's keymap copy so the UI sees them
                        // without a re-read.
                        Ok(()) => {
                            if let Some(km) = o.keymap.as_mut() {
                                km.apply(&writes);
                            }
                            self.refresh();
                        }
                        Err(e) => self.fail(kind, e),
                    }
                }
            }
            Command::SetLedCal { kind, cal, save } => {
                if let Some(o) = self.open.get_mut(&key(kind)) {
                    match o.dev.transact(&proto::set_led_cal(&cal, save)) {
                        Ok(_) => o.led_cal = Some(cal),
                        Err(e) => self.fail(kind, e),
                    }
                }
            }
            Command::SetShortcuts(on) => {
                if let Some(o) = self.open.get_mut(&key(DeviceKind::Keyboard)) {
                    match o.dev.transact(&proto::set_shortcuts(on)) {
                        Ok(_) => {
                            o.shortcuts = Some(on);
                            self.refresh();
                        }
                        Err(e) => self.fail(DeviceKind::Keyboard, e),
                    }
                }
            }
            Command::SetCombos(c) => {
                if let Some(o) = self.open.get_mut(&key(DeviceKind::Macropad)) {
                    match o.dev.transact(&proto::set_combos(&c)) {
                        Ok(_) => {
                            o.combos = Some(c);
                            self.refresh();
                        }
                        Err(e) => self.fail(DeviceKind::Macropad, e),
                    }
                }
            }
            Command::Rescan => {
                self.last_scan = None;
            }
            Command::Bootloader(kind, by_app) => {
                // The device resets into its bootloader and disappears from USB, so stop using it now. If it
                // isn't open (stock firmware), open its raw interface just for this one message.
                let sent = match self.open.remove(&key(kind)) {
                    Some(o) => o.dev.send(&proto::bootloader_request()).is_ok(),
                    None => self
                        .api
                        .as_mut()
                        .and_then(|api| device::open_raw(api, kind))
                        .is_some_and(|d| d.send(&proto::bootloader_request()).is_ok()),
                };
                self.rebuild_layout();
                self.set_message(if sent && by_app {
                    format!("Updating the {} firmware…", kind.name().to_lowercase())
                } else if sent {
                    format!(
                        "{} restarted into flashing mode: drag the .uf2 file onto the new RPI-RP2 drive.",
                        kind.name()
                    )
                } else {
                    format!("Could not reach the {}.", kind.name())
                });
                // It will show up again as a new device once the new firmware starts.
                self.stock.retain(|k| *k != kind);
                self.refresh();
            }
            Command::Shutdown => unreachable!(),
        }
    }

    /// Sends the same clock to both devices back to back so joined effects line up.
    ///
    /// The two clock messages are sent without waiting in between, so they reach the devices within
    /// a fraction of a millisecond of each other. Their echoes are then cleared out by a STATE round
    /// trip per device: `recv_matching` skips the stale clock answers until the STATE answer comes.
    /// Errors are ignored; the next refresh notices an unplugged device.
    fn sync_clocks(&mut self) {
        self.last_clock_sync = Instant::now();
        // Milliseconds since the engine started; the `as u32` wraps after 49 days, which the firmware
        // copes with like its own timer wrapping.
        let ms = self.t0_clock.elapsed().as_millis() as u32;
        let msg = proto::set_clock(ms);
        for o in self.open.values() {
            let _ = o.dev.send(&msg);
        }
        for o in self.open.values() {
            let _ = o.dev.transact(&proto::state_request());
        }
    }

    /// Looks for devices that are not open yet and opens the ones running our firmware, reading
    /// their keymap, LED calibration and (per device) shortcuts or combos. Blocks this thread for
    /// the round trips (about 40 for the keyboard's keymap).
    fn scan(&mut self) {
        self.last_scan = Some(Instant::now());
        let skip: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        let found = if self.demo {
            demo_devices(&skip)
        } else if let Some(api) = self.api.as_mut() {
            let found = device::scan(api, &skip);
            self.status.lock().unwrap().numpad = device::numpad_present(api);
            found
        } else {
            return;
        };
        let mut stock = Vec::new();
        let mut added = false;
        for f in found {
            match f {
                Found::Ready(dev, info, leds) => {
                    let kind = dev.kind;
                    // A device plugged in while the PC is drawing joins in straight away.
                    // Errors are ignored here: if it fails, the first frame sent to it fails
                    // too and `fail` handles it.
                    if self.linked.is_some() || self.audio.is_some() {
                        let _ = dev.transact(&proto::set_host_mode(true));
                    }
                    // These extras are optional: None when the firmware is too old or a read fails.
                    let keymap = dev.keymap().ok();
                    let led_cal = dev.led_cal().ok().flatten();
                    let shortcuts = if kind == DeviceKind::Keyboard { dev.shortcuts().ok().flatten() } else { None };
                    let combos = if kind == DeviceKind::Macropad { dev.combos().ok().flatten() } else { None };
                    self.open.insert(key(kind), Open { dev, info, leds, keymap, combos, led_cal, shortcuts });
                    added = true;
                }
                Found::Stock(kind) => stock.push(kind),
            }
        }
        // Replaces the list: a stock device that was unplugged is no longer reported.
        self.stock = stock;
        if added {
            self.rebuild_layout();
        }
        self.refresh();
    }

    /// Recomputes the combined LED layout after a device came or went or the macropad changed sides.
    fn rebuild_layout(&mut self) {
        let kb = self.open.get(&0).map(|o| o.leds.as_slice()).unwrap_or(&[]);
        let pad = self.open.get(&1).map(|o| o.leds.as_slice()).unwrap_or(&[]);
        self.layout = CombinedLayout::new(kb, pad, self.side);
        self.status.lock().unwrap().layout_size = (self.layout.width, self.layout.height);
    }

    /// Something went wrong talking to a device: assume it was unplugged.
    /// Closes the device (dropping it closes the USB handle) so the next scan can find it again.
    fn fail(&mut self, kind: DeviceKind, e: device::DeviceError) {
        self.open.remove(&key(kind));
        self.rebuild_layout();
        self.status.lock().unwrap().message = Some(format!("{} disconnected: {e}", kind.name()));
        self.refresh();
    }

    /// Re-reads INFO and STATE from every open device and publishes a fresh [`Status`]. Also records
    /// which devices are missing or on stock firmware. Calls `repaint`.
    fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        let mut updates = Vec::new();
        for kind in kinds {
            let o = &self.open[&key(kind)];
            // Two round trips per device: INFO (Fn lock, hotkey lock...) and STATE (effect settings).
            let res = o.dev.info().and_then(|info| {
                let p = o.dev.transact(&proto::state_request())?;
                Ok((info, proto::parse_state(&p)))
            });
            match res {
                Ok((info, (state, streaming))) => {
                    let o = self.open.get_mut(&key(kind)).unwrap();
                    o.info = info.clone();
                    updates.push((
                        kind,
                        Slot::Ready(DeviceStatus {
                            info,
                            state,
                            streaming,
                            leds: o.leds.clone(),
                            keymap: o.keymap.clone(),
                            combos: o.combos,
                            led_cal: o.led_cal,
                            shortcuts: o.shortcuts,
                            serial: o.dev.serial.clone(),
                        }),
                    ));
                }
                Err(e) => {
                    self.fail(kind, e);
                    return; // fail() refreshes again
                }
            }
        }
        {
            let mut st = self.status.lock().unwrap();
            // Publish everything under one short lock, after all USB traffic is done: first mark every
            // device missing or stock, then fill in the ready ones. A poisoned lock (a UI thread panicked
            // while holding it) panics here too; the app is going down anyway.
            for kind in [DeviceKind::Keyboard, DeviceKind::Macropad] {
                *st.slot_mut(kind) = if self.stock.contains(&kind) { Slot::Stock } else { Slot::Missing };
            }
            for (kind, slot) in updates {
                *st.slot_mut(kind) = slot;
            }
            st.linked = self.linked.is_some();
        }
        (self.repaint)();
    }

    /// The audio show could not start listening: show why and leave it off.
    fn audio_failed(&mut self, e: String) {
        let mut st = self.status.lock().unwrap();
        st.audio = false;
        st.audio_problem = Some(format!("Can't listen to the sound: {e}"));
        drop(st);
        (self.repaint)();
    }

    /// Stops the audio show (if running) and gives the lights back to the devices' own effects.
    fn stop_audio(&mut self) {
        if self.audio.take().is_none() {
            return;
        }
        for o in self.open.values() {
            // Best effort: a device that fails here falls back by itself within a few seconds.
            let _ = o.dev.transact(&proto::set_host_mode(false));
        }
        let mut st = self.status.lock().unwrap();
        st.audio = false;
        st.audio_colors = Default::default();
        st.audio_level = 0.0;
        drop(st);
        (self.repaint)();
    }

    /// One audio mode frame: listen, update the show, draw it on both devices.
    fn render_audio(&mut self) {
        // Key presses since the last frame, for the key reactions on top of the show
        let mut presses = Vec::new();
        // Only poll for key presses when the show uses them: it costs a round trip per device.
        if self.audio.as_ref().is_some_and(|r| r.show.keys != KeyReact::Off || r.show.look == AudioLook::Typing) {
            let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
            for kind in kinds {
                match self.open[&key(kind)].dev.transact(&proto::key_events_request()) {
                    Ok(p) => presses
                        .extend(proto::parse_key_events(&p).into_iter().filter_map(|led| self.layout.led(kind, led))),
                    Err(e) => {
                        self.fail(kind, e);
                        return;
                    }
                }
            }
        }
        let Some(run) = self.audio.as_mut() else {
            return;
        };
        for at in presses {
            run.show.press(at);
        }
        // Seconds since the last frame, clamped so a stall (slow USB, a suspended PC) doesn't make the
        // show jump, and a zero step can't divide anything by zero.
        let dt = run.last.elapsed().as_secs_f32().clamp(0.001, 0.2);
        run.last = Instant::now();
        let bands = run.listener.bands(dt);
        run.show.update(&bands, dt);
        // Average band level, 0..1, for the meter on the Audio page.
        let level = bands.iter().sum::<f32>() / bands.len().max(1) as f32;
        let (device, problem) = (run.listener.device(), run.listener.problem());
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        let mut colors_out: [Vec<Rgb>; 2] = Default::default();
        for &kind in &kinds {
            let points = self.layout.points(kind).to_vec();
            // Both devices are drawn from one show on the combined layout, so a ring or bar runs on
            // across the gap between them.
            let colors = run.show.render(&points, self.layout.width, self.layout.height);
            let msgs = proto::encode_frame(&colors);
            colors_out[key(kind) as usize] = colors;
            if let Err(e) = self.open[&key(kind)].dev.transact_many(&msgs) {
                self.fail(kind, e);
                return;
            }
        }
        let mut st = self.status.lock().unwrap();
        st.audio_colors = colors_out;
        st.audio_level = level;
        st.audio_device = device;
        st.audio_problem = problem;
        drop(st);
        (self.repaint)();
    }

    /// Switches every open device to host mode so it shows the PC's frames. Used by both Linked
    /// effects and the audio show (the devices can't tell them apart).
    fn start_linked(&mut self) {
        // Linked effects are drawn from time 0 at each start.
        self.t0 = Instant::now();
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        for kind in kinds {
            let o = &self.open[&key(kind)];
            // throw away key presses from before Linked mode started
            let _ = o.dev.transact(&proto::key_events_request());
            if let Err(e) = o.dev.transact(&proto::set_host_mode(true)) {
                self.fail(kind, e);
            }
        }
        self.status.lock().unwrap().linked = true;
    }

    /// Stops a Linked effect (if running): host mode off, preview cleared.
    fn stop_linked(&mut self) {
        if self.linked.take().is_none() {
            return;
        }
        for o in self.open.values() {
            let _ = o.dev.transact(&proto::set_host_mode(false));
        }
        let mut st = self.status.lock().unwrap();
        st.linked = false;
        st.preview.clear();
        st.fps = 0.0;
        drop(st);
        (self.repaint)();
    }

    /// One Linked frame: collect key presses (for ripples), draw every LED at the current time,
    /// send the frame to each device and publish the preview and fps.
    fn render_frame(&mut self) {
        let t = self.t0.elapsed().as_secs_f32();
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();

        // Key presses -> ripples
        for &kind in &kinds {
            let o = &self.open[&key(kind)];
            match o.dev.transact(&proto::key_events_request()) {
                Ok(p) => {
                    for led in proto::parse_key_events(&p) {
                        if let Some(pt) = self.layout.led(kind, led) {
                            self.renderer.add_ripple(pt, t);
                        }
                    }
                }
                Err(e) => {
                    self.fail(kind, e);
                    return;
                }
            }
        }

        // Forget ripples that have faded, then draw each device's LEDs at their combined-layout spots.
        self.renderer.prune(t);
        let mut preview = Vec::new();
        for &kind in &kinds {
            let points = self.layout.points(kind).to_vec();
            let colors = self.renderer.render(&points, t, self.layout.width);
            preview.extend(points.iter().copied().zip(colors.iter().copied()));
            let msgs = proto::encode_frame(&colors);
            if let Err(e) = self.open[&key(kind)].dev.transact_many(&msgs) {
                self.fail(kind, e);
                return;
            }
        }

        self.frames += 1;
        let mut st = self.status.lock().unwrap();
        st.preview = preview;
        // Frames per second, averaged over about one second.
        if self.fps_timer.elapsed() >= Duration::from_secs(1) {
            st.fps = self.frames as f32 / self.fps_timer.elapsed().as_secs_f32();
            self.frames = 0;
            self.fps_timer = Instant::now();
        }
        drop(st);
        (self.repaint)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycodes::{Board, KC_F1};
    use std::time::Duration;

    /// Polls the engine's status every 20 ms until `f` is true; panics after 10 s with `what`.
    fn wait_for(h: &EngineHandle, what: &str, f: impl Fn(&Status) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if f(&h.status.lock().unwrap()) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    /// The keyboard's status, if it is connected and ready.
    fn keyboard(s: &Status) -> Option<&DeviceStatus> {
        match &s.keyboard {
            Slot::Ready(d) => Some(d),
            _ => None,
        }
    }

    /// The Top row setting through the real engine, on the pretend keyboard: back and forth
    /// many times, and once with the top row scrambled first.
    #[test]
    fn top_row_setting_through_the_engine() {
        let mut h = start(|| {}, true);
        wait_for(&h, "the keyboard", |s| keyboard(s).is_some());
        for i in 0..50 {
            let on = i % 2 == 0;
            h.send(Command::SetFnLock(on));
            wait_for(&h, "Fn lock", |s| keyboard(s).and_then(|d| d.info.layer) == Some(on as u8));
        }
        // Swap F1's two jobs on the keyboard, as a mixed-up keymap would have them
        let (r, c) = Board::Keyboard.matrix()[1];
        let mute = Board::Keyboard.default_code(0, 1);
        h.send(Command::SetKeys {
            kind: DeviceKind::Keyboard,
            writes: vec![(0, r, c, KC_F1), (1, r, c, mute), (2, r, c, mute), (3, r, c, KC_F1)],
        });
        wait_for(&h, "the scramble", |s| {
            keyboard(s).and_then(|d| d.keymap.as_ref()).is_some_and(|k| k.layers[0][1] == KC_F1)
        });
        h.send(Command::SetFnLock(false));
        wait_for(&h, "the fix", |s| {
            keyboard(s).is_some_and(|d| {
                d.info.layer == Some(0)
                    && d.keymap.as_ref().is_some_and(|k| k.layers[0][1] == mute && k.layers[1][1] == KC_F1)
            })
        });
        assert!(h.status.lock().unwrap().message.as_deref().unwrap_or("").contains("put back"));
        h.shutdown();
    }
}
