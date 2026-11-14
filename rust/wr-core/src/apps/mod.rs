//! Apps that show your own content rather than readings: the Text app, the Picture app (built-in
//! pictures and Draw tab drawings), and the panel names shown by Identify panels.

pub mod panel_names;
pub mod picture;
pub mod text;

/// The app names drawn by this module, added to the ones `render.rs` draws itself.
pub const CONTENT_APPS: [&str; 2] = ["text", "snap"];
