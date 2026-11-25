//! One panel in music mode (Python's `looks.MusicPanel`), and turning a picture into LED values.
//!
//! The monitor makes one `MusicPanel` per panel and, on that panel's `music-N` thread, calls `frame`
//! about 33 times a second with the latest samples from `capture` and the seconds since the last call,
//! then `to_leds` to get the 0..255 values the panel layer sends. A `MusicPanel` decides between the
//! look itself and its calm idle version: idle starts after `idle_after_sec` of silence and ends once
//! sound has lasted `IDLE_RESUME_SEC`.

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
    /// The look while sound is heard. It is fed every frame, even while idle, so its silence detection,
    /// automatic gain and history keep up and the bars resume smoothly.
    pub visualizer: Visualizer,
    /// A second copy of the look draws the idle version, so the first keeps listening undisturbed
    /// and the music picks up where it left off.
    idle_visualizer: Visualizer,
    /// Seconds of silence before the idle version starts (from the settings; never negative).
    idle_after_sec: f64,
    /// Seconds of silence so far (reset once sound ends the idle version or is heard while not idle).
    quiet_for: f64,
    /// Seconds of sound so far without a silent frame (reset by any silent frame).
    heard_for: f64,
    /// True while the idle version is shown.
    pub idle: bool,
}

impl MusicPanel {
    /// A panel for the given settings; `side_right` is true for the right panel (some looks mirror or
    /// pick a channel by side). Random numbers are seeded randomly (`with_seed` fixes them for tests).
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

    /// True when the look heard nothing on its last frame (see `Visualizer::is_silent`).
    pub fn is_silent(&self) -> bool {
        self.visualizer.is_silent()
    }

    /// The look's latest loudness figure (see `Visualizer::loudness`), reported by the
    /// monitor (with `is_silent`) so the window can show what is heard.
    pub fn loudness(&self) -> i64 {
        self.visualizer.loudness
    }

    /// The next picture, switching to and from the idle version.
    ///
    /// `samples` are stereo samples in -1..1, newest last; `dt` is the seconds since the last frame (at
    /// least 1 ms is used, so a zero or negative value can't stall the timers). Returns brightness values
    /// in 0..1 per LED.
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
            // While not idle any sound resets the silence timer; while idle, only sound that has lasted
            // IDLE_RESUME_SEC ends the idle version (a single click or cough doesn't).
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
///
/// `foreground_value` is the brightest LED value allowed now (follows the screen brightness; at least
/// 1 is used). Each brightness is scaled by it and rounded half to even (as numpy does); anything
/// brighter than 0.02 is shown as at least 1 so faint trails don't vanish at low brightness, and
/// anything at or below 0.02 (or NaN) is off.
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
