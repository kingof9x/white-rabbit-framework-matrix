//! One panel in music mode (Python's `looks.MusicPanel`), and turning a picture into LED values.

use wr_core::Grid;

use crate::settings::Settings;
use crate::visualizer::{Frame, Stereo, Visualizer, HEIGHT, WIDTH};

/// Seconds of silence before the idle version starts.
pub const IDLE_AFTER_SEC: f64 = 4.0;
/// Seconds sound must last to end the idle version (so a single click doesn't).
pub const IDLE_RESUME_SEC: f64 = 0.4;

/// The look while sound is heard, and the look's calm idle version once nothing has been heard for
/// idle_after_sec, until sound comes back for IDLE_RESUME_SEC.
pub struct MusicPanel {
    pub visualizer: Visualizer,
    /// A second copy of the look draws the idle version, so the first keeps listening undisturbed
    /// and the music picks up where it left off.
    idle_visualizer: Visualizer,
    idle_after_sec: f64,
    quiet_for: f64,
    heard_for: f64,
    pub idle: bool,
}

impl MusicPanel {
    pub fn new(settings: &Settings, side_right: bool, idle_after_sec: f64) -> Self {
        Self::with_seed(settings, side_right, idle_after_sec, None)
    }

    /// seed: fixed random numbers, for tests.
    pub fn with_seed(settings: &Settings, side_right: bool, idle_after_sec: f64, seed: Option<u64>) -> Self {
        MusicPanel {
            visualizer: Visualizer::new(settings, side_right, seed),
            idle_visualizer: Visualizer::new(settings, side_right, seed),
            idle_after_sec: idle_after_sec.max(0.0),
            quiet_for: 0.0,
            heard_for: 0.0,
            idle: false,
        }
    }

    pub fn is_silent(&self) -> bool {
        self.visualizer.is_silent()
    }

    pub fn loudness(&self) -> i64 {
        self.visualizer.loudness
    }

    /// The next picture, switching to and from the idle version.
    pub fn frame(&mut self, samples: &[Stereo], dt: f64) -> Frame {
        let dt = dt.max(0.001);
        let grid = self.visualizer.frame(samples, dt);
        if self.visualizer.is_silent() {
            self.heard_for = 0.0;
            self.quiet_for += dt;
            if self.quiet_for >= self.idle_after_sec {
                self.idle = true;
            }
        } else {
            self.heard_for += dt;
            if !self.idle || self.heard_for >= IDLE_RESUME_SEC {
                self.idle = false;
                self.quiet_for = 0.0;
            }
        }
        if self.idle {
            self.idle_visualizer.idle_frame(dt)
        } else {
            grid
        }
    }
}

/// Brightness 0.0-1.0 to LED values, keeping faint LEDs just visible.
pub fn to_leds(grid: &Frame, foreground_value: i32) -> Grid {
    let scale = foreground_value.max(1) as f64;
    let mut out = Grid::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            let g = grid[x][y];
            let value = (g * scale).round_ties_even() as i64;
            let value = if g > 0.02 { value.max(1) } else { 0 };
            out.v[x][y] = value.clamp(0, 255) as i32;
        }
    }
    out
}
