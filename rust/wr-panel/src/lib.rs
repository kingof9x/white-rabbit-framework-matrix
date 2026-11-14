//! Talking to the LED panels: the bytes of each command (`protocol`), finding which serial port is the
//! left or the right panel (`ports`), sharing the open ports with the White Rabbit window (`hub`),
//! dimming by LED current (`dimming`), and the panels' drawing threads that send them frames and find
//! them again when they move (`panel`). `winports` lists and opens the serial ports on Windows.

pub mod dimming;
pub mod hub;
pub mod overlay;
pub mod panel;
pub mod ports;
pub mod protocol;
#[cfg(windows)]
pub mod winports;
