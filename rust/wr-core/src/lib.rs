//! White Rabbit's monitor, the parts that are the same on every platform: drawing the apps onto a
//! panel, the layout file and which app shows when, and the bytes sent to a panel.
//!
//! Everything here mirrors the Python monitor in `led_mon/`, and draws exactly what it draws:
//! `tests/golden.rs` compares the pictures with the ones Python saved in `tests/golden/`.

pub mod drawing;
pub mod grid;
pub mod layout;
pub mod monitors;
pub mod patterns;
#[rustfmt::skip]
mod patterns_data;
pub mod ports;
pub mod protocol;
pub mod pyround;
pub mod queue;
pub mod render;
pub mod settings;

pub use grid::{Grid, HEIGHT, WIDTH};
