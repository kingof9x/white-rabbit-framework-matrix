//! The Framework 16 RGB keyboard and RGB macropad (f16custom firmware), brought over from the
//! Framework 16 Keyboard Control app (kingof9x/framework-16-keyboard-customization):
//! the raw HID message format ([`protocol`]), the combined keyboard + macropad layout ([`layout`]),
//! the PC-drawn effects ([`effects`]), talking to the devices ([`device`]), the lighting engine
//! ([`engine`]) and the keyboard picture ([`render`]).
//!
//! The engine runs on its own thread (`hid-engine`) and owns the devices: everyone else sends it
//! [`engine::Command`]s over a channel and reads its status. It finds the keyboard and macropad over
//! USB HID, sends the lighting (the devices run most effects themselves; PC-drawn effects and the
//! audio show are drawn here and streamed), and finds them again after an unplug. Only one program
//! may drive them at a time: the window while it is open, otherwise the monitor (`wr-monitor/src/
//! keyboard.rs`), handed over with the monitor's "keyboard" control command. Settings are a plain
//! `key=value` text file ([`settings`]), read forgivingly. [`sim`] is a pretend keyboard and macropad
//! for `--demo` and the tests.

// Pure logic, no USB: audio shows, the built-in effects copied for the preview, PC-drawn
// effects, key codes and keymaps (with their generated data tables), key and LED layouts, and
// the HID message format.
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
// The Fn lock stress test (test builds only).
#[cfg(test)]
mod fn_lock_stress;
pub mod render;
pub mod settings;
pub mod shortcut;
pub mod sim;
