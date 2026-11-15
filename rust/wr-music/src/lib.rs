//! White Rabbit's music visualizer in Rust: listening to sound (Windows), turning it into one of the
//! 22 looks on each panel, and the calm idle version of a look while nothing is heard.
//!
//! Mirrors `led_mon/equalizer_files/` (looks.py, audio.py) and draws exactly what it draws:
//! `tests/golden.rs` compares the pictures with the ones Python saved in `tests/golden/`.
//!
//! For the monitor: [`capture::acquire`] a recording, make a [`MusicPanel`] per panel, and every
//! frame call `panel.frame(&capture.snapshot_f64(), dt)` then [`to_leds`] to get the panel's picture.

// The looks index rows and columns the way the Python code does, which keeps them easy to check
// against it; and max/min instead of clamp keeps numpy's behaviour for NaN (clamp would panic).
#![allow(clippy::needless_range_loop, clippy::manual_clamp, clippy::single_range_in_vec_init)]

pub mod analyzer;
pub mod capture;
pub mod demo;
pub mod npmath;
pub mod panel;
pub mod settings;
pub mod visualizer;

pub use panel::{to_leds, MusicPanel};
pub use settings::{from_json, normalize, Settings, STYLES};
pub use visualizer::{Frame, Stereo, Visualizer};
