//! White Rabbit's monitor, the parts that are the same on every platform: drawing the apps onto a
//! panel, the layout file and which app shows when, and the log file. Talking to the panels is in
//! `wr-panel`.
//!
//! Everything here mirrors the Python monitor in `led_mon/`, and draws exactly what it draws:
//! `tests/golden.rs` compares the pictures with the ones Python saved in `tests/golden/`.

pub mod animations;
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
pub mod pyrand;
pub mod pyround;
pub mod queue;
pub mod render;
pub mod sand;
pub mod settings;

pub use grid::{Grid, HEIGHT, WIDTH};
