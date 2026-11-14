//! The main loop: ten times a second, pick each panel's apps, draw them and hand the frames to the
//! panels' drawing threads. A port of `render_iteration` in `led_mon/led_system_monitor.py`, without
//! music mode, the window's previews and the control channel yet.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use wr_core::layout::{Layout, Rotation};
use wr_core::queue::{FrameQueue, Item};
use wr_core::render::{brightness_values, render_ids, render_panel, Readings};
use wr_core::settings::Settings;
use wr_core::Grid;
use wr_panel::ports::Pins;

use wr_core::{error, info, warn};

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
    pub settings: Mutex<Settings>,
}

impl Shared {
    pub fn identify(&self, seconds: f64) {
        *self.identify_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs_f64(seconds));
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

        if self.shared.identifying() {
            for (q, apps) in self.queues.iter().zip(&panels) {
                let mut grid = Grid::new();
                render_ids(&mut grid, apps, fg, bg);
                q.put(Item::Frame(grid, Some(false)));
            }
            self.latch = true;
            return Duration::from_millis(100);
        }

        for (side, (q, apps)) in self.queues.iter().zip(&panels).enumerate() {
            let mut grid = Grid::new();
            let animate = render_panel(&mut grid, apps, self.readings.as_mut(), fg, bg);
            let mut do_animate = None;
            if [changed.left, changed.right][side] {
                do_animate = Some(animate);
            }
            if self.latch && apps.animate() {
                do_animate = Some(true);
            }
            q.put(Item::Frame(grid, do_animate));
        }
        self.latch = false;
        Duration::from_millis(100)
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
    fn identify_shows_the_names() {
        let mut m = monitor();
        m.shared.identify(3.0);
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
    fn the_bundled_layout_reads() {
        let m = monitor();
        assert!(!m.rotation.layout.quadrants.is_empty());
    }
}
