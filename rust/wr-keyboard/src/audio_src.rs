//! Listening for audio mode: what the PC is playing, measured into frequency bands by White
//! Rabbit's own code (`wr-music`), so both apps hear the same thing the same way. In `--demo`
//! (and off Windows) a made-up beat stands in for the sound.

use std::sync::Arc;
use wr_music::analyzer::{Analyzer, MAX_LEVEL};
use wr_music::capture::{self, Capture, InputMode};

/// Bands measured (bass first).
pub const BANDS: usize = 16;

pub struct Listener {
    capture: Option<Arc<Capture>>,
    analyzer: Analyzer,
    t: f32,
}

impl Listener {
    /// `sensitivity`: 1 (quiet music shows little) to 10 (everything shows a lot), 5 = normal.
    /// `microphone`: listen to a microphone (`device`, "" = the default one) instead of what the
    /// PC plays.
    pub fn start(demo: bool, sensitivity: u8, microphone: bool, device: &str) -> Result<Self, String> {
        let mode = if microphone { InputMode::Microphone } else { InputMode::Playback };
        let capture =
            if demo { None } else { Some(capture::acquire(mode, if microphone { device } else { "" }, None)?) };
        Ok(Self {
            capture,
            analyzer: Analyzer::new(BANDS, mode, sensitivity.clamp(1, 10) as i64, true, 1.0, None),
            t: 0.0,
        })
    }

    /// The device being listened to, once it is open.
    pub fn device(&self) -> Option<String> {
        match &self.capture {
            Some(c) => c.device_name(),
            None => Some("pretend music (demo)".into()),
        }
    }

    pub fn problem(&self) -> Option<String> {
        self.capture.as_ref().and_then(|c| c.problem())
    }

    /// Band levels 0..1 for the latest sound; `dt` = seconds since the last call.
    pub fn bands(&mut self, dt: f32) -> Vec<f32> {
        self.t += dt;
        match &self.capture {
            Some(c) => {
                let samples: Vec<f64> = c.snapshot_f64().into_iter().map(|[l, r]| (l + r) * 0.5).collect();
                self.analyzer.levels(&samples, dt as f64).into_iter().map(|v| (v / MAX_LEVEL) as f32).collect()
            }
            None => demo_bands(self.t),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(c) = &self.capture {
            capture::release(c);
        }
    }
}

/// The microphones (and other recording inputs) this PC has. Slow: call it off the window's thread.
pub fn microphones() -> Vec<String> {
    capture::list_microphones()
}

/// A made-up song at 120 beats a minute: kick drum, a wandering melody and hi-hats.
fn demo_bands(t: f32) -> Vec<f32> {
    let beat = (t * 2.0).fract();
    let kick = (-beat * 7.0).exp();
    let hat = (-((t * 4.0 + 0.5).fract()) * 12.0).exp();
    (0..BANDS)
        .map(|i| {
            let f = i as f32 / (BANDS - 1) as f32;
            let melody = 0.35 + 0.25 * ((t * 0.9 + f * 6.0).sin() * (t * 0.37 + f * 2.0).cos());
            let v = kick * (1.0 - f).powi(3) + melody * (1.0 - (f - 0.5).abs()) * 0.8 + hat * f.powi(2) * 0.7;
            v.clamp(0.0, 1.0)
        })
        .collect()
}
