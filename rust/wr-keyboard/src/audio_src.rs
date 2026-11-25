//! Listening for audio mode: what the PC is playing, measured into frequency bands by White
//! Rabbit's own code (`wr-music`), so both apps hear the same thing the same way. In `--demo`
//! a made-up beat stands in for the sound. (Without `--demo`, recording only works on Windows:
//! elsewhere `wr-music` refuses to start, and [`Listener::start`] returns its error.)
//!
//! The lighting engine creates one [`Listener`] when the audio show starts and asks it for band
//! levels once a frame, on the engine's `hid-engine` thread. The recording itself runs on
//! `wr-music`'s own capture thread; the listener only takes a snapshot of the latest samples.
//! Recordings are shared process-wide: if another part of the same program (such as the LED panel
//! music visualizer) already records the same input, this joins it rather than opening the sound
//! device twice, and leaves it again on drop.

use std::sync::Arc;
use wr_music::analyzer::{Analyzer, MAX_LEVEL};
use wr_music::capture::{self, Capture, InputMode};

/// Bands measured (bass first).
/// (The same number as [`crate::audio::BANDS`], so no resampling is needed.)
pub const BANDS: usize = 16;

/// A running listener: the shared recording (None in demo mode), the analyzer that turns
/// samples into band levels, and the time since it started (for the demo music).
pub struct Listener {
    /// The shared recording; None in demo mode.
    capture: Option<Arc<Capture>>,
    /// FFT and smoothing from `wr-music`, set up for 16 bands and this input.
    analyzer: Analyzer,
    /// Seconds since start, the sum of every `dt` (drives the demo music).
    t: f32,
}

impl Listener {
    /// `sensitivity`: 1 (quiet music shows little) to 10 (everything shows a lot), 5 = normal.
    /// `microphone`: listen to a microphone (`device`, "" = the default one) instead of what the
    /// PC plays.
    ///
    /// Errors (a sentence for the Audio page) come from `wr-music` when recording can't start.
    pub fn start(demo: bool, sensitivity: u8, microphone: bool, device: &str) -> Result<Self, String> {
        let mode = if microphone { InputMode::Microphone } else { InputMode::Playback };
        let capture =
            if demo { None } else { Some(capture::acquire(mode, if microphone { device } else { "" }, None)?) };
        Ok(Self {
            capture,
            // Automatic level (true), no extra gain (1.0) and the default noise gate (None).
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

    /// Why the recording isn't hearing anything right now (device gone...), if `wr-music` knows.
    pub fn problem(&self) -> Option<String> {
        self.capture.as_ref().and_then(|c| c.problem())
    }

    /// Band levels 0..1 for the latest sound; `dt` = seconds since the last call.
    pub fn bands(&mut self, dt: f32) -> Vec<f32> {
        self.t += dt;
        match &self.capture {
            Some(c) => {
                // Mix the stereo samples down to mono, then scale the analyzer's 0..34 bar heights to 0..1.
                let samples: Vec<f64> = c.snapshot_f64().into_iter().map(|[l, r]| (l + r) * 0.5).collect();
                self.analyzer.levels(&samples, dt as f64).into_iter().map(|v| (v / MAX_LEVEL) as f32).collect()
            }
            None => demo_bands(self.t),
        }
    }
}

/// Leaves the shared recording; the last one to leave stops it.
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
    // Position within the current beat (2 beats a second). The kick fades fast after each beat,
    // the hi-hat sits on the off-beats at double speed; bass bands get the kick, treble the hats,
    // and two slow waves make a "melody" wander across the middle.
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
