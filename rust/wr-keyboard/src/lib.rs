//! The Framework 16 RGB keyboard and RGB macropad (f16custom firmware), brought over from the
//! Framework 16 Keyboard Control app (kingof9x/framework-16-keyboard-customization):
//! the raw HID message format ([`protocol`]), the combined keyboard + macropad layout ([`layout`]),
//! the PC-drawn effects ([`effects`]), talking to the devices ([`device`]), the lighting engine
//! ([`engine`]) and the keyboard picture ([`render`]).

pub mod audio;
pub mod builtin;
pub mod effects;
pub mod keycodes;
mod keymap_data;
pub mod keys;
pub mod layout;
pub mod protocol;

// Talking to the devices, the pretend devices for --demo, the lighting engine and the keyboard picture
pub mod audio_src;
pub mod deck;
pub mod device;
pub mod engine;
#[cfg(test)]
mod fn_lock_stress;
pub mod render;
pub mod settings;
pub mod shortcut;
pub mod sim;
