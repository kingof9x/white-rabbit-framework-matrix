//! The app transition (slide, fade, pixel rain, zipper) when the window takes the panels for Math or a
//! game: from the monitor's last frame on each panel to the window's first frames. The monitor hands
//! its last frames over when the window borrows the panels; the transition itself is wr-core's.
//!
//! How it fits together: `Lender::borrow` (`panels.rs`) stores the frames the monitor returned with its
//! "hold" reply; a job that starts drawing (a game or a Math picture, on its own worker thread) takes
//! them with `Lender::take_last_frames` and builds a `LeadIn`. For each frame of its own picture it
//! then calls `writes`, which runs the picture through wr-core's `Effects` (the same transition code
//! the monitor uses when it switches apps), so the user sees the monitor's picture slide or fade into
//! the window's. Once `active()` turns false the job sends its frames as usual.
//!
//! Only the first two panels (left and right) get a transition; any further panel just shows the
//! window's picture.

use std::time::Instant;

use serde_json::Value;
use wr_core::effects::{Effects, EffectsSettings, Side, Transition};
use wr_core::grid::{Grid, HEIGHT, WIDTH};

use crate::commands as cmd;
use crate::games::{Pic, PANEL_WIDTH};
use crate::panels::Panel;
use crate::settings;

/// A panel's frame as the monitor drew it, column by column.
pub type Frame = [[u8; HEIGHT]; WIDTH];

/// One transition in progress, owned by the job thread that draws.
pub struct LeadIn {
    /// wr-core's effects engine, set up with the user's transition settings and primed with the old frames.
    fx: Effects,
    /// When the transition began; `writes` measures its clock (seconds) from here.
    start: Instant,
}

impl LeadIn {
    /// From these frames (one per panel, in the panels' order); None when there is nothing to play.
    ///
    /// Reads the "effects" settings from settings.json (the same ones the monitor uses), so the window's
    /// transition matches the one the user picked for the monitor.
    pub fn new(old: &[Option<Frame>]) -> Option<LeadIn> {
        let mut s = EffectsSettings::from_json(&settings::get("effects").unwrap_or(Value::Null));
        Self::with(&mut s, old)
    }

    /// `new` with the settings given, for tests. Returns None when the transition is "cut" (switch at
    /// once) or neither of the first two panels has an old frame.
    fn with(s: &mut EffectsSettings, old: &[Option<Frame>]) -> Option<LeadIn> {
        if s.transition() == Transition::Cut || old.iter().take(2).all(Option::is_none) {
            return None;
        }
        // Only the transition: the monitor plays the messages, not the window
        s.text_line = false;
        s.show_vertical = false;
        let mut fx = Effects::new();
        fx.configure(s.clone());
        for (i, frame) in old.iter().take(2).enumerate() {
            let side = Side::ALL[i];
            fx.panel_present(side, true);
            if let Some(frame) = frame {
                let mut g = Grid::new();
                for (x, col) in frame.iter().enumerate() {
                    for (y, v) in col.iter().enumerate() {
                        // Brightness 0..255 per LED, as the monitor sent it.
                        g.set(x, y, *v as i32);
                    }
                }
                // Show the old frame once at time 0 so the engine has it as "the last frame" on this side, then
                // start the slide: from now on `apply` mixes from that frame to whatever new grid it is given.
                fx.apply(side, &g, true, 0.0);
                fx.start_slide(side, 0.0);
            }
        }
        Some(LeadIn { fx, start: Instant::now() })
    }

    /// Still playing: send every frame, even one that looks the same as the last.
    pub fn active(&self) -> bool {
        Side::ALL.iter().any(|&s| self.fx.player(s).active())
    }

    /// The commands to send for this frame of the window's picture, with the transition over it.
    ///
    /// `frame` is the window's whole picture, all panels side by side (PANEL_WIDTH columns each). Returns
    /// (port, greyscale commands) for each panel, ready for `Sender::write_many`.
    pub fn writes(&mut self, frame: &Pic, panels: &[Panel]) -> Vec<(String, Vec<Vec<u8>>)> {
        let now = self.start.elapsed().as_secs_f64();
        self.grids(frame, panels.len(), now)
            .into_iter()
            .zip(panels)
            .map(|(g, p)| (p.device.clone(), cmd::greyscale(&g.to_bytes())))
            .collect()
    }

    /// Cuts the wide picture into one grid per panel and runs the first two through the transition at
    /// time `now` (seconds since the start).
    fn grids(&mut self, frame: &Pic, count: usize, now: f64) -> Vec<Grid> {
        let bytes = frame.bytes();
        (0..count)
            .map(|i| {
                let mut g = Grid::new();
                for x in 0..WIDTH {
                    for y in 0..HEIGHT {
                        // Column x of panel i is column i*9 + x of the wide picture; the picture is stored
                        // row by row (index y * w + column). Columns past the picture's width stay dark.
                        let fx = i * PANEL_WIDTH + x;
                        if fx < frame.w {
                            g.set(x, y, bytes[y * frame.w + fx] as i32);
                        }
                    }
                }
                if i < 2 {
                    self.fx.apply(Side::ALL[i], &g, true, now)
                } else {
                    g
                }
            })
            .collect()
    }
}

/// Tests for the window's app transition.
#[cfg(test)]
mod tests {
    use super::*;

    /// The transition starts on the monitor's frame, is active while playing, and has finished (showing
    /// the window's dark picture) two seconds later.
    #[test]
    fn slides_from_the_monitors_frame_to_the_window_and_stops() {
        let mut s = EffectsSettings::default();
        let mut lead = LeadIn::with(&mut s, &[Some([[160; HEIGHT]; WIDTH])]).unwrap();
        let dark = Pic::new(PANEL_WIDTH);
        let first = lead.grids(&dark, 1, 0.0);
        assert!(first[0].v.iter().flatten().any(|&v| v == 160), "starts on the monitor's picture");
        assert!(lead.active());
        let last = lead.grids(&dark, 1, 2.0);
        assert!(last[0].is_dark());
        assert!(!lead.active());
    }

    /// No transition is made for the "cut" setting or when there are no old frames.
    #[test]
    fn nothing_to_play_with_cut_or_no_frames() {
        let mut cut = EffectsSettings { slide: false, ..EffectsSettings::default() };
        assert!(LeadIn::with(&mut cut, &[Some([[160; HEIGHT]; WIDTH])]).is_none());
        assert!(LeadIn::with(&mut EffectsSettings::default(), &[None, None]).is_none());
    }
}
