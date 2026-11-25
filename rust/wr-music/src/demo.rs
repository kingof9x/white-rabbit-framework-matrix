//! Made-up music for previews and tests (Python's `looks.demo_samples`).
//!
//! A deterministic function of time, so the same `t` always gives the same samples: the golden test
//! feeds it to every look and compares the pictures with Python's, and the window can preview a look
//! without any sound playing.

use std::f64::consts::PI;

use crate::analyzer::{FFT_SIZE, SAMPLE_RATE};
use crate::visualizer::Stereo;

/// A beat, a bass line and a few notes, as FFT_SIZE stereo samples ending at time t (seconds).
///
/// Mix: a kick-like beat twice a second (a burst that dies away), a bass note around 55 Hz that drifts
/// slowly, five notes from 220 Hz to 3.5 kHz whose loudness swells at different speeds, and a hi-hat at
/// 7 kHz four times a second. The right channel has the notes
/// a little quieter and the hi-hat louder, so the two channels differ.
pub fn demo_samples(t: f64) -> Vec<Stereo> {
    let sr = SAMPLE_RATE as f64;
    let seconds = FFT_SIZE as f64 / sr;
    let n = (seconds * sr) as usize;
    // Angular frequencies (radians per second): the bass wanders between 45 and 65 Hz.
    let bass_hz = 2.0 * PI * (55.0 + 10.0 * (t * 0.7).sin());
    let notes: Vec<(f64, f64)> = [(1.3, 220.0), (0.9, 440.0), (1.7, 880.0), (2.3, 1760.0), (0.5, 3520.0)]
        .iter()
        // (amplitude, angular frequency) of each note; each note's amplitude moves between 0.024 and 0.12.
        .map(|&(k, f)| (0.12 * (0.6 + 0.4 * (t * k).sin()), 2.0 * PI * f))
        .collect();
    let hat_hz = 2.0 * PI * 7000.0;
    (0..n)
        .map(|i| {
            let time = t - seconds + i as f64 / sr;
            // Goes from 1 down towards 0 over each half second, then jumps back to 1: a beat at 120 bpm.
            let beat = (-crate::npmath::py_mod(time * 2.0, 1.0) * 6.0).exp();
            let bass = 0.5 * beat * (bass_hz * time).sin();
            let mut sum = 0.0;
            for &(amp, w) in &notes {
                sum += amp * (w * time).sin();
            }
            // Same idea four times a second, but much shorter (decays 5x faster) and quieter.
            let hats = 0.03 * (-crate::npmath::py_mod(time * 4.0, 1.0) * 30.0).exp() * (hat_hz * time).sin();
            [bass + sum + hats, bass + 0.8 * sum + 1.2 * hats]
        })
        .collect()
}
