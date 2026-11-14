//! Apps that show your own content rather than readings. So far the Text app.

pub mod text;

/// The app names drawn by this module, added to the ones `render.rs` draws itself.
pub const CONTENT_APPS: [&str; 1] = ["text"];
