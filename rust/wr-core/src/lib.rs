//! White Rabbit's monitor, the parts that are the same on every platform: drawing the apps onto a
//! panel, the layout file and which app shows when, and the log file. Talking to the panels is in
//! `wr-panel`.
//!
//! Everything here mirrors the Python monitor in `led_mon/`, and draws exactly what it draws:
//! `tests/golden.rs` compares the pictures with the ones Python saved in `tests/golden/`.
//!
//! The crate has no threads of its own and does no I/O except reading the layout/settings files it is
//! given and the log, so it builds and is tested on any platform. The pieces:
//! - `grid`: the 9 x 34 picture of one panel (`Grid`), values 0-255.
//! - `layout`: the layout file (four quarters, each a list of apps with durations) and `Rotation`,
//!   which says which app each quarter shows at a given time.
//! - `render`: draws a panel's apps into a grid from `Readings` (CPU, memory, battery, ...; the
//!   monitor reads Windows, the tests give fixed values); `apps/` has the apps that need more.
//! - `effects`, `plug_looks`, `animations`, `sand`: what moves on its own (plug ripples, slides, the
//!   text line, the old-TV switch-off, animated looks).
//! - `pyrand`, `nprand`, `pyround`: Python's and numpy's random numbers and rounding, so the pictures
//!   match Python pixel for pixel.
//! - `queue`: the newest-frame-wins queue between the main loop and a panel's drawing thread.
//! - `settings`, `log`: settings.json and the log file.

pub mod animations;
pub mod apps;
pub mod drawing;
pub mod effects;
pub mod grid;
pub mod layout;
pub mod log;
pub mod monitors;
pub mod nprand;
pub mod patterns;
#[rustfmt::skip]
mod patterns_data;
pub mod plug_looks;
pub mod pyrand;
pub mod pyround;
pub mod queue;
pub mod render;
pub mod sand;
pub mod settings;

pub use grid::{Grid, HEIGHT, WIDTH};
