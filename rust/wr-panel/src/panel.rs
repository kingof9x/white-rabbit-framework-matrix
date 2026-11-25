//! The two panels and their drawing threads: a port of `DrawingThread`, `swap_panels` and `align_panels`
//! in `led_mon/drawing.py`.
//!
//! A drawing thread takes frames from its queue and sends them to its panel. It finds the panel again
//! when the modules are moved to other slots (or plugged in later), skips a frame that is already on the
//! panel unless it was sent more than 5 seconds ago (the panel firmware sleeps after a minute without
//! commands), and two threads never use the same panel. Every frame goes through the effects (ripples,
//! sliding, the text line) just before it is sent, and while an effect plays the thread redraws the last
//! frame on its own about 30 times a second so the effect moves smoothly.
//!
//! # Who owns what
//!
//! `Panels` is created once by the monitor and shared (`Arc`) by the two drawing threads (one `Panel`
//! each, side 0 = left, 1 = right, each running `Panel::run` on its own thread), the main loop (`swap`,
//! `align`, `options`, `port_names`) and the control channel thread (`hub()` for the window's commands).
//! The main loop never writes to a port itself: it puts frames on each side's `FrameQueue`.
//!
//! # Finding a panel again
//!
//! When a write fails (panel unplugged, keyboard moved, computer slept) the thread lets go of the port
//! (`lost`) and looks right away; if no panel is found it tries again on later frames, waiting 0.5 s,
//! then 1, 2, 4 and at most 8 s between tries. It looks first at the USB position (or port name) its
//! panel last had, so a panel that comes back at the same slot under a new COM number goes back to the
//! same side. See `Panels::connect` for how the two sides avoid taking each other's panel.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::dimming;
use crate::hub::{Hub, PortSystem, SharedPort};
use crate::ports::{discover, find_panel_port, Pins};
use crate::protocol;
use wr_core::queue::{FrameQueue, Item};
use wr_core::{info, warn, Grid};

/// An unchanged frame is sent again after this long, so the panel never falls asleep.
/// Also the reason a picture that never changes (a clock showing the same minute) still costs one
/// frame write every 5 s per panel.
pub const RESEND_UNCHANGED: Duration = Duration::from_secs(5);

/// Effects drawn over the frames just before they go to a panel (side 0 = left, 1 = right).
/// Implemented by `overlay::EffectsOverlay` in the app and by a counting fake in the tests. Shared by
/// both drawing threads, hence `Send + Sync`; implementations do their own locking.
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
/// Atomics rather than a lock, because each value stands alone and is read on every frame; `Relaxed`
/// ordering is enough since nothing else depends on seeing them in a particular order.
#[derive(Debug, Default)]
pub struct Options {
    /// "Keep every shade when the panels are dim" (dimming by LED current).
    pub full_shades: AtomicBool,
    /// The brightest value the apps draw with now (follows the screen brightness).
    pub foreground: AtomicI32,
}

/// What one side is drawing on now.
#[derive(Default)]
struct Conn {
    /// The open port this side draws on, or None while its panel is missing. Holding the `Arc` keeps the
    /// port open (see `hub.rs`).
    port: Option<Arc<SharedPort>>,
    /// The panel's USB location (or port name) last seen, looked at first when reconnecting.
    location: Option<String>,
}

/// Both panels: which port each side draws on, shared by the two drawing threads, the main loop and the
/// window's commands.
pub struct Panels {
    /// All open ports, shared with the window's commands.
    hub: Hub,
    /// The layout file's left/right pins. Shared with the monitor, which replaces them when the layout
    /// changes; only ever cloned here, never held while another lock is taken.
    pins: Arc<Mutex<Pins>>,
    /// Which port each side (0 left, 1 right) draws on. The first lock in the order in `lib.rs`.
    sides: Mutex<[Conn; 2]>,
    /// Read by the drawing threads on every frame; set by the main loop.
    pub options: Options,
    /// The effects, once the monitor has set them. Only cloned out under the lock, and used after it is
    /// released, so effect drawing never runs with this lock held.
    overlay: Mutex<Option<Arc<dyn Overlay>>>,
}

/// True if two USB locations are the same position, e.g. `1-3.3:x.0` and `1-3.3` (but not `1-3.30`).
/// Used to keep a missing panel's slot for its side: the interface suffix (after `:`) is ignored and
/// case doesn't matter.
fn same_position(a: &str, b: &str) -> bool {
    let base = |l: &str| l.split(':').next().unwrap_or(l).to_lowercase();
    base(a) == base(b)
}

/// "left" for 0, "right" for anything else; for log messages.
fn side_name(side: usize) -> &'static str {
    if side == 0 {
        "left"
    } else {
        "right"
    }
}

impl Panels {
    /// Both sides start with no panel; the drawing threads connect when they start (`Panel::run`).
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

    /// Sets the effects drawn over every frame (called once at start-up, before the threads draw).
    pub fn set_overlay(&self, overlay: Arc<dyn Overlay>) {
        *self.overlay.lock().unwrap() = Some(overlay);
    }

    /// A clone of the overlay, so the caller can use it without holding the `overlay` lock.
    fn overlay(&self) -> Option<Arc<dyn Overlay>> {
        self.overlay.lock().unwrap().clone()
    }

    /// The port each side draws on now, None while its panel is missing.
    pub fn port_names(&self) -> [Option<String>; 2] {
        let sides = self.sides.lock().unwrap();
        [0, 1].map(|s| sides[s].port.as_ref().map(|p| p.name().to_string()))
    }

    /// A clone of this side's port handle, taken under `sides` and used after `sides` is released, so a
    /// frame being written never blocks `swap`, `connect` or `port_names`.
    fn port(&self, side: usize) -> Option<Arc<SharedPort>> {
        self.sides.lock().unwrap()[side].port.clone()
    }

    /// Finds, opens and claims this side's panel; tells it to scroll or not.
    /// On success, returns the port name and tells the overlay this side has a panel. Errors are messages
    /// for the log ("no free LED panel found", or the open/write error). Holds `sides` from the choice
    /// until the port is recorded, so the two sides can never pick the same panel at once; inside that it
    /// takes `Hub::open` and the port's lock, which follows the lock order in `lib.rs`. The scroll command
    /// is the one write that proves the port works before the side takes it (unless the window holds it).
    fn connect(&self, side: usize, animate: bool) -> Result<String, String> {
        // The port list and a copy of the pins are fetched before `sides` is locked: listing ports asks
        // Windows (slow) and the pins lock must not be held together with `sides`.
        let list = self.hub.list();
        let pins = self.pins.lock().unwrap().clone();
        let mut sides = self.sides.lock().unwrap();
        let other = &sides[1 - side];
        let other_port = other.port.as_ref().map(|p| p.name().to_lowercase());
        // While the other side is looking for its panel, the panel back at the other side's own USB
        // position is kept for it. Otherwise, when the modules come back one at a time (moving the
        // keyboard), the first side to look grabs whichever panel showed up first, and the two swap.
        let reserved = other.port.is_none().then(|| other.location.clone()).flatten().filter(|l| !l.is_empty());
        // A port is taken if the other side draws on it now, or if it is (or sits at the USB position of)
        // the panel the other side is still waiting for.
        let taken = |p: &str| {
            other_port.as_deref() == Some(p.to_lowercase().as_str())
                || reserved.as_deref().is_some_and(|r| {
                    p.eq_ignore_ascii_case(r)
                        || list.iter().any(|d| {
                            d.device.eq_ignore_ascii_case(p)
                                && d.location.as_deref().is_some_and(|l| same_position(l, r))
                        })
                })
        };
        let name = find_panel_port(&list, &pins, sides[side].location.as_deref(), side, &taken)
            .ok_or_else(|| "no free LED panel found".to_string())?;
        let port = self.hub.open(&name).map_err(|e| e.to_string())?;
        port.lock().write(&protocol::animate(animate)).map_err(|e| e.to_string())?;
        // The USB position is remembered for finding this panel again; a port without one (not USB, or
        // Windows didn't say) remembers its port name instead, which `find_panel_port` also accepts.
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
        // `sides` is released before calling the overlay, whose lock comes later in the lock order and
        // whose effects code may take a while.
        drop(sides);
        if let Some(o) = self.overlay() {
            o.panel_present(side, true);
        }
        Ok(name)
    }

    /// Lets go of this side's port (it stays open while the window or the other side uses it).
    /// Tells the overlay the panel is gone only if the side had one, so it isn't told twice. The side's
    /// remembered USB position is kept, so the next `connect` looks there first.
    fn disconnect(&self, side: usize) {
        let had = self.sides.lock().unwrap()[side].port.take().is_some();
        if had {
            if let Some(o) = self.overlay() {
                o.panel_present(side, false);
            }
        }
    }

    /// Swap left and right, straight away: each drawing thread now draws on the other's panel.
    /// Called from the main loop (a hotkey or the tray). Running effects are dropped on both sides, since
    /// they were drawn for the other panel. The remembered USB positions move with the ports.
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
        // `discover` puts the left panel first, by the pins or by USB position. Only an exact cross-over
        // (each side on the other's panel) swaps; any other mismatch is left to the drawing threads.
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
/// Owned by its drawing thread (not shared); `run` consumes it.
pub struct Panel {
    /// 0 = left, 1 = right.
    side: usize,
    /// The shared state of both sides.
    panels: Arc<Panels>,
    /// True while the firmware's own scroll is on: frames are then not drawn, the panel scrolls by itself.
    animate: bool,
    /// The last frame from the queue, before effects, kept so an effect can be redrawn over it.
    last_grid: Option<Grid>,
    /// How long to wait after the next failed search (0.5 s, doubling up to 8 s; reset on success).
    backoff: Duration,
    /// No search before this moment unless forced.
    next_try: Instant,
    /// So "No left panel yet" is logged once per absence, not on every try.
    warned_missing: bool,
}

impl Panel {
    /// A drawing thread's state for one side, not yet connected; the first search may happen at once.
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
    /// `force` ignores the wait (`next_try`); used at start-up and right after losing the panel. A failed
    /// search also lets go of whatever port the side still had.
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

    /// Sends one command (sleep, scroll on/off) to this side's panel under the port's lock; does nothing
    /// without a panel. Dropped while the window holds the panel.
    fn send(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self.panels.port(self.side) {
            Some(p) => p.lock().write(bytes),
            None => Ok(()),
        }
    }

    /// Sends one frame (already with effects) to this side's panel, or nothing if there is none. An error
    /// means a write failed; the caller then calls `lost`.
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
        // The bytes after dimming are what the panel would show, so they are what is compared and kept.
        let key = grid.to_bytes();
        if let Some((shown, at)) = &state.shown {
            if *shown == key && at.elapsed() < RESEND_UNCHANGED {
                // Already on the panel: sending it again would only keep the panel busy
                return Ok(());
            }
        }
        // One write per command, as Python sends them: the firmware reads one command from each USB
        // transfer, so commands joined into one write would be cut up and lost
        for command in protocol::frame_commands(&grid) {
            state.write(&command)?;
        }
        // While held the writes above were dropped, so the frame must not be recorded as shown (or the
        // window would get a stale `last_frame` and the first frame after release would be skipped).
        if !state.is_held() {
            state.shown = Some((key, Instant::now()));
            state.last_frame = Some(key);
        }
        Ok(())
    }

    /// Handles one item from the queue. False when the thread should stop.
    /// `Item::Frame(grid, animate)`: `animate` is Some when the main loop switched the firmware scroll on or
    /// off for this frame (a scrolling app came or went), None otherwise. Write errors are not returned:
    /// they make the thread look for its panel again (`lost`).
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
                // No panel and too early to look again: the frame is dropped (the next one will be newer anyway).
                if self.panels.port(self.side).is_none() && !self.connect(false) {
                    return true;
                }
                let mut r = Ok(());
                // While the firmware scrolls by itself, frames are not sent: a frame would stop the scroll.
                if !self.animate {
                    let shown = match self.panels.overlay() {
                        Some(o) => o.apply(self.side, &grid, true),
                        None => grid.clone(),
                    };
                    self.last_grid = Some(grid);
                    r = self.draw(&shown);
                }
                // The scroll command goes after the frame, so the firmware scrolls the picture just drawn. Like
                // every write, it makes the next frame be sent even if it is unchanged.
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
    /// Called when the queue gave nothing within the effect's interval. Does nothing while scrolling, with
    /// no panel, or before the first frame.
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

    /// A write failed: log it, let go of the port (it is broken, see `hub.rs`) and look for the panel at
    /// once, ignoring the backoff.
    fn lost(&mut self, e: std::io::Error) {
        warn!("Lost the {} panel ({e}); looking for it again", side_name(self.side));
        self.panels.disconnect(self.side);
        self.connect(true);
    }

    /// The drawing thread's body, until `Item::Stop`. Waits on the queue for at most the effect interval
    /// while an effect plays (about 33 ms), or 1 s otherwise; the 1 s timeout only keeps the loop checking
    /// whether an effect started. Lets go of the port when it ends.
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
    use wr_core::HEIGHT;

    /// Two fake panels: COM4 at USB position 1-3.2 (so left) and COM5 at 1-3.3 (so right), listed in the
    /// wrong order on purpose; no pins.
    fn setup() -> (Arc<Fake>, Arc<Panels>, Panel, Panel) {
        let fake = Arc::new(Fake::default());
        *fake.ports.lock().unwrap() = vec![panel_port("COM5", "1-3.3:x.0"), panel_port("COM4", "1-3.2:x.0")];
        let panels = Panels::new(Arc::new(FakeSystem(fake.clone())), Arc::default());
        let left = Panel::new(0, panels.clone());
        let right = Panel::new(1, panels.clone());
        (fake, panels, left, right)
    }

    /// Every (port, bytes) written so far.
    fn writes(fake: &Fake) -> Vec<(String, Vec<u8>)> {
        fake.written.lock().unwrap().clone()
    }

    /// Shorthand for `Some(s.to_string())`.
    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    /// Proves that sides are chosen by USB position, not by COM number or list order.
    #[test]
    fn left_and_right_by_usb_position() {
        let (_, panels, mut left, mut right) = setup();
        assert!(left.connect(true) && right.connect(true));
        assert_eq!(panels.port_names(), [some("COM4"), some("COM5")]);
    }

    /// Proves the exact bytes of a frame, that an unchanged frame is skipped, that any other write makes
    /// the next frame go out again, and that an unchanged frame is resent after `RESEND_UNCHANGED`.
    #[test]
    fn unchanged_frames_are_skipped_until_the_keepalive() {
        let (fake, panels, mut left, _) = setup();
        let mut g = Grid::new();
        g.set(2, 2, 100);
        left.handle(Item::Frame(g.clone(), Some(false)));
        let w: Vec<_> = writes(&fake).into_iter().map(|(_, b)| b).collect();
        // animate off when connecting, the frame (one column, then flush: one write each, since the
        // firmware reads one command per USB transfer), animate off
        assert_eq!(w.len(), 4);
        assert_eq!(
            w[1],
            protocol::command(protocol::STAGE_COL, &{
                let mut p = vec![2u8];
                p.extend((0..HEIGHT).map(|y| if y == 2 { 100 } else { 0 }));
                p
            })
        );
        assert_eq!(w[2], protocol::command(protocol::FLUSH_COLS, &[]));
        // Any other command forgets what the panel shows (as in Python), so the next frame goes out again
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(writes(&fake).len(), 6);
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(writes(&fake).len(), 6);
        let port = panels.port(0).unwrap();
        port.lock().shown.as_mut().unwrap().1 -= RESEND_UNCHANGED;
        left.handle(Item::Frame(g, None));
        assert_eq!(writes(&fake).len(), 8);
    }

    /// Proves that when both panels move to new slots and COM ports, each side still gets the right one.
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

    /// Proves that a failed write makes the thread switch to the panel now on its side within the same
    /// frame, and that later frames go there.
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

    /// Proves the reservation in `Panels::connect`: a side does not take the panel the other side is
    /// waiting for, so panels plugged back one at a time don't swap sides.
    #[test]
    fn panels_coming_back_one_at_a_time_keep_their_sides() {
        // Callum's log: both panels unplugged (keyboard moved), then the right one (1-3.3) came back
        // first, and the left side took it; then the left one (1-4.2) came back and went to the right.
        let fake = Arc::new(Fake::default());
        *fake.ports.lock().unwrap() = vec![panel_port("COM4", "1-4.2:x.0"), panel_port("COM5", "1-3.3:x.0")];
        let panels = Panels::new(Arc::new(FakeSystem(fake.clone())), Arc::default());
        let (mut left, mut right) = (Panel::new(0, panels.clone()), Panel::new(1, panels.clone()));
        assert!(left.connect(true) && right.connect(true));
        assert_eq!(panels.port_names(), [some("COM4"), some("COM5")]);
        panels.disconnect(0);
        panels.disconnect(1);
        *fake.ports.lock().unwrap() = vec![panel_port("COM5", "1-3.3:x.0")];
        assert!(!left.connect(true), "the right panel is kept for the right side");
        assert!(right.connect(true));
        *fake.ports.lock().unwrap() = vec![panel_port("COM3", "1-4.2:x.0"), panel_port("COM5", "1-3.3:x.0")];
        assert!(left.connect(true));
        assert_eq!(panels.port_names(), [some("COM3"), some("COM5")]);
    }

    /// Proves that the reservation only covers the other side's own slot: a panel in a new slot is used.
    #[test]
    fn a_lone_panel_in_a_new_place_is_still_used() {
        // Both panels gone; one comes back at a position neither side had: the first side to look takes it
        let (fake, panels, mut left, mut right) = setup();
        left.connect(true);
        right.connect(true);
        panels.disconnect(0);
        panels.disconnect(1);
        *fake.ports.lock().unwrap() = vec![panel_port("COM6", "1-3.4:x.0")];
        assert!(right.connect(true));
        assert!(!left.connect(true));
        assert_eq!(panels.port_names(), [None, some("COM6")]);
    }

    /// Proves that `swap` sends frames to the other panel at once, and that `align` puts them back to
    /// match the positions or the pins (and does nothing when they already match).
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

    /// Proves that nothing reaches a held panel and that drawing resumes (column + flush) after release.
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
        assert_eq!(writes(&fake).len(), before + 2, "drawn again once released (column + flush)");
    }

    /// Proves that the monitor's last frame stays readable through the hub after the window holds the panel.
    #[test]
    fn the_last_frame_is_kept_for_the_window_while_it_holds_the_panel() {
        let (_, panels, mut left, _) = setup();
        left.connect(true);
        let mut g = Grid::new();
        g.set(2, 3, 90);
        left.handle(Item::Frame(g, None));
        panels.hub().hold(&["COM4".into()], None);
        let frame = panels.hub().last_frame("COM4").expect("kept");
        assert_eq!(frame[2][3], 90);
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

    /// Proves that the overlay is applied to every frame sent, that the thread redraws on its own while an
    /// effect plays ("step"), and that the panel appearing is reported first.
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
