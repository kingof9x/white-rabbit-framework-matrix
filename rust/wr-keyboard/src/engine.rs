//! Background thread that owns the USB devices: applies settings, streams
//! Linked frames and keeps a status snapshot for the UI.

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

const FRAME_INTERVAL: Duration = Duration::from_millis(33); // ~30 fps
const IDLE_INTERVAL: Duration = Duration::from_millis(50);
const REFRESH_INTERVAL: Duration = Duration::from_millis(1000);
const SCAN_INTERVAL: Duration = Duration::from_millis(2500);
/// How often joined devices get their effect clocks lined up again.
const CLOCK_SYNC_INTERVAL: Duration = Duration::from_secs(10);

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
    Rescan,
    Shutdown,
}

#[derive(Clone, PartialEq, Debug)]
pub struct AudioSettings {
    pub look: AudioLook,
    /// 1..10, 5 = normal.
    pub sensitivity: u8,
    /// Listen to a microphone instead of what the PC plays.
    pub microphone: bool,
    /// Which microphone ("" = the default one).
    pub device: String,
    pub colors: AudioColors,
    /// How key presses show on top of the show.
    pub keys: KeyReact,
}

impl AudioSettings {
    fn listener(&self, demo: bool) -> Result<Listener, String> {
        Listener::start(demo, self.sensitivity, self.microphone, &self.device)
    }
}

struct AudioRun {
    settings: AudioSettings,
    listener: Listener,
    show: AudioShow,
    last: Instant,
}

#[derive(Clone)]
pub struct DeviceStatus {
    pub info: DeviceInfo,
    pub state: LightState,
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
    pub serial: Option<String>,
}

#[derive(Clone, Default)]
pub enum Slot {
    #[default]
    Missing,
    /// Found, but it is not running the f16custom firmware.
    Stock,
    Ready(DeviceStatus),
}

#[derive(Clone, Default)]
pub struct Status {
    pub keyboard: Slot,
    pub macropad: Slot,
    pub linked: bool,
    /// Linked mode preview: LED position + color.
    pub preview: Vec<(Point, Rgb)>,
    pub layout_size: (f32, f32),
    pub fps: f32,
    pub message: Option<String>,
    /// A Framework numpad is plugged in.
    pub numpad: bool,
    /// Audio mode is drawing.
    pub audio: bool,
    /// What audio mode listens to, or why it can't.
    pub audio_device: Option<String>,
    pub audio_problem: Option<String>,
    /// How loud the sound is right now (0..1), for the meter on the Audio page.
    pub audio_level: f32,
    /// Audio mode's last frame per device (LED order): [keyboard, macropad].
    pub audio_colors: [Vec<Rgb>; 2],
}

impl Status {
    pub fn slot(&self, kind: DeviceKind) -> &Slot {
        match kind {
            DeviceKind::Keyboard => &self.keyboard,
            DeviceKind::Macropad => &self.macropad,
        }
    }
    pub fn slot_mut(&mut self, kind: DeviceKind) -> &mut Slot {
        match kind {
            DeviceKind::Keyboard => &mut self.keyboard,
            DeviceKind::Macropad => &mut self.macropad,
        }
    }
}

struct Open {
    dev: Device,
    info: DeviceInfo,
    leds: Vec<LedPoint>,
    keymap: Option<Keymap>,
    combos: Option<proto::Combos>,
    led_cal: Option<proto::LedCal>,
    shortcuts: Option<bool>,
}

pub struct EngineHandle {
    pub tx: Sender<Command>,
    pub status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// Restores the built-in effects and stops the thread.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
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
fn demo_devices(skip: &[DeviceKind]) -> Vec<Found> {
    [(DeviceKind::Keyboard, SimPort::keyboard()), (DeviceKind::Macropad, SimPort::macropad())]
        .into_iter()
        .filter(|(kind, _)| !skip.contains(kind))
        .map(|(kind, port)| device::identify(Device::new(kind, Box::new(port))))
        .collect()
}

struct Engine {
    rx: Receiver<Command>,
    status: Arc<Mutex<Status>>,
    repaint: Box<dyn Fn() + Send>,
    api: Option<HidApi>,
    demo: bool,
    open: BTreeMap<u8, Open>, // key: 0 keyboard, 1 macropad
    stock: Vec<DeviceKind>,
    joined: bool,
    last_clock_sync: Instant,
    t0_clock: Instant,
    linked: Option<EffectParams>,
    audio: Option<AudioRun>,
    renderer: Renderer,
    side: MacropadSide,
    layout: CombinedLayout,
    t0: Instant,
    last_scan: Option<Instant>,
    last_refresh: Instant,
    frames: u32,
    fps_timer: Instant,
}

fn key(kind: DeviceKind) -> u8 {
    match kind {
        DeviceKind::Keyboard => 0,
        DeviceKind::Macropad => 1,
    }
}

impl Engine {
    fn new(rx: Receiver<Command>, status: Arc<Mutex<Status>>, repaint: Box<dyn Fn() + Send>, demo: bool) -> Self {
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

    fn set_message(&self, m: impl Into<String>) {
        self.status.lock().unwrap().message = Some(m.into());
        (self.repaint)();
    }

    fn run(mut self) {
        loop {
            let tick = Instant::now();
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

            let all_ready = self.open.len() == 2;
            if !all_ready && self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
                self.scan();
            }
            if self.audio.is_some() {
                self.render_audio();
            } else if self.linked.is_some() {
                self.render_frame();
            }
            if self.joined && self.last_clock_sync.elapsed() >= CLOCK_SYNC_INTERVAL {
                self.sync_clocks();
            }
            if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh();
            }

            let interval = if self.linked.is_some() || self.audio.is_some() { FRAME_INTERVAL } else { IDLE_INTERVAL };
            if let Some(rest) = interval.checked_sub(tick.elapsed()) {
                thread::sleep(rest);
            }
        }
    }

    fn handle(&mut self, c: Command) {
        match c {
            Command::SetState { kind, state, save } => {
                if let Some(o) = self.open.get(&key(kind)) {
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
                None => match settings.listener(self.demo) {
                    Ok(listener) => {
                        self.stop_linked();
                        let mut show = AudioShow::new(settings.look);
                        show.colors = settings.colors;
                        show.keys = settings.keys;
                        self.audio = Some(AudioRun { settings, listener, show, last: Instant::now() });
                        self.start_linked();
                        let mut st = self.status.lock().unwrap();
                        st.linked = false;
                        st.audio = true;
                        st.audio_problem = None;
                    }
                    Err(e) => self.audio_failed(e),
                },
            },
            Command::SetAudio(None) => self.stop_audio(),
            Command::SetLinked(Some(params)) => {
                self.stop_audio();
                self.renderer.params = params;
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
                self.stock.retain(|k| *k != kind);
                self.refresh();
            }
            Command::Shutdown => unreachable!(),
        }
    }

    /// Sends the same clock to both devices back to back so joined effects line up.
    fn sync_clocks(&mut self) {
        self.last_clock_sync = Instant::now();
        let ms = self.t0_clock.elapsed().as_millis() as u32;
        let msg = proto::set_clock(ms);
        for o in self.open.values() {
            let _ = o.dev.send(&msg);
        }
        for o in self.open.values() {
            let _ = o.dev.transact(&proto::state_request());
        }
    }

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
                    if self.linked.is_some() || self.audio.is_some() {
                        let _ = dev.transact(&proto::set_host_mode(true));
                    }
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
        self.stock = stock;
        if added {
            self.rebuild_layout();
        }
        self.refresh();
    }

    fn rebuild_layout(&mut self) {
        let kb = self.open.get(&0).map(|o| o.leds.as_slice()).unwrap_or(&[]);
        let pad = self.open.get(&1).map(|o| o.leds.as_slice()).unwrap_or(&[]);
        self.layout = CombinedLayout::new(kb, pad, self.side);
        self.status.lock().unwrap().layout_size = (self.layout.width, self.layout.height);
    }

    /// Something went wrong talking to a device: assume it was unplugged.
    fn fail(&mut self, kind: DeviceKind, e: device::DeviceError) {
        self.open.remove(&key(kind));
        self.rebuild_layout();
        self.status.lock().unwrap().message = Some(format!("{} disconnected: {e}", kind.name()));
        self.refresh();
    }

    fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        let mut updates = Vec::new();
        for kind in kinds {
            let o = &self.open[&key(kind)];
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

    fn audio_failed(&mut self, e: String) {
        let mut st = self.status.lock().unwrap();
        st.audio = false;
        st.audio_problem = Some(format!("Can't listen to the sound: {e}"));
        drop(st);
        (self.repaint)();
    }

    fn stop_audio(&mut self) {
        if self.audio.take().is_none() {
            return;
        }
        for o in self.open.values() {
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
        let dt = run.last.elapsed().as_secs_f32().clamp(0.001, 0.2);
        run.last = Instant::now();
        let bands = run.listener.bands(dt);
        run.show.update(&bands, dt);
        let level = bands.iter().sum::<f32>() / bands.len().max(1) as f32;
        let (device, problem) = (run.listener.device(), run.listener.problem());
        let kinds: Vec<DeviceKind> = self.open.values().map(|o| o.dev.kind).collect();
        let mut colors_out: [Vec<Rgb>; 2] = Default::default();
        for &kind in &kinds {
            let points = self.layout.points(kind).to_vec();
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

    fn start_linked(&mut self) {
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
