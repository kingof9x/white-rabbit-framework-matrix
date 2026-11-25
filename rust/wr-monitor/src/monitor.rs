//! The main loop: ten times a second, pick each panel's apps, draw them and hand the frames to the
//! panels' drawing threads. A port of `render_iteration` in `led_mon/led_system_monitor.py`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use wr_core::apps::panel_names::{load_panel_names, name_grid};
use wr_core::layout::{Layout, Rotation};
use wr_core::queue::{FrameQueue, Item};
use wr_core::render::{brightness_values, render_ids, render_panel_on, Readings};
use wr_core::settings::Settings;
use wr_core::Grid;
use wr_panel::ports::Pins;

use crate::music::Music;
use wr_panel::overlay::EffectsOverlay;
use wr_panel::panel::Panels;

use wr_core::{error, info, warn};

/// How long Identify panels shows each panel's own name before the apps' names.
const NAME_SECONDS: f64 = 2.0;

/// What the tray, the shortcuts and the power events change, read by the main loop.
#[derive(Default)]
pub struct Shared {
    /// Stop animation (tray): the panels go dark.
    pub paused: AtomicBool,
    /// Freeze app switching.
    pub frozen: AtomicBool,
    /// Next app: every quarter moves on once.
    pub next_app: AtomicBool,
    /// The lid is closed, the screen is off or the PC is going to sleep (and the setting is on).
    pub power_sleep: AtomicBool,
    /// Read the layout file again.
    pub reload: AtomicBool,
    pub quit: AtomicBool,
    /// Show the apps' names until then.
    pub identify_until: Mutex<Option<Instant>>,
    /// Before the apps' names: each panel's own name (left, right) until then.
    pub panel_names: Mutex<Option<(Instant, [String; 2])>>,
    pub settings: Mutex<Settings>,
    /// The window gave panels back: send every panel its picture and scrolling again, since what the
    /// drawing threads think is on the panels is out of date.
    pub redraw: AtomicBool,
    /// Still pictures from the window's layout editor for each side, until then.
    pub preview: Mutex<Option<(Instant, [Option<Grid>; 2])>>,
    /// Music mode: every panel shows the audio visualizer.
    pub music_mode: AtomicBool,
    /// The Audio tab changed the look: start the visualizers again with it.
    pub music_restart: AtomicBool,
    /// What the visualizers hear ({side: {device, problem, loudness, silent, input}}).
    pub hearing: Mutex<serde_json::Value>,
    /// The last thing plugged in or out, for the Effects tab (`PlugEvent::to_json`).
    pub last_plug: Mutex<serde_json::Value>,
    /// Both panels (wr-panel), once the exe has found them: dimming, swapping and lining them up.
    pub panels: Mutex<Option<Arc<Panels>>>,
    /// The Effects tab's effects drawn over the panels.
    pub effects: Mutex<Option<Arc<EffectsOverlay>>>,
}

impl Shared {
    pub fn set_music_mode(&self, on: bool) {
        self.music_mode.store(on, Ordering::SeqCst);
        info!("Audio reactivity {}", if on { "on" } else { "off" });
    }

    /// Identify panels: each panel's name for 2 seconds, then the apps' names for `seconds`.
    pub fn identify(&self, seconds: f64) {
        self.identify_with_names(seconds, load_panel_names(&crate::paths::settings_file()));
    }

    pub fn identify_with_names(&self, seconds: f64, names: [String; 2]) {
        let names_until = Instant::now() + Duration::from_secs_f64(NAME_SECONDS);
        *self.panel_names.lock().unwrap() = Some((names_until, names));
        *self.identify_until.lock().unwrap() = Some(names_until + Duration::from_secs_f64(seconds));
    }

    /// The name to show on this panel (0 left, 1 right) right now while identifying, or None.
    fn panel_name(&self, side: usize) -> Option<String> {
        let names = self.panel_names.lock().unwrap();
        names.as_ref().filter(|(until, _)| Instant::now() < *until).map(|(_, n)| n[side].clone())
    }

    pub fn show_preview(&self, frames: [Option<Grid>; 2], seconds: Duration) {
        *self.preview.lock().unwrap() = Some((Instant::now() + seconds, frames));
    }

    pub fn clear_preview(&self) {
        *self.preview.lock().unwrap() = None;
    }

    /// The preview picture for a side (0 left, 1 right), while there is one.
    pub fn preview_frame(&self, side: usize) -> Option<Grid> {
        let preview = self.preview.lock().unwrap();
        preview.as_ref().filter(|(until, _)| Instant::now() < *until).and_then(|(_, frames)| frames[side].clone())
    }

    fn identifying(&self) -> bool {
        self.identify_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }
}

/// The layout file and when it was last changed, so an edit in the window is picked up.
pub struct LayoutSource {
    pub path: Option<PathBuf>,
    pub modified: Option<SystemTime>,
}

impl LayoutSource {
    pub fn locate() -> LayoutSource {
        let path = crate::paths::layout_file();
        let modified = path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        LayoutSource { path, modified }
    }

    pub fn load(&self) -> Layout {
        let text = self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        let from_file = text.as_deref().map(Layout::parse);
        match from_file {
            Some(Ok(layout)) => layout,
            Some(Err(e)) => {
                error!("The layout file {} can't be read ({e}); using the bundled layout", self.display());
                Layout::parse(crate::paths::BUNDLED_LAYOUT).expect("bundled layout")
            }
            None => Layout::parse(crate::paths::BUNDLED_LAYOUT).expect("bundled layout"),
        }
    }

    pub fn changed(&self) -> bool {
        let now = LayoutSource::locate();
        now.path != self.path || now.modified != self.modified
    }

    fn display(&self) -> String {
        self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(bundled)".into())
    }
}

pub fn pins_of(layout: &Layout) -> Pins {
    Pins {
        left_port: layout.left_port.clone(),
        right_port: layout.right_port.clone(),
        left_location: layout.left_location.clone(),
        right_location: layout.right_location.clone(),
    }
}

pub struct Monitor {
    pub shared: Arc<Shared>,
    pub queues: [Arc<FrameQueue>; 2],
    pub pins: Arc<Mutex<Pins>>,
    pub readings: Box<dyn Readings + Send>,
    /// The screen's brightness, 0.0-1.0.
    pub screen_brightness: Box<dyn Fn() -> f64 + Send>,
    pub source: LayoutSource,
    pub rotation: Rotation,
    started: Instant,
    blanked: bool,
    asleep: bool,
    /// Turn the panels' own scrolling back on after showing the names.
    latch: bool,
    /// Send every panel its scrolling on the next frame (see Shared::redraw).
    redraw: bool,
    music: Music,
    /// The names of the apps each panel shows, to slide only when they change.
    shown: [Option<Vec<String>>; 2],
    /// Music mode's look, read from settings.json when it starts.
    music_settings: Option<wr_music::Settings>,
    /// The side showed a preview on the last frame.
    previewed: [bool; 2],
    last_check: Instant,
    /// settings.json's size and change time when it was last read: it is read again only when they change.
    settings_stamp: Option<(u64, SystemTime)>,
    /// What each panel last showed from its apps, for the old-TV switch-off.
    last_grids: [Option<Grid>; 2],
    low_battery: LowBattery,
}

/// Says so on the text line once when the battery drops to 20% (again after charging): Python's
/// `LowBatteryNotice`.
#[derive(Default)]
struct LowBattery {
    said: bool,
    next_check: f64,
}

impl LowBattery {
    const LEVEL: f64 = 20.0;

    /// The text to say now, if any. `battery` is (charge 0.0-1.0, plugged in).
    fn check(&mut self, now: f64, battery: impl FnOnce() -> Option<(f64, bool)>) -> Option<String> {
        if now < self.next_check {
            return None;
        }
        self.next_check = now + 30.0;
        let (charge, plugged) = battery()?;
        let percent = charge * 100.0;
        if plugged || percent > Self::LEVEL + 2.0 {
            self.said = false;
        } else if percent <= Self::LEVEL && !self.said {
            self.said = true;
            return Some(format!("Battery {}%", percent as i64));
        }
        None
    }
}

/// Plays the old-TV frames on the panels together, one every `TV_FRAME_SECONDS`.
fn play_tv(queues: &[Arc<FrameQueue>; 2], frames: [Vec<Grid>; 2]) {
    let steps = frames.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..steps {
        for (q, f) in queues.iter().zip(&frames) {
            if let Some(g) = f.get(i) {
                q.put(Item::Frame(g.clone(), Some(false)));
            }
        }
        std::thread::sleep(Duration::from_secs_f64(wr_core::animations::TV_FRAME_SECONDS));
    }
}

impl Monitor {
    pub fn new(
        shared: Arc<Shared>,
        queues: [Arc<FrameQueue>; 2],
        pins: Arc<Mutex<Pins>>,
        readings: Box<dyn Readings + Send>,
        screen_brightness: Box<dyn Fn() -> f64 + Send>,
        source: LayoutSource,
    ) -> Monitor {
        let layout = source.load();
        *pins.lock().unwrap() = pins_of(&layout);
        wr_core::apps::picture::set_drawings_folder(crate::paths::config_dir().join("drawings"));
        Monitor {
            shared,
            queues,
            pins,
            readings,
            screen_brightness,
            source,
            rotation: Rotation::new(layout),
            started: Instant::now(),
            blanked: false,
            asleep: false,
            latch: false,
            redraw: false,
            music: Music::default(),
            shown: [None, None],
            music_settings: None,
            previewed: [false; 2],
            last_check: Instant::now(),
            settings_stamp: None,
            last_grids: [None, None],
            low_battery: LowBattery::default(),
        }
    }

    fn reload(&mut self) {
        self.source = LayoutSource::locate();
        let layout = self.source.load();
        self.reload_layout(layout);
    }

    fn reload_layout(&mut self, layout: Layout) {
        *self.pins.lock().unwrap() = pins_of(&layout);
        self.rotation.set_layout(layout);
        // Swap left and right (layout editor) changes which panel is which: follow it now
        if let Some(panels) = self.shared.panels.lock().unwrap().clone() {
            panels.align();
        }
        // `shown` stays: the next frame plays the app transition only if the layout now shows
        // different apps on a panel
        info!("Layout read again");
    }

    /// Once a second: a changed layout file or settings.json (the window saves both).
    fn check_files(&mut self) {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_check = Instant::now();
        if self.source.changed() {
            self.reload();
        }
        // Parse settings.json only when it changed (it used to be parsed every second regardless)
        let file = crate::paths::settings_file();
        let stamp = std::fs::metadata(&file).ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
        if stamp.is_none() || stamp != self.settings_stamp {
            self.settings_stamp = stamp;
            *self.shared.settings.lock().unwrap() = Settings::load(&file);
        }
    }

    /// One pass of the loop. Returns how long to wait before the next one.
    ///
    /// In order:
    /// 1. requests from the tray and the window: read the layout again, a changed layout file or
    ///    settings.json (checked once a second), panels given back by the window;
    /// 2. paused (tray) or the lid closed: play the old-TV switch-off once, send dark frames, and with
    ///    the lid send the panels to sleep; then wait 250 ms and do nothing else;
    /// 3. brightness from the screen (or the fixed level), the next apps from the rotation, the
    ///    visualizers started or stopped (`run_music`), the low-battery message;
    /// 4. identifying: the panel names, then the apps' names, instead of the apps;
    /// 5. otherwise each panel's picture: the window's preview if there is one, nothing if a visualizer
    ///    draws that panel, else its apps drawn from the readings. A panel that now shows other apps
    ///    gets the slide transition; scrolling on/off is sent only when it changes;
    /// 6. coming back from off: the old-TV switch-on into the new pictures; then the frames go to the
    ///    drawing threads' queues. Wait 100 ms.
    ///
    /// Nothing here waits for slow hardware: the quick readings (CPU, memory, battery) are taken here,
    /// the slow ones (sensors, GPU, weather, what's playing) come from background threads, and the
    /// panels are written by the drawing threads, so a slow sensor or a missing panel can't hold up
    /// the loop.
    /// (The old-TV effect is the exception: `play_tv` sleeps between its frames, under a second.)
    pub fn step(&mut self) -> Duration {
        if self.shared.reload.swap(false, Ordering::SeqCst) {
            self.reload();
        }
        self.check_files();
        if self.shared.redraw.swap(false, Ordering::SeqCst) {
            self.blanked = false;
            self.asleep = false;
            self.redraw = true;
        }

        let power_sleep = self.shared.power_sleep.load(Ordering::SeqCst);
        if self.shared.paused.load(Ordering::SeqCst) || power_sleep {
            if !self.blanked {
                // Stop the visualizers first so they don't light the panels again
                self.music.stop_all();
                if self.shared.settings.lock().unwrap().tv_effect {
                    // Old TV turning off: the picture squeezes into a line, then a dot that fades
                    let fg = self.music.foreground.load(Ordering::SeqCst);
                    let frames = [0, 1].map(|s| wr_core::animations::tv_off_frames(self.last_grids[s].as_ref(), fg));
                    play_tv(&self.queues, frames);
                }
                for q in &self.queues {
                    q.put(Item::Frame(Grid::new(), Some(false)));
                    q.put(Item::Frame(Grid::new(), None));
                }
                self.blanked = true;
            }
            if power_sleep && !self.asleep {
                for q in &self.queues {
                    q.put(Item::Sleep);
                }
                self.asleep = true;
            }
            self.music.stop_all();
            self.latch = true;
            return Duration::from_millis(250);
        }
        let settings = self.shared.settings.lock().unwrap().clone();
        // Coming back on after the panels were off: play the old-TV switch-on into the first pictures
        let tv_on = self.blanked && settings.tv_effect;
        self.blanked = false;
        self.asleep = false;
        let ratio = if settings.follow_screen_brightness {
            (self.screen_brightness)()
        } else {
            settings.brightness_level as f64 / 100.0
        };
        let (fg, bg) = brightness_values(ratio);

        let now = self.started.elapsed().as_secs_f64();
        self.rotation.frozen = self.shared.frozen.load(Ordering::SeqCst);
        let next = self.shared.next_app.load(Ordering::SeqCst);
        let changed = self.rotation.step(now, next);
        if next && !self.rotation.frozen {
            self.shared.next_app.store(false, Ordering::SeqCst);
        }
        let panels = [self.rotation.panel(false), self.rotation.panel(true)];
        self.music.foreground.store(fg, Ordering::SeqCst);
        let effects = self.shared.effects.lock().unwrap().clone();
        let full_shades = effects.as_ref().is_some_and(|fx| {
            let mut e = fx.effects();
            e.foreground = fg;
            e.settings.full_shades
        });
        if let Some(panels) = self.shared.panels.lock().unwrap().as_ref() {
            panels.options.foreground.store(fg, Ordering::SeqCst);
            panels.options.full_shades.store(full_shades, Ordering::SeqCst);
        }
        self.run_music(&panels);
        let now_sec = self.started.elapsed().as_secs_f64();
        let readings = &mut self.readings;
        if let Some(text) = self.low_battery.check(now_sec, || readings.battery()) {
            if let Some(fx) = effects.as_ref() {
                let mut e = fx.effects();
                if e.settings.text_battery {
                    e.say(&text);
                }
            }
        }

        if self.shared.identifying() {
            for (side, (q, apps)) in self.queues.iter().zip(&panels).enumerate() {
                let mut grid = Grid::new();
                match self.shared.panel_name(side) {
                    Some(name) => grid = name_grid(&name, fg),
                    None => render_ids(&mut grid, apps, fg, bg),
                }
                q.put(Item::Frame(grid, Some(false)));
            }
            self.latch = true;
            return Duration::from_millis(100);
        }

        let mut outputs: Vec<(usize, Grid, Option<bool>)> = Vec::new();
        for (side, (q, apps)) in self.queues.iter().zip(&panels).enumerate() {
            if let Some(grid) = self.shared.preview_frame(side) {
                // The window's preview stays still: the panel's own scrolling goes off
                q.put(Item::Frame(grid, Some(false)));
                self.previewed[side] = true;
                continue;
            }
            if self.music.running(side) {
                // The visualizer draws this panel by itself
                continue;
            }
            let mut grid = Grid::new();
            self.readings.set_panel(side == 1);
            let animate = render_panel_on(&mut grid, apps, self.readings.as_mut(), fg, bg, ["left", "right"][side]);
            let mut do_animate = None;
            if [changed.left, changed.right][side] {
                do_animate = Some(animate);
            }
            // The app transition (App transition effects page) only when the panel really shows
            // different apps than last time: not when the same app comes round again, nor after
            // the monitor starts or reads the layout again
            if app_changed(&mut self.shown[side], apps) && !animate {
                if let Some(fx) = self.shared.effects.lock().unwrap().as_ref() {
                    fx.start_slide(side);
                }
            }
            if self.latch && apps.animate() {
                do_animate = Some(true);
            }
            // Coming back from a preview or from the window holding the panel: say the scrolling
            // again, which also makes the drawing thread send the next frame even if unchanged
            if self.redraw || std::mem::take(&mut self.previewed[side]) {
                do_animate = Some(animate);
            }
            outputs.push((side, grid, do_animate));
        }
        if tv_on {
            let mut frames: [Vec<Grid>; 2] = [vec![], vec![]];
            for (side, grid, _) in &outputs {
                let mut f = wr_core::animations::tv_on_frames(Some(grid), fg);
                f.pop();
                frames[*side] = f;
            }
            play_tv(&self.queues, frames);
        }
        for (side, grid, do_animate) in outputs {
            self.last_grids[side] = Some(grid.clone());
            self.queues[side].put(Item::Frame(grid, do_animate));
        }
        self.latch = false;
        self.redraw = false;
        Duration::from_millis(100)
    }

    /// Music mode puts the audio visualizer on every panel; otherwise a panel showing the
    /// "equalizer" app gets one with that app's settings. Panels that no longer want one go back to
    /// their apps.
    fn run_music(&mut self, panels: &[wr_core::layout::PanelApps; 2]) {
        if self.shared.music_restart.swap(false, Ordering::SeqCst) {
            self.music_settings = None;
            self.music.stop_all();
        }
        let mut wanted: [Option<wr_music::Settings>; 2] = [None, None];
        if self.shared.music_mode.load(Ordering::SeqCst) {
            let settings = self.music_settings.get_or_insert_with(load_music_settings).clone();
            wanted = [Some(settings.clone()), Some(settings)];
        } else {
            self.music_settings = None;
            for (side, apps) in panels.iter().enumerate() {
                if let Some(app) = apps.apps().into_iter().find(|a| a.name == "equalizer") {
                    let args = serde_json::to_value(&app.args).unwrap_or_default();
                    wanted[side] = Some(wr_music::from_json(&args));
                }
            }
        }
        let held = self.shared.identifying() || (0..2).any(|s| self.shared.preview_frame(s).is_some());
        self.music.held.store(held, Ordering::SeqCst);
        for (side, want) in wanted.iter().enumerate() {
            let was_running = self.music.running(side);
            match want {
                Some(settings) => self.music.ensure(side, settings, &self.queues[side]),
                None if was_running => {
                    self.music.stop(side);
                    // Say the scrolling again and send the apps' picture even if it looks unchanged
                    self.redraw = true;
                }
                None => {}
            }
            // Music starting or stopping changes what the panel shows: play the app transition
            if was_running != self.music.running(side) {
                self.shown[side] = None;
                if let Some(fx) = self.shared.effects.lock().unwrap().as_ref() {
                    fx.start_slide(side);
                }
            }
        }
        *self.shared.hearing.lock().unwrap() = self.music.hearing();
    }

    pub fn run(mut self) {
        while !self.shared.quit.load(Ordering::SeqCst) {
            let wait = self.step();
            std::thread::sleep(wait);
        }
        for q in &self.queues {
            q.put(Item::Frame(Grid::new(), Some(false)));
            q.put(Item::Frame(Grid::new(), None));
            q.put(Item::Stop);
        }
    }
}

/// Whether a panel now shows different apps than on the last frame drawn (`shown` remembers them).
/// False for the first frame, when nothing was shown before.
fn app_changed(shown: &mut Option<Vec<String>>, apps: &wr_core::layout::PanelApps) -> bool {
    let key: Vec<String> = apps.apps().iter().map(|a| a.name.clone()).collect();
    match shown.replace(key.clone()) {
        Some(before) => before != key,
        None => false,
    }
}

/// Music mode's look and sound input from settings.json (Python's `app_settings.load_music`).
fn load_music_settings() -> wr_music::Settings {
    let text = std::fs::read_to_string(crate::paths::settings_file()).unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    wr_music::from_json(&json["music"])
}

/// Logs a panic in a thread before it ends, so a crash leaves a trace in the log.
pub fn log_panics() {
    std::panic::set_hook(Box::new(|info| {
        warn!("Crashed: {info}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wr_core::effects::Side;
    use wr_core::layout::App;

    struct Fixed;
    impl Readings for Fixed {
        fn cpu_cores(&mut self) -> Vec<f64> {
            vec![0.5; 8]
        }
        fn cpu_history(&mut self) -> Vec<Option<f64>> {
            vec![]
        }
        fn memory(&mut self) -> f64 {
            0.5
        }
        fn battery(&mut self) -> Option<(f64, bool)> {
            None
        }
        fn clock(&mut self, _: &App) -> (String, bool) {
            ("1234".into(), false)
        }
        fn now(&mut self) -> f64 {
            0.0
        }
    }

    fn monitor() -> Monitor {
        let source = LayoutSource { path: None, modified: None };
        let queues = [Arc::new(FrameQueue::new()), Arc::new(FrameQueue::new())];
        let mut m = Monitor::new(
            Arc::new(Shared::default()),
            queues,
            Arc::new(Mutex::new(Pins::default())),
            Box::new(Fixed),
            Box::new(|| 1.0),
            source,
        );
        m.last_check = Instant::now() + Duration::from_secs(3600); // don't read the real files
        m
    }

    fn frames(q: &FrameQueue) -> Vec<Item> {
        std::iter::from_fn(|| q.get(Duration::ZERO)).collect()
    }

    #[test]
    fn draws_a_frame_for_each_panel() {
        let mut m = monitor();
        m.step();
        for q in &m.queues {
            let items = frames(q);
            assert_eq!(items.len(), 1);
            assert!(matches!(items[0], Item::Frame(_, _)));
        }
    }

    #[test]
    fn pausing_blanks_once_and_the_lid_sends_the_panels_to_sleep() {
        let mut m = monitor();
        m.shared.power_sleep.store(true, Ordering::SeqCst);
        m.step();
        m.step();
        // The second dark frame replaces the first, which keeps its "scrolling off"
        assert_eq!(frames(&m.queues[0]), vec![Item::Frame(Grid::new(), Some(false)), Item::Sleep]);
    }

    #[test]
    fn identify_shows_the_panel_names_first() {
        let mut m = monitor();
        m.shared.identify_with_names(3.0, ["Desk".into(), "Right".into()]);
        m.step();
        let Item::Frame(grid, _) = &frames(&m.queues[0])[0] else { panic!() };
        assert_eq!(*grid, name_grid("Desk", 160));
        let Item::Frame(grid, _) = &frames(&m.queues[1])[0] else { panic!() };
        assert_eq!(*grid, name_grid("Right", 160));
    }

    #[test]
    fn identify_shows_the_names() {
        let mut m = monitor();
        m.shared.identify_with_names(3.0, ["Left".into(), "Right".into()]);
        // Past the panel names
        *m.shared.panel_names.lock().unwrap() = None;
        m.step();
        // The bundled layout's left panel starts with the clock on the whole panel: its name fills it
        let Item::Frame(grid, animate) = &frames(&m.queues[0])[0] else { panic!() };
        assert_eq!(*animate, Some(false));
        let mut expected = Grid::new();
        wr_core::drawing::draw_outline_border(&mut expected, 35);
        wr_core::drawing::draw_id(&mut expected, "time", 160);
        assert_eq!(*grid, expected);
    }

    #[test]
    fn a_preview_shows_then_the_apps_come_back() {
        let mut m = monitor();
        let mut picture = Grid::new();
        picture.set(4, 4, 200);
        m.shared.show_preview([Some(picture.clone()), None], Duration::from_secs(30));
        m.step();
        assert_eq!(frames(&m.queues[0]), vec![Item::Frame(picture, Some(false))]);
        assert!(matches!(frames(&m.queues[1])[..], [Item::Frame(_, _)]));
        m.shared.clear_preview();
        m.step();
        // The clock doesn't scroll: scrolling off is said again, so the panel gets the clock at once
        assert!(matches!(frames(&m.queues[0])[..], [Item::Frame(_, Some(false))]));
        m.step();
        assert!(matches!(frames(&m.queues[0])[..], [Item::Frame(_, None)]));
    }

    #[test]
    fn giving_panels_back_redraws_them() {
        let mut m = monitor();
        m.step();
        frames(&m.queues[0]);
        frames(&m.queues[1]);
        m.shared.redraw.store(true, Ordering::SeqCst);
        m.step();
        for q in &m.queues {
            assert!(matches!(frames(q)[..], [Item::Frame(_, Some(_))]));
        }
    }

    /// A monitor with the effects overlay (sliding on) and this layout; each panel has had a frame.
    fn with_layout(yaml: &str) -> (Monitor, Arc<wr_panel::overlay::EffectsOverlay>) {
        let mut m = monitor();
        let fx = wr_panel::overlay::EffectsOverlay::new(wr_core::effects::EffectsSettings::default());
        *m.shared.effects.lock().unwrap() = Some(fx.clone());
        m.rotation.set_layout(wr_core::layout::Layout::parse(yaml).unwrap());
        m.step();
        for side in [Side::Left, Side::Right] {
            // What the drawing thread does: the panel's picture goes through the effects
            fx.effects().apply(side, &Grid::new(), true, 0.0);
        }
        (m, fx)
    }

    fn sliding(fx: &wr_panel::overlay::EffectsOverlay) -> bool {
        let e = fx.effects();
        [Side::Left, Side::Right].iter().any(|&s| e.player(s).active())
    }

    const ONE_APP_EACH: &str = "duration: 10\nquadrants:\n  top-left:\n  - app:\n    name: cpu\n  bottom-left:\n  - app:\n    name: mem-bat\n  top-right:\n  - app:\n    name: disk\n  bottom-right:\n  - app:\n    name: net\n";

    #[test]
    fn one_app_per_panel_never_plays_the_app_transition() {
        let (mut m, fx) = with_layout(ONE_APP_EACH);
        // Its time comes round (next app), then the layout is read again: still the same apps
        m.shared.next_app.store(true, Ordering::SeqCst);
        m.step();
        assert!(!sliding(&fx), "same app came round again");
        // (What reading the layout file again does, without the real file)
        m.reload_layout(wr_core::layout::Layout::parse(ONE_APP_EACH).unwrap());
        m.step();
        m.shared.next_app.store(true, Ordering::SeqCst);
        m.step();
        assert!(!sliding(&fx), "after reading the layout again");
    }

    #[test]
    fn a_different_app_plays_the_app_transition() {
        let two = ONE_APP_EACH.replace("    name: cpu\n", "    name: cpu\n  - app:\n    name: time\n");
        let (mut m, fx) = with_layout(&two);
        assert!(!sliding(&fx), "not on the first frame");
        m.shared.next_app.store(true, Ordering::SeqCst);
        m.step();
        assert!(fx.effects().player(Side::Left).active(), "cpu -> time slides");
        assert!(!fx.effects().player(Side::Right).active(), "the right panel didn't change");
    }

    #[test]
    fn the_bundled_layout_reads() {
        let m = monitor();
        assert!(!m.rotation.layout.quadrants.is_empty());
    }

    #[test]
    fn low_battery_is_said_once_until_charged() {
        let mut b = LowBattery::default();
        assert_eq!(b.check(0.0, || Some((0.19, false))), Some("Battery 19%".into()));
        assert_eq!(b.check(31.0, || Some((0.18, false))), None);
        assert_eq!(b.check(62.0, || Some((0.30, true))), None);
        assert_eq!(b.check(93.0, || Some((0.20, false))), Some("Battery 20%".into()));
        // Not more often than every 30 seconds
        assert_eq!(b.check(100.0, || panic!("read too soon")), None);
    }

    #[test]
    fn stopping_plays_the_old_tv_switch_off() {
        let mut m = monitor();
        m.step();
        for q in &m.queues {
            frames(q);
        }
        m.shared.paused.store(true, Ordering::SeqCst);
        m.step();
        // The queues keep only the newest frames: the TV frames went by, then the dark frames
        assert!(m.last_grids.iter().all(Option::is_some));
        m.shared.settings.lock().unwrap().tv_effect = false;
        m.shared.paused.store(false, Ordering::SeqCst);
        m.step();
        m.shared.paused.store(true, Ordering::SeqCst);
        let started = Instant::now();
        m.step();
        assert!(started.elapsed() < Duration::from_millis(30), "no TV frames when it's off");
    }
}
