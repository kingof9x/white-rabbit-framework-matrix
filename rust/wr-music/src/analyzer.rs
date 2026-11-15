//! Measuring how loud each frequency band is, as a bar height from 0 to 34 (Python's `looks.Analyzer`).

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use crate::npmath;
use crate::settings::InputMode;

pub const MAX_LEVEL: f64 = 34.0;
pub const SAMPLE_RATE: usize = 48000;
/// Samples analysed per frame (85 ms): enough to tell the lowest bass notes apart.
pub const FFT_SIZE: usize = 4096;
/// Band edges: nine octaves centred on 31.5 Hz ... 8 kHz, like the original equalizer.
pub const LOW_HZ: f64 = 22.3;
pub const HIGH_HZ: f64 = 11314.0;

/// dB per sensitivity step away from 5.
pub const SENSITIVITY_STEP_DB: f64 = 3.0;
pub const AUTO_LIMIT_DB: f64 = 20.0;
pub const AUTO_MIN_SIGNAL_DB: f64 = 15.0;
pub const AUTO_DOWN_DB_PER_SEC: f64 = 25.0;
pub const AUTO_UP_DB_PER_SEC: f64 = 5.0;
pub const MIC_FLOOR_SECONDS: f64 = 10.0;
pub const MIC_FLOOR_STEP: f64 = 0.1;
pub const MIC_FLOOR_PERCENTILE: f64 = 30.0;
pub const MIC_MARGIN_DB: f64 = 6.0;
pub const MIC_MIN_SPAN_DB: f64 = 12.0;
pub const MIC_FIXED_SPAN_DB: f64 = 30.0;
pub const MIC_PEAK_FALL_DB_PER_SEC: f64 = 1.5;
/// The loudest sound sets the bars' full height, but never more than this far above the room's noise.
pub const MIC_MAX_SPAN_DB: f64 = 40.0;
/// Quieter than this means the microphone gave nothing at all (muted, busy or stalled), not a quiet room.
pub const MIC_NOTHING_DB: f64 = -150.0;

/// Loudness (dB) shown as an empty bar and as a full bar.
pub fn level_range_db(mode: InputMode) -> (f64, f64) {
    match mode {
        InputMode::Playback => (-60.0, 0.0),
        InputMode::Microphone => (-80.0, -30.0),
    }
}

pub fn default_noise_gate(mode: InputMode) -> i64 {
    match mode {
        InputMode::Playback => 1,
        InputMode::Microphone => 2,
    }
}

pub struct Analyzer {
    pub n_bands: usize,
    window: Vec<f64>,
    window_rms: f64,
    bins: Vec<Vec<usize>>,
    pub bottom_db: f64,
    pub top_db: f64,
    pub offset_db: f64,
    pub auto: bool,
    pub auto_db: f64,
    pub microphone: bool,
    sensitivity: i64,
    gain_db: f64,
    history: Vec<Vec<f64>>,
    history_wait: f64,
    peak_above: f64,
    noise_gate: f64,
    fft: Arc<dyn RealToComplex<f64>>,
}

impl Analyzer {
    /// level_gain and noise_gate: the older equalizer settings (None for the defaults).
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
        let val = 1.0 / (FFT_SIZE as f64 * (1.0 / SAMPLE_RATE as f64));
        let freqs: Vec<f64> = (0..FFT_SIZE / 2 + 1).map(|k| k as f64 * val).collect();
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
    pub fn band_db(&self, samples: &[f64]) -> Vec<f64> {
        let take = samples.len().min(FFT_SIZE);
        let mut x = vec![0.0; FFT_SIZE - take];
        x.extend_from_slice(&samples[samples.len() - take..]);
        let m = npmath::mean(&x);
        let mut input: Vec<f64> = x.iter().zip(&self.window).map(|(v, w)| (v - m) * w).collect();
        let mut spectrum = self.fft.make_output_vec();
        self.fft.process(&mut input, &mut spectrum).expect("FFT sizes match");
        let power: Vec<f64> = spectrum.iter().map(|c| c.norm().powi(2)).collect();
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

    fn gate(&self, levels: &mut [f64]) {
        for v in levels.iter_mut() {
            if *v < self.noise_gate {
                *v = 0.0;
            }
        }
    }

    fn microphone_levels(&mut self, samples: &[f64], dt: f64) -> Vec<f64> {
        let db = self.band_db(samples);
        if npmath::max(&db) < MIC_NOTHING_DB {
            // Nothing recorded: show nothing, and keep the room's noise and the loudest sound as they were
            return vec![0.0; self.n_bands];
        }
        let db: Vec<f64> = db.iter().map(|v| v + self.gain_db).collect();
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
        let mut levels: Vec<f64> = above.iter().map(|a| npmath::clip(a / span * MAX_LEVEL, 0.0, MAX_LEVEL)).collect();
        self.gate(&mut levels);
        levels
    }

    /// Bar heights (0-34) for the latest sound; dt is the time since the last call.
    pub fn levels(&mut self, samples: &[f64], dt: f64) -> Vec<f64> {
        if self.microphone {
            return self.microphone_levels(samples, dt);
        }
        let mut db: Vec<f64> = self.band_db(samples).iter().map(|v| v + self.offset_db).collect();
        if self.auto {
            let loudest = npmath::max(&db);
            // Gain that puts the loudest band just under the top
            let mut target = (-AUTO_LIMIT_DB).max(AUTO_LIMIT_DB.min((self.top_db - 4.0) - loudest));
            if loudest < self.bottom_db + AUTO_MIN_SIGNAL_DB {
                // Silence or the hiss of a quiet room: don't turn it up
                target = target.min(self.auto_db);
            }
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
            .map(|v| npmath::clip((v - self.bottom_db) / (self.top_db - self.bottom_db) * MAX_LEVEL, 0.0, MAX_LEVEL))
            .collect();
        self.gate(&mut levels);
        levels
    }
}
