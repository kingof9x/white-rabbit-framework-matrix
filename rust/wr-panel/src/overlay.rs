//! The effects (`wr_core::effects`: plug ripples, the unplug drain, sliding, the text line) as the
//! drawing threads' overlay. One `Effects` for both panels, behind a lock, with the clock filled in.
//!
//! The drawing code in `wr_core::effects` is plain code that is given the time as a number. This file
//! wraps it in a `Mutex` and supplies that time from one `Instant`, so it can be shared by three kinds
//! of thread: the two drawing threads (through the `Overlay` trait in `panel.rs`: `apply`, `interval`,
//! `forget`, `panel_present`) and the monitor's main loop (`plug`, `start_slide`, and `effects()` for
//! the rest). The lock is held only for one call into `Effects`, never while a port is locked by this
//! code, and it is the last lock in the order given in `lib.rs`, so it cannot take part in a deadlock.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wr_core::effects::{Effects, EffectsSettings, Side};
use wr_core::Grid;

use crate::panel::Overlay;

/// The shared effects state and the moment the clock started. Created once by the monitor and handed
/// to `Panels::set_overlay` as an `Arc`.
pub struct EffectsOverlay {
    /// Both panels' effects. Locked for each call; the drawing threads call it about 30 times a second
    /// while an effect plays.
    effects: Mutex<Effects>,
    /// Time zero of the effects' clock (`now`).
    start: Instant,
}

/// Side index used by the drawing threads (0 = left, anything else = right) to the effects' `Side`.
fn side(index: usize) -> Side {
    if index == 0 {
        Side::Left
    } else {
        Side::Right
    }
}

impl EffectsOverlay {
    /// A new overlay with the user's effect settings, clock starting now.
    pub fn new(settings: EffectsSettings) -> Arc<EffectsOverlay> {
        let mut effects = Effects::new();
        effects.configure(settings);
        Arc::new(EffectsOverlay { effects: Mutex::new(effects), start: Instant::now() })
    }

    /// Seconds on the effects' clock (like Python's time.monotonic()).
    pub fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// For the main loop: `plug`, `say`, `start_slide`, `configure`, and setting `foreground`.
    pub fn effects(&self) -> MutexGuard<'_, Effects> {
        // A poisoned lock (a panic while drawing an effect) is used anyway: losing the effects for good
        // would be worse than one odd frame. Release builds abort on panic, so this only matters in tests.
        self.effects.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Something was plugged in or out at `position` ("left-back", ... or None if unknown).
    pub fn plug(&self, position: Option<&str>, plugged_in: bool) {
        let now = self.now();
        self.effects().plug(position, plugged_in, now);
    }

    /// The panel on `side` (0 left, 1 right) changes app: slide the next one in.
    pub fn start_slide(&self, side_index: usize) {
        let now = self.now();
        self.effects().start_slide(side(side_index), now);
    }
}

impl Overlay for EffectsOverlay {
    /// The frame with this side's effects drawn over it at the current time (see `Overlay::apply`).
    fn apply(&self, side_index: usize, grid: &Grid, new_frame: bool) -> Grid {
        let now = self.now();
        self.effects().apply(side(side_index), grid, new_frame, now)
    }

    /// How soon the drawing thread should redraw for a playing effect (seconds turned into a `Duration`).
    fn interval(&self, side_index: usize) -> Option<Duration> {
        self.effects().interval(side(side_index)).map(Duration::from_secs_f64)
    }

    /// Drops this side's running effects (panel asleep or swapped).
    fn forget(&self, side_index: usize) {
        self.effects().forget(side(side_index));
    }

    /// Records whether this side has a panel. When a panel appears on a side that had none, the app
    /// transition is started so its first picture comes in like an app change instead of popping up.
    fn panel_present(&self, side_index: usize, present: bool) {
        let mut e = self.effects();
        // Read before updating, under the same lock, so "just came on" is decided without a gap.
        let was = e.present(side(side_index));
        e.panel_present(side(side_index), present);
        if present && !was {
            // Just came on: the first picture comes in with the app transition
            e.start_intro(side(side_index));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves that a panel appearing plays the intro transition (the picture is not all there at first and
    /// redraws are asked for), and that reporting an already present panel again does not restart it.
    #[test]
    fn a_panel_coming_on_plays_the_transition_once() {
        let overlay = EffectsOverlay::new(EffectsSettings::default());
        overlay.panel_present(0, true);
        let mut grid = Grid::new();
        grid.set(4, 30, 200);
        // Sliding (the default) brings the picture up from the bottom: not all there at first
        assert_ne!(overlay.apply(0, &grid, true), grid);
        assert!(overlay.interval(0).is_some(), "keeps redrawing while it plays");
        // Already on (swapping the panels round): nothing new
        std::thread::sleep(Duration::from_millis(400));
        overlay.apply(0, &grid, false);
        overlay.panel_present(0, true);
        assert_eq!(overlay.apply(0, &grid, true), grid);
    }

    /// Proves that plugging something in asks the drawing thread to keep redrawing while the ripple plays.
    #[test]
    fn a_plug_ripple_keeps_the_panel_redrawing() {
        let overlay = EffectsOverlay::new(EffectsSettings::default());
        overlay.panel_present(0, true);
        overlay.panel_present(1, true);
        assert_eq!(overlay.interval(0), None);
        overlay.plug(Some("left-back"), true);
        assert!(overlay.interval(0).is_some());
        let mut g = Grid::new();
        g.set(4, 10, 100);
        let _ = overlay.apply(0, &g, true);
    }
}
