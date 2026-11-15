//! Made-up music for previews and tests (Python's `looks.demo_samples`).

use std::f64::consts::PI;

use crate::analyzer::{FFT_SIZE, SAMPLE_RATE};
use crate::visualizer::Stereo;

/// A beat, a bass line and a few notes, as FFT_SIZE stereo samples ending at time t (seconds).
pub fn demo_samples(t: f64) -> Vec<Stereo> {
    let sr = SAMPLE_RATE as f64;
    let seconds = FFT_SIZE as f64 / sr;
    let n = (seconds * sr) as usize;
    let bass_hz = 2.0 * PI * (55.0 + 10.0 * (t * 0.7).sin());
    let notes: Vec<(f64, f64)> = [(1.3, 220.0), (0.9, 440.0), (1.7, 880.0), (2.3, 1760.0), (0.5, 3520.0)]
        .iter()
        .map(|&(k, f)| (0.12 * (0.6 + 0.4 * (t * k).sin()), 2.0 * PI * f))
        .collect();
    let hat_hz = 2.0 * PI * 7000.0;
    (0..n)
        .map(|i| {
            let time = t - seconds + i as f64 / sr;
            let beat = (-crate::npmath::py_mod(time * 2.0, 1.0) * 6.0).exp();
            let bass = 0.5 * beat * (bass_hz * time).sin();
            let mut sum = 0.0;
            for &(amp, w) in &notes {
                sum += amp * (w * time).sin();
            }
            let hats = 0.03 * (-crate::npmath::py_mod(time * 4.0, 1.0) * 30.0).exp() * (hat_hz * time).sin();
            [bass + sum + hats, bass + 0.8 * sum + 1.2 * hats]
        })
        .collect()
}
