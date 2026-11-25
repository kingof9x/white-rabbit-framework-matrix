//! Measuring how loud each frequency band is, as a bar height from 0 to 34 (Python's `looks.Analyzer`).
//!
//! Where it runs: each `Visualizer` (one per panel, on the monitor's `music-N` thread) owns an
//! `Analyzer` and calls `levels` once per frame (~33 times a second) with the latest mono samples.
//! No threads, no I/O: plain maths, compared with Python's numbers in `tests/golden.rs`.
//!
//! How a frame is measured (`band_db`):
//! 1. Take the last 4096 samples (85 ms at 48 kHz), padded with zeros in front if fewer came.
//! 2. Remove the average (a DC offset would show as bass) and multiply by a Hann window, which fades
//!    the ends to zero so the cut-off edges don't smear energy over every band.
//! 3. Real FFT: 2049 frequency bins, 11.7 Hz apart (48000 / 4096).
//! 4. Add up the power of the bins in each band. The bands are spaced evenly on a log scale from
//!    22.3 Hz to 11.3 kHz (so each covers the same musical interval), the way the ear hears pitch.
//! 5. Turn each band's power into an RMS amplitude (undoing the FFT's and the window's scaling) and
//!    then into decibels.
//!
//! Then `levels` maps the decibels to bar heights. Two quite different schemes:
//! playback (what the speakers play, a clean digital signal) maps a fixed dB range (-60..0 dB) onto the
//! bars, optionally with an automatic gain that keeps the loudest band near the top; the microphone
//! (noisy, quiet, room-dependent) measures each band's own background noise over the last 10 seconds
//! and shows only how far the sound rises above it.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use crate::npmath;
use crate::settings::InputMode;

/// Full bar height: the panel is 34 LEDs tall.
pub const MAX_LEVEL: f64 = 34.0;
/// Samples per second the analysis assumes (the capture side asks the sound device for 48 kHz).
pub const SAMPLE_RATE: usize = 48000;
/// Samples analysed per frame (85 ms): enough to tell the lowest bass notes apart.
pub const FFT_SIZE: usize = 4096;
/// Band edges: nine octaves centred on 31.5 Hz ... 8 kHz, like the original equalizer.
/// Lowest band edge in Hz (half an octave below 31.5 Hz).
pub const LOW_HZ: f64 = 22.3;
/// Highest band edge in Hz (half an octave above 8 kHz).
pub const HIGH_HZ: f64 = 11314.0;

/// dB per sensitivity step away from 5.
/// Playback only: sensitivity 5 adds 0 dB, 10 adds +15 dB, 1 adds -12 dB.
pub const SENSITIVITY_STEP_DB: f64 = 3.0;
/// Playback auto sensitivity: the automatic gain stays within +-20 dB.
pub const AUTO_LIMIT_DB: f64 = 20.0;
/// Playback auto sensitivity: sound less than this far above the bottom of the range (-60 + 15 =
/// -45 dB) is treated as silence or hiss, and the gain is not raised for it.
pub const AUTO_MIN_SIGNAL_DB: f64 = 15.0;
/// How fast the automatic gain drops when it gets loud (fast, so a sudden loud part isn't clipped long).
pub const AUTO_DOWN_DB_PER_SEC: f64 = 25.0;
/// How fast the automatic gain rises when it gets quiet (slow, so a short pause doesn't pump the bars).
pub const AUTO_UP_DB_PER_SEC: f64 = 5.0;
/// Microphone: how many seconds of history the room's noise floor is taken from.
pub const MIC_FLOOR_SECONDS: f64 = 10.0;
/// Microphone: one history entry is kept every 0.1 s (so 100 entries in all).
pub const MIC_FLOOR_STEP: f64 = 0.1;
/// Microphone: the noise floor of a band is this percentile of its history (30th: below most sound,
/// above the odd dropout).
pub const MIC_FLOOR_PERCENTILE: f64 = 30.0;
/// Microphone: sound must be this far above the floor before a bar starts to rise, so the noise itself
/// stays dark.
pub const MIC_MARGIN_DB: f64 = 6.0;
/// Microphone auto: the full bar height is never less than this many dB above the floor (keeps
/// quiet noise from filling the panel).
pub const MIC_MIN_SPAN_DB: f64 = 12.0;
/// Microphone without auto sensitivity: a full bar is this many dB above floor + margin.
pub const MIC_FIXED_SPAN_DB: f64 = 30.0;
/// Microphone auto: how fast the remembered loudest sound fades (dB per second), so the scale adapts
/// slowly after a loud moment.
pub const MIC_PEAK_FALL_DB_PER_SEC: f64 = 1.5;
/// The loudest sound sets the bars' full height, but never more than this far above the room's noise.
pub const MIC_MAX_SPAN_DB: f64 = 40.0;
/// Quieter than this means the microphone gave nothing at all (muted, busy or stalled), not a quiet room.
pub const MIC_NOTHING_DB: f64 = -150.0;

/// Loudness (dB) shown as an empty bar and as a full bar.
/// A microphone picks up much quieter levels than playback, hence its lower range (only the playback
/// range is used for mapping; the microphone uses its own floor-based scheme in `microphone_levels`).
pub fn level_range_db(mode: InputMode) -> (f64, f64) {
    match mode {
        InputMode::Playback => (-60.0, 0.0),
        InputMode::Microphone => (-80.0, -30.0),
    }
}

/// Bars lower than this many levels (out of 34) are shown as 0, hiding flicker at the bottom.
pub fn default_noise_gate(mode: InputMode) -> i64 {
    match mode {
        InputMode::Playback => 1,
        InputMode::Microphone => 2,
    }
}

/// One panel's measuring state. Created by `Visualizer::new` from the settings, and kept for as long as
/// the look runs, because the automatic gain and the microphone's history build up over time.
pub struct Analyzer {
    /// How many bands (bars) to measure; set by the look (one per bar or row it draws).
    pub n_bands: usize,
    /// The Hann window, FFT_SIZE values from 0 up to 1 and back.
    window: Vec<f64>,
    /// RMS of the window, used to undo the loudness the window takes away.
    window_rms: f64,
    /// For each band, the FFT bin indexes inside it (at least one: a band narrower than one bin gets the
    /// nearest bin).
    bins: Vec<Vec<usize>>,
    /// Playback: the loudness (dB) shown as an empty bar.
    pub bottom_db: f64,
    /// Playback: the loudness (dB) shown as a full bar.
    pub top_db: f64,
    /// Playback: dB added for the sensitivity setting and the old `level-gain` setting.
    pub offset_db: f64,
    /// Automatic sensitivity on or off.
    pub auto: bool,
    /// Playback: the automatic gain now (dB), moved a little each frame towards its target.
    pub auto_db: f64,
    /// True for microphone input (uses `microphone_levels`).
    pub microphone: bool,
    /// The sensitivity setting, 1..10 (5 = neutral).
    sensitivity: i64,
    /// The old `level-gain` setting as dB (`level_gain` 2.0 = +6 dB).
    gain_db: f64,
    /// Microphone: band dB values, one entry every 0.1 s, oldest first, at most 10 s worth.
    history: Vec<Vec<f64>>,
    /// Microphone: seconds since the last history entry.
    history_wait: f64,
    /// Microphone auto: the loudest recent sound, in dB above the floor (fades over time).
    peak_above: f64,
    /// Bars below this level are set to 0 (0..34).
    noise_gate: f64,
    /// The planned real FFT of FFT_SIZE samples (planning once is much faster than per frame).
    fft: Arc<dyn RealToComplex<f64>>,
}

impl Analyzer {
    /// level_gain and noise_gate: the older equalizer settings (None for the defaults).
    /// `sensitivity` is 1..10 (not checked here: `settings::normalize` clamps it). `level_gain` below 0.01
    /// counts as 0.01 (-40 dB). `noise_gate` is clamped to 0..34.
    pub fn new(
        n_bands: usize,
        mode: InputMode,
        sensitivity: i64,
        auto: bool,
        level_gain: f64,
        noise_gate: Option<i64>,
    ) -> Self {
        let window = npmath::hanning(FFT_SIZE);
        let squares: Vec<f64> = window.iter().map(|w| w * w).collect();
        let window_rms = npmath::mean(&squares).sqrt();
        // np.fft.rfftfreq
        // The frequency of each FFT bin: bin k is k * 48000 / 4096 Hz (about 11.7 Hz apart).
        let val = 1.0 / (FFT_SIZE as f64 * (1.0 / SAMPLE_RATE as f64));
        let freqs: Vec<f64> = (0..FFT_SIZE / 2 + 1).map(|k| k as f64 * val).collect();
        // n_bands + 1 edges spaced by a constant ratio; band i is [edges[i], edges[i + 1]).
        let edges = npmath::geomspace(LOW_HZ, HIGH_HZ, n_bands + 1);
        let bins = edges
            .windows(2)
            .map(|pair| {
                let (low, high) = (pair[0], pair[1]);
                let idx: Vec<usize> = (0..freqs.len()).filter(|&i| freqs[i] >= low && freqs[i] < high).collect();
                if !idx.is_empty() {
                    return idx;
                }
                // A band narrower than the FFT's resolution: use the nearest frequency
                let centre = (low * high).sqrt();
                let mut best = 0;
                for i in 1..freqs.len() {
                    if (freqs[i] - centre).abs() < (freqs[best] - centre).abs() {
                        best = i;
                    }
                }
                vec![best]
            })
            .collect();
        let (bottom_db, top_db) = level_range_db(mode);
        // An amplitude factor turned into dB. A NaN `level_gain` gives NaN, which is replaced by 0 dB below.
        let gain_db = 20.0 * level_gain.max(0.01).log10();
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        let noise_gate = noise_gate.unwrap_or_else(|| default_noise_gate(mode)).clamp(0, MAX_LEVEL as i64) as f64;
        Analyzer {
            n_bands,
            window,
            window_rms,
            bins,
            bottom_db,
            top_db,
            offset_db: (sensitivity - 5) as f64 * SENSITIVITY_STEP_DB + gain_db,
            auto,
            auto_db: 0.0,
            microphone: mode == InputMode::Microphone,
            sensitivity,
            gain_db,
            history: Vec::new(),
            history_wait: 0.0,
            peak_above: MIC_MIN_SPAN_DB,
            noise_gate,
            fft: RealFftPlanner::<f64>::new().plan_fft_forward(FFT_SIZE),
        }
    }

    /// Loudness of each band in dB (0 dB: a full-scale sine wave).
    ///
    /// More exactly: 20 * log10 of the band's RMS amplitude, where an RMS of 1.0 is 0 dB. A full-scale sine
    /// (amplitude 1) whose energy lands in one band reads about -3 dB. Silence reads -200 dB (the 1e-10
    /// added before the log keeps it finite). Uses only the last FFT_SIZE samples.
    pub fn band_db(&self, samples: &[f64]) -> Vec<f64> {
        let take = samples.len().min(FFT_SIZE);
        // Zeros in front when fewer than FFT_SIZE samples came, then the newest samples.
        let mut x = vec![0.0; FFT_SIZE - take];
        x.extend_from_slice(&samples[samples.len() - take..]);
        // Removing the mean takes out any DC offset, which would otherwise show up in the lowest band.
        let m = npmath::mean(&x);
        let mut input: Vec<f64> = x.iter().zip(&self.window).map(|(v, w)| (v - m) * w).collect();
        let mut spectrum = self.fft.make_output_vec();
        // Can only fail if the buffer sizes are wrong, and both have the planned sizes.
        self.fft.process(&mut input, &mut spectrum).expect("FFT sizes match");
        let power: Vec<f64> = spectrum.iter().map(|c| c.norm().powi(2)).collect();
        // Parseval: the bins of one side of the spectrum hold half of N * (sum of squared windowed
        // samples). Doubling that and dividing by N * window_rms gives the band's RMS amplitude of the
        // original (unwindowed) signal.
        let scale = FFT_SIZE as f64 * self.window_rms;
        self.bins
            .iter()
            .map(|idx| {
                let band: Vec<f64> = idx.iter().map(|&i| power[i]).collect();
                let rms = (2.0 * npmath::sum(&band)).sqrt() / scale;
                20.0 * (rms + 1e-10).log10()
            })
            .collect()
    }

    /// Sets every bar below the noise gate to 0.
    fn gate(&self, levels: &mut [f64]) {
        for v in levels.iter_mut() {
            if *v < self.noise_gate {
                *v = 0.0;
            }
        }
    }

    /// Microphone bar heights: how far each band is above its own noise floor (30th percentile of the last
    /// 10 s), scaled so `span` dB above floor + margin is a full bar. Updates the history and the remembered
    /// peak. `dt` is the time since the last call in seconds.
    fn microphone_levels(&mut self, samples: &[f64], dt: f64) -> Vec<f64> {
        let db = self.band_db(samples);
        if npmath::max(&db) < MIC_NOTHING_DB {
            // Nothing recorded: show nothing, and keep the room's noise and the loudest sound as they were
            return vec![0.0; self.n_bands];
        }
        let db: Vec<f64> = db.iter().map(|v| v + self.gain_db).collect();
        // The first frame seeds the history, so there is a floor to compare with straight away.
        if self.history.is_empty() {
            self.history.push(db.clone());
        }
        self.history_wait += dt;
        if self.history_wait >= MIC_FLOOR_STEP {
            self.history_wait = 0.0;
            self.history.push(db.clone());
            let keep = (MIC_FLOOR_SECONDS / MIC_FLOOR_STEP) as usize;
            if self.history.len() > keep {
                let extra = self.history.len() - keep;
                self.history.drain(..extra);
            }
        }
        // A low percentile rather than the minimum: a band of noise flickers by several dB from frame to frame
        let above: Vec<f64> = (0..self.n_bands)
            .map(|b| {
                let column: Vec<f64> = self.history.iter().map(|h| h[b]).collect();
                db[b] - npmath::percentile(&column, MIC_FLOOR_PERCENTILE) - MIC_MARGIN_DB
            })
            .collect();
        let mut span = if self.auto {
            // The loudest recent sound fills the panel, but background noise is never stretched to fill it
            self.peak_above = MIC_MAX_SPAN_DB
                .min(npmath::max(&above).max(self.peak_above - MIC_PEAK_FALL_DB_PER_SEC * dt).max(MIC_MIN_SPAN_DB));
            self.peak_above * 1.1
        } else {
            MIC_FIXED_SPAN_DB
        };
        // Each sensitivity step above 5 makes the bars about 15% taller
        span *= 0.87f64.powf((self.sensitivity - 5) as f64);
        // dB above floor + margin as a fraction of the span, times 34 bars, clipped to the panel.
        let mut levels: Vec<f64> = above.iter().map(|a| npmath::clip(a / span * MAX_LEVEL, 0.0, MAX_LEVEL)).collect();
        self.gate(&mut levels);
        levels
    }

    /// Bar heights (0-34) for the latest sound; dt is the time since the last call.
    /// `samples` is mono audio in -1..1 (the newest last). Returns `n_bands` values in 0..34 (not rounded).
    /// Updates the automatic gain (playback) or the noise history (microphone).
    pub fn levels(&mut self, samples: &[f64], dt: f64) -> Vec<f64> {
        if self.microphone {
            return self.microphone_levels(samples, dt);
        }
        let mut db: Vec<f64> = self.band_db(samples).iter().map(|v| v + self.offset_db).collect();
        if self.auto {
            let loudest = npmath::max(&db);
            // Gain that puts the loudest band just under the top
            // 4 dB of headroom under the top, and never more than 20 dB of boost or cut. Because `db` already
            // includes the sensitivity offset, the automatic gain largely cancels the sensitivity setting
            // except near those limits and for quiet sound.
            let mut target = (-AUTO_LIMIT_DB).max(AUTO_LIMIT_DB.min((self.top_db - 4.0) - loudest));
            if loudest < self.bottom_db + AUTO_MIN_SIGNAL_DB {
                // Silence or the hiss of a quiet room: don't turn it up
                target = target.min(self.auto_db);
            }
            // Move towards the target at a limited speed: down fast, up slowly.
            if target < self.auto_db {
                self.auto_db = target.max(self.auto_db - AUTO_DOWN_DB_PER_SEC * dt);
            } else {
                self.auto_db = target.min(self.auto_db + AUTO_UP_DB_PER_SEC * dt);
            }
            for v in db.iter_mut() {
                *v += self.auto_db;
            }
        }
        let mut levels: Vec<f64> = db
            .iter()
            // Linear in dB: -60 dB is an empty bar, 0 dB a full one, -30 dB half.
            .map(|v| npmath::clip((v - self.bottom_db) / (self.top_db - self.bottom_db) * MAX_LEVEL, 0.0, MAX_LEVEL))
            .collect();
        self.gate(&mut levels);
        levels
    }
}
