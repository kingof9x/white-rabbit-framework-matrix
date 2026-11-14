//! The two panels and their drawing threads: a port of `DrawingThread`, `swap_panels` and `align_panels`
//! in `led_mon/drawing.py`.
//!
//! A drawing thread takes frames from its queue and sends them to its panel. It finds the panel again
//! when the modules are moved to other slots (or plugged in later), skips a frame that is already on the
//! panel unless it was sent more than 5 seconds ago (the panel firmware sleeps after a minute without
//! commands), and two threads never use the same panel. Every frame goes through the effects (ripples,
//! sliding, the text line) just before it is sent, and while an effect plays the thread redraws the last
//! frame on its own about 30 times a second so the effect moves smoothly.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::dimming;
use crate::hub::{Hub, PortSystem, SharedPort};
use crate::ports::{discover, find_panel_port, Pins};
use crate::protocol;
use wr_core::queue::{FrameQueue, Item};
use wr_core::{info, warn, Grid, HEIGHT, WIDTH};

/// An unchanged frame is sent again after this long, so the panel never falls asleep.
pub const RESEND_UNCHANGED: Duration = Duration::from_secs(5);

/// Effects drawn over the frames just before they go to a panel (side 0 = left, 1 = right).
pub trait Overlay: Send + Sync {
    /// The frame with the effects on it. `new_frame` is false when redrawing the last frame for an effect.
    fn apply(&self, side: usize, grid: &Grid, new_frame: bool) -> Grid;
    /// How soon to redraw while an effect plays on this side, None when none does.
    fn interval(&self, side: usize) -> Option<Duration>;
    /// Stop this side's effects (the panel went to sleep, or swapped).
    fn forget(&self, side: usize);
    /// A panel was found, or lost, on this side.
    fn panel_present(&self, side: usize, present: bool);
}

/// Settings the drawing threads read on every frame.
#[derive(Debug, Default)]
pub struct Options {
    /// "Keep every shade when the panels are dim" (dimming by LED current).
    pub full_shades: AtomicBool,
    /// The brightest value the apps draw with now (follows the screen brightness).
    pub foreground: AtomicI32,
}

#[derive(Default)]
struct Conn {
    port: Option<Arc<SharedPort>>,
    /// The panel's USB location (or port name) last seen, looked at first when reconnecting.
    location: Option<String>,
}

/// Both panels: which port each side draws on, shared by the two drawing threads, the main loop and the
/// window's commands.
pub struct Panels {
    hub: Hub,
    pins: Arc<Mutex<Pins>>,
    sides: Mutex<[Conn; 2]>,
    pub options: Options,
    overlay: Mutex<Option<Arc<dyn Overlay>>>,
}

fn side_name(side: usize) -> &'static str {
    if side == 0 {
        "left"
    } else {
        "right"
    }
}

impl Panels {
    pub fn new(system: Arc<dyn PortSystem>, pins: Arc<Mutex<Pins>>) -> Arc<Panels> {
        Arc::new(Panels {
            hub: Hub::new(system),
            pins,
            sides: Mutex::default(),
            options: Options::default(),
            overlay: Mutex::new(None),
        })
    }

    /// The shared ports, for the window's serial/hold/release commands.
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    pub fn set_overlay(&self, overlay: Arc<dyn Overlay>) {
        *self.overlay.lock().unwrap() = Some(overlay);
    }

    fn overlay(&self) -> Option<Arc<dyn Overlay>> {
        self.overlay.lock().unwrap().clone()
    }

    /// The port each side draws on now, None while its panel is missing.
    pub fn port_names(&self) -> [Option<String>; 2] {
        let sides = self.sides.lock().unwrap();
        [0, 1].map(|s| sides[s].port.as_ref().map(|p| p.name().to_string()))
    }

    fn port(&self, side: usize) -> Option<Arc<SharedPort>> {
        self.sides.lock().unwrap()[side].port.clone()
    }

    /// Finds, opens and claims this side's panel; tells it to scroll or not.
    fn connect(&self, side: usize, animate: bool) -> Result<String, String> {
        let list = self.hub.list();
        let pins = self.pins.lock().unwrap().clone();
        let mut sides = self.sides.lock().unwrap();
        let other = sides[1 - side].port.as_ref().map(|p| p.name().to_lowercase());
        let taken = |p: &str| other.as_deref() == Some(p.to_lowercase().as_str());
        let name = find_panel_port(&list, &pins, sides[side].location.as_deref(), side, &taken)
            .ok_or_else(|| "no free LED panel found".to_string())?;
        let port = self.hub.open(&name).map_err(|e| e.to_string())?;
        port.lock().write(&protocol::animate(animate)).map_err(|e| e.to_string())?;
        let location = list
            .iter()
            .find(|p| p.device.eq_ignore_ascii_case(&name))
            .and_then(|p| p.location.clone())
            .or_else(|| Some(name.clone()));
        info!(
            "Connected the {} panel on {name} (USB location {})",
            side_name(side),
            location.as_deref().unwrap_or("?")
        );
        sides[side] = Conn { port: Some(port), location };
        drop(sides);
        if let Some(o) = self.overlay() {
            o.panel_present(side, true);
        }
        Ok(name)
    }

    /// Lets go of this side's port (it stays open while the window or the other side uses it).
    fn disconnect(&self, side: usize) {
        let had = self.sides.lock().unwrap()[side].port.take().is_some();
        if had {
            if let Some(o) = self.overlay() {
                o.panel_present(side, false);
            }
        }
    }

    /// Swap left and right, straight away: each drawing thread now draws on the other's panel.
    pub fn swap(&self) {
        let names = {
            let mut sides = self.sides.lock().unwrap();
            sides.swap(0, 1);
            [0, 1].map(|s| sides[s].port.as_ref().map(|p| p.name().to_string()))
        };
        if let Some(o) = self.overlay() {
            for (side, name) in names.iter().enumerate() {
                o.forget(side);
                o.panel_present(side, name.is_some());
            }
        }
        info!(
            "Swapped the panels: left is now {}, right is now {}",
            names[0].as_deref().unwrap_or("none"),
            names[1].as_deref().unwrap_or("none")
        );
    }

    /// After the left/right settings changed: swap the panels when each is drawing on the panel the
    /// settings now give the other side. True if they were swapped.
    pub fn align(&self) -> bool {
        let [Some(left), Some(right)] = self.port_names() else { return false };
        let found = discover(&self.hub.list(), &self.pins.lock().unwrap());
        if found.len() < 2 {
            return false;
        }
        let (want_left, want_right) = (&found[0].1, &found[1].1);
        if left.eq_ignore_ascii_case(want_right) && right.eq_ignore_ascii_case(want_left) {
            self.swap();
            return true;
        }
        false
    }
}

/// One panel's drawing thread.
pub struct Panel {
    side: usize,
    panels: Arc<Panels>,
    animate: bool,
    last_grid: Option<Grid>,
    backoff: Duration,
    next_try: Instant,
    warned_missing: bool,
}

impl Panel {
    pub fn new(side: usize, panels: Arc<Panels>) -> Panel {
        Panel {
            side,
            panels,
            animate: false,
            last_grid: None,
            backoff: Duration::from_millis(500),
            next_try: Instant::now(),
            warned_missing: false,
        }
    }

    /// Finds and opens this side's panel; false (and try again later) when there is none.
    pub fn connect(&mut self, force: bool) -> bool {
        let now = Instant::now();
        if !force && now < self.next_try {
            return false;
        }
        match self.panels.connect(self.side, self.animate) {
            Ok(_) => {
                self.backoff = Duration::from_millis(500);
                self.warned_missing = false;
                true
            }
            Err(e) => {
                self.panels.disconnect(self.side);
                self.next_try = now + self.backoff;
                self.backoff = (self.backoff * 2).min(Duration::from_secs(8));
                if !self.warned_missing {
                    warn!("No {} panel yet: {e}", side_name(self.side));
                    self.warned_missing = true;
                }
                false
            }
        }
    }

    fn send(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self.panels.port(self.side) {
            Some(p) => p.lock().write(bytes),
            None => Ok(()),
        }
    }

    fn draw(&self, grid: &Grid) -> std::io::Result<()> {
        let Some(port) = self.panels.port(self.side) else { return Ok(()) };
        // The whole frame under the port's lock, so the window's commands can't land in the middle
        let mut state = port.lock();
        let options = &self.panels.options;
        let grid = dimming::prepare(
            &mut state,
            grid,
            options.full_shades.load(Ordering::Relaxed),
            options.foreground.load(Ordering::Relaxed),
        )?;
        let key = grid.to_bytes();
        if let Some((shown, at)) = &state.shown {
            if *shown == key && at.elapsed() < RESEND_UNCHANGED {
                // Already on the panel: sending it again would only keep the panel busy
                return Ok(());
            }
        }
        let mut bytes = Vec::with_capacity(WIDTH * (HEIGHT + 4) + 3);
        for command in protocol::frame_commands(&grid) {
            bytes.extend_from_slice(&command);
        }
        state.write(&bytes)?;
        if !state.is_held() {
            state.shown = Some((key, Instant::now()));
        }
        Ok(())
    }

    /// Handles one item from the queue. False when the thread should stop.
    pub fn handle(&mut self, item: Item) -> bool {
        let result = match item {
            Item::Stop => return false,
            Item::Sleep => {
                if let Some(o) = self.panels.overlay() {
                    o.forget(self.side);
                }
                // The firmware fades the panel out and turns its LEDs off until the next command
                self.send(&protocol::sleep())
            }
            Item::Frame(grid, animate) => {
                if let Some(a) = animate {
                    self.animate = a;
                }
                if self.panels.port(self.side).is_none() && !self.connect(false) {
                    return true;
                }
                let mut r = Ok(());
                if !self.animate {
                    let shown = match self.panels.overlay() {
                        Some(o) => o.apply(self.side, &grid, true),
                        None => grid.clone(),
                    };
                    self.last_grid = Some(grid);
                    r = self.draw(&shown);
                }
                if let (Ok(()), Some(a)) = (&r, animate) {
                    r = self.send(&protocol::animate(a));
                }
                r
            }
        };
        if let Err(e) = result {
            self.lost(e);
        }
        true
    }

    /// An effect is playing and no new frame came: draw its next step over the last frame.
    fn redraw_effect(&mut self) {
        let (Some(grid), Some(overlay)) = (&self.last_grid, self.panels.overlay()) else { return };
        if self.animate || self.panels.port(self.side).is_none() {
            return;
        }
        let shown = overlay.apply(self.side, grid, false);
        if let Err(e) = self.draw(&shown) {
            self.lost(e);
        }
    }

    fn lost(&mut self, e: std::io::Error) {
        warn!("Lost the {} panel ({e}); looking for it again", side_name(self.side));
        self.panels.disconnect(self.side);
        self.connect(true);
    }

    pub fn run(mut self, queue: Arc<FrameQueue>) {
        self.connect(true);
        loop {
            let wait = self.panels.overlay().and_then(|o| o.interval(self.side));
            match queue.get(wait.unwrap_or(Duration::from_secs(1))) {
                Some(item) => {
                    if !self.handle(item) {
                        break;
                    }
                }
                None if wait.is_some() => self.redraw_effect(),
                None => {}
            }
        }
        self.panels.disconnect(self.side);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::fake::*;

    fn setup() -> (Arc<Fake>, Arc<Panels>, Panel, Panel) {
        let fake = Arc::new(Fake::default());
        *fake.ports.lock().unwrap() = vec![panel_port("COM5", "1-3.3:x.0"), panel_port("COM4", "1-3.2:x.0")];
        let panels = Panels::new(Arc::new(FakeSystem(fake.clone())), Arc::default());
        let left = Panel::new(0, panels.clone());
        let right = Panel::new(1, panels.clone());
        (fake, panels, left, right)
    }

    fn writes(fake: &Fake) -> Vec<(String, Vec<u8>)> {
        fake.written.lock().unwrap().clone()
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn left_and_right_by_usb_position() {
        let (_, panels, mut left, mut right) = setup();
        assert!(left.connect(true) && right.connect(true));
        assert_eq!(panels.port_names(), [some("COM4"), some("COM5")]);
    }

    #[test]
    fn unchanged_frames_are_skipped_until_the_keepalive() {
        let (fake, panels, mut left, _) = setup();
        let mut g = Grid::new();
        g.set(2, 2, 100);
        left.handle(Item::Frame(g.clone(), Some(false)));
        let w: Vec<_> = writes(&fake).into_iter().map(|(_, b)| b).collect();
        // animate off when connecting, the frame (one column + flush), animate off
        assert_eq!(w.len(), 3);
        assert_eq!(
            w[1],
            [
                protocol::command(protocol::STAGE_COL, &{
                    let mut p = vec![2u8];
                    p.extend((0..HEIGHT).map(|y| if y == 2 { 100 } else { 0 }));
                    p
                }),
                protocol::command(protocol::FLUSH_COLS, &[])
            ]
            .concat()
        );
        // Any other command forgets what the panel shows (as in Python), so the next frame goes out again
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(writes(&fake).len(), 4);
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(writes(&fake).len(), 4);
        let port = panels.port(0).unwrap();
        port.lock().shown.as_mut().unwrap().1 -= RESEND_UNCHANGED;
        left.handle(Item::Frame(g, None));
        assert_eq!(writes(&fake).len(), 5);
    }

    #[test]
    fn a_moved_panel_is_found_again() {
        let (fake, panels, mut left, mut right) = setup();
        left.connect(true);
        right.connect(true);
        // Both modules moved: new COM ports at new positions
        *fake.ports.lock().unwrap() = vec![panel_port("COM8", "1-4.2:x.0"), panel_port("COM9", "1-4.3:x.0")];
        panels.disconnect(0);
        panels.disconnect(1);
        assert!(right.connect(true) && left.connect(true));
        assert_eq!(panels.port_names(), [some("COM8"), some("COM9")]);
    }

    #[test]
    fn a_failed_write_finds_the_panel_again() {
        let (fake, panels, mut left, _) = setup();
        left.connect(true);
        // The keyboard was moved: COM4 is gone, the left panel is now COM7
        fake.fail_writes.lock().unwrap().push("COM4".into());
        *fake.ports.lock().unwrap() = vec![panel_port("COM5", "1-3.3:x.0"), panel_port("COM7", "1-4.2:x.0")];
        let mut g = Grid::new();
        g.set(0, 0, 50);
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(panels.port_names()[0], some("COM7"), "the panel now on the left side");
        left.handle(Item::Frame(g, None));
        assert_eq!(writes(&fake).last().unwrap().0, "COM7");
    }

    #[test]
    fn swap_and_align() {
        let (fake, panels, mut left, mut right) = setup();
        left.connect(true);
        right.connect(true);
        panels.swap();
        assert_eq!(panels.port_names(), [some("COM5"), some("COM4")]);
        let mut g = Grid::new();
        g.set(1, 1, 9);
        left.handle(Item::Frame(g, None));
        assert_eq!(writes(&fake).last().unwrap().0, "COM5");
        // Settings unchanged (left = COM4 by position): align puts them back
        assert!(panels.align());
        assert_eq!(panels.port_names(), [some("COM4"), some("COM5")]);
        assert!(!panels.align());
        // Pinned the other way round: align swaps
        *panels.pins.lock().unwrap() = Pins { left_port: some("COM5"), right_port: some("COM4"), ..Pins::default() };
        assert!(panels.align());
        assert_eq!(panels.port_names(), [some("COM5"), some("COM4")]);
    }

    #[test]
    fn held_panels_are_not_drawn_on() {
        let (fake, panels, mut left, _) = setup();
        left.connect(true);
        panels.hub().hold(&["COM4".into()], None);
        let before = writes(&fake).len();
        let mut g = Grid::new();
        g.set(0, 0, 1);
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(writes(&fake).len(), before);
        panels.hub().release(None);
        left.handle(Item::Frame(g, None));
        assert_eq!(writes(&fake).len(), before + 1, "drawn again once released");
    }

    /// Adds 1 to pixel (0, 0) on every frame; plays an effect for 3 redraws.
    #[derive(Default)]
    struct Count(Mutex<(u32, Vec<String>)>);
    impl Overlay for Count {
        fn apply(&self, _side: usize, grid: &Grid, new_frame: bool) -> Grid {
            let mut c = self.0.lock().unwrap();
            c.0 += 1;
            c.1.push(if new_frame { "new" } else { "step" }.into());
            let mut g = grid.clone();
            g.set(0, 0, c.0 as i32);
            g
        }
        fn interval(&self, _side: usize) -> Option<Duration> {
            (self.0.lock().unwrap().0 < 4).then_some(Duration::from_millis(1))
        }
        fn forget(&self, side: usize) {
            self.0.lock().unwrap().1.push(format!("forget {side}"));
        }
        fn panel_present(&self, side: usize, present: bool) {
            self.0.lock().unwrap().1.push(format!("present {side} {present}"));
        }
    }

    #[test]
    fn effects_go_over_the_frames_and_keep_moving() {
        let (fake, panels, left, _) = setup();
        let count = Arc::new(Count::default());
        panels.set_overlay(count.clone());
        let queue = Arc::new(FrameQueue::new());
        queue.put(Item::Frame(Grid::new(), None));
        let q = queue.clone();
        let thread = std::thread::spawn(move || left.run(q));
        let start = Instant::now();
        while count.0.lock().unwrap().0 < 4 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        queue.put(Item::Stop);
        thread.join().unwrap();
        let events = count.0.lock().unwrap().1.clone();
        assert_eq!(events[..5], ["present 0 true", "new", "step", "step", "step"]);
        // the frames sent carry the effect's pixel: 1, 2, 3, 4
        let firsts: Vec<u8> = writes(&fake)
            .iter()
            .filter(|(_, b)| b.len() > 4 && b[2] == protocol::STAGE_COL)
            .map(|(_, b)| b[4])
            .collect();
        assert_eq!(firsts, [1, 2, 3, 4]);
    }
}
