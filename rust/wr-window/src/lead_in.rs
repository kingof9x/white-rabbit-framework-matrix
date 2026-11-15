//! The app transition (slide, fade, pixel rain, zipper) when the window takes the panels for Math or a
//! game: from the monitor's last frame on each panel to the window's first frames. The monitor hands
//! its last frames over when the window borrows the panels; the transition itself is wr-core's.

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

pub struct LeadIn {
    fx: Effects,
    start: Instant,
}

impl LeadIn {
    /// From these frames (one per panel, in the panels' order); None when there is nothing to play.
    pub fn new(old: &[Option<Frame>]) -> Option<LeadIn> {
        let mut s = EffectsSettings::from_json(&settings::get("effects").unwrap_or(Value::Null));
        Self::with(&mut s, old)
    }

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
                        g.set(x, y, *v as i32);
                    }
                }
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
    pub fn writes(&mut self, frame: &Pic, panels: &[Panel]) -> Vec<(String, Vec<Vec<u8>>)> {
        let now = self.start.elapsed().as_secs_f64();
        self.grids(frame, panels.len(), now)
            .into_iter()
            .zip(panels)
            .map(|(g, p)| (p.device.clone(), cmd::greyscale(&g.to_bytes())))
            .collect()
    }

    fn grids(&mut self, frame: &Pic, count: usize, now: f64) -> Vec<Grid> {
        let bytes = frame.bytes();
        (0..count)
            .map(|i| {
                let mut g = Grid::new();
                for x in 0..WIDTH {
                    for y in 0..HEIGHT {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn nothing_to_play_with_cut_or_no_frames() {
        let mut cut = EffectsSettings { slide: false, ..EffectsSettings::default() };
        assert!(LeadIn::with(&mut cut, &[Some([[160; HEIGHT]; WIDTH])]).is_none());
        assert!(LeadIn::with(&mut EffectsSettings::default(), &[None, None]).is_none());
    }
}
