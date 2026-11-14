//! Apps that show your own content rather than readings: the Text app and the Picture app
//! (built-in pictures and Draw tab drawings).

pub mod picture;
pub mod text;

/// The app names drawn by this module, added to the ones `render.rs` draws itself.
pub const CONTENT_APPS: [&str; 2] = ["text", "snap"];
