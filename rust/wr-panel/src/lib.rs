//! Talking to the LED panels: the bytes of each command (`protocol`), finding which serial port is the
//! left or the right panel (`ports`), and a panel's drawing thread that sends it frames and finds it
//! again when it moves (`panel`). `winports` lists and opens the serial ports on Windows.

pub mod panel;
pub mod ports;
pub mod protocol;
#[cfg(windows)]
pub mod winports;
