//! The main loop: ten times a second, pick each panel's apps, draw them and hand the frames to the
//! panels' drawing threads. A port of `render_iteration` in `led_mon/led_system_monitor.py`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use wr_core::apps::panel_names::{load_panel_names, name_grid};
use wr_core::layout::{Layout, Rotation};
use wr_core::queue::{FrameQueue, Item};
use wr_core::render::{brightness_values, render_ids, render_panel, Readings};
use wr_core::settings::Settings;
use wr_core::Grid;
use wr_panel::ports::Pins;

use crate::music::Music;

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
}

impl Shared {
    pub fn set_music_mode(&self, on: bool) {
        self.music_mode.store(on, Ordering::SeqCst);
        info!("Music mode {}", if on { "on" } else { "off" });
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
    /// Music mode's look, read from settings.json when it starts.
    music_settings: Option<wr_music::Settings>,
    /// The side showed a preview on the last frame.
    previewed: [bool; 2],
    last_check: Instant,
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
            music_settings: None,
            previewed: [false; 2],
            last_check: Instant::now(),
        }
    }

    fn reload(&mut self) {
        self.source = LayoutSource::locate();
        let layout = self.source.load();
        *self.pins.lock().unwrap() = pins_of(&layout);
        self.rotation.set_layout(layout);
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
        *self.shared.settings.lock().unwrap() = Settings::load(&crate::paths::settings_file());
    }

    /// One pass of the loop. Returns how long to wait before the next one.
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
        self.blanked = false;
        self.asleep = false;

        let settings = self.shared.settings.lock().unwrap().clone();
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
        self.run_music(&panels);

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
            let animate = render_panel(&mut grid, apps, self.readings.as_mut(), fg, bg);
            let mut do_animate = None;
            if [changed.left, changed.right][side] {
                do_animate = Some(animate);
            }
            if self.latch && apps.animate() {
                do_animate = Some(true);
            }
            // Coming back from a preview or from the window holding the panel: say the scrolling
            // again, which also makes the drawing thread send the next frame even if unchanged
            if self.redraw || std::mem::take(&mut self.previewed[side]) {
                do_animate = Some(animate);
            }
            q.put(Item::Frame(grid, do_animate));
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
            match want {
                Some(settings) => self.music.ensure(side, settings, &self.queues[side]),
                None if self.music.running(side) => {
                    self.music.stop(side);
                    // Say the scrolling again and send the apps' picture even if it looks unchanged
                    self.redraw = true;
                }
                None => {}
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

    #[test]
    fn the_bundled_layout_reads() {
        let m = monitor();
        assert!(!m.rotation.layout.quadrants.is_empty());
    }
}
