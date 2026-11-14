//! The effects (`wr_core::effects`: plug ripples, the unplug drain, sliding, the text line) as the
//! drawing threads' overlay. One `Effects` for both panels, behind a lock, with the clock filled in.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wr_core::effects::{Effects, EffectsSettings, Side};
use wr_core::Grid;

use crate::panel::Overlay;

pub struct EffectsOverlay {
    effects: Mutex<Effects>,
    start: Instant,
}

fn side(index: usize) -> Side {
    if index == 0 {
        Side::Left
    } else {
        Side::Right
    }
}

impl EffectsOverlay {
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
    fn apply(&self, side_index: usize, grid: &Grid, new_frame: bool) -> Grid {
        let now = self.now();
        self.effects().apply(side(side_index), grid, new_frame, now)
    }

    fn interval(&self, side_index: usize) -> Option<Duration> {
        self.effects().interval(side(side_index)).map(Duration::from_secs_f64)
    }

    fn forget(&self, side_index: usize) {
        self.effects().forget(side(side_index));
    }

    fn panel_present(&self, side_index: usize, present: bool) {
        self.effects().panel_present(side(side_index), present);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
