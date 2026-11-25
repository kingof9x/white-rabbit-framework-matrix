//! Talking to the LED panels: the bytes of each command (`protocol`), finding which serial port is the
//! left or the right panel (`ports`), sharing the open ports with the White Rabbit window (`hub`),
//! dimming by LED current (`dimming`), and the panels' drawing threads that send them frames and find
//! them again when they move (`panel`). `winports` lists and opens the serial ports on Windows.
//!
//! # The path of one frame
//!
//! ```text
//! main loop --put--> FrameQueue --get--> Panel::handle (drawing thread)
//!                                          |  overlay.apply (ripples, slide, text line)
//!                                          |  Panels::port(side)       which port this side uses
//!                                          |  port.lock()              the whole frame under one lock
//!                                          |    dimming::prepare       LED current (White Rabbit firmware)
//!                                          |    skip if already shown (re-sent every 5 s as a keep-alive)
//!                                          |    protocol::frame_commands -> one USB write per command
//!                                          v
//!                                      SharedPort -> Port (winports::SerialPort on Windows)
//! ```
//!
//! A failed write marks the open port broken; the drawing thread lets go of it and looks for its
//! panel again (by its last USB position first), with a growing pause (0.5 s up to 8 s) while it is
//! missing. The window's commands (`hub::Hub::serial`) go to the same `SharedPort`.
//!
//! # Locks, and the order they are taken in
//!
//! | lock                        | guards                                        | held for                 |
//! |-----------------------------|-----------------------------------------------|--------------------------|
//! | `Panels::sides`             | which port each side draws on                 | finding/opening a port   |
//! | `Hub::open`                 | the map of open ports (weak references)       | a lookup or an open      |
//! | `SharedPort::state`         | one port: its handle, what it shows, dimming  | one whole frame or command list |
//! | `Hub::held`                 | which ports the window holds, and by whom     | a lookup                 |
//! | `Panels::overlay`, `pins`   | the effects overlay, the pinned sides          | a clone                  |
//!
//! They are always taken in the order of that table, top to bottom, and a lock lower down is never
//! held while taking one higher up; that is what keeps them free of deadlocks. (`Panels::connect`
//! holds `sides` while opening the port, so two sides never claim the same panel.) `Hub::serial_many`
//! locks several ports at once, in the order of their names, so two of them can't wait on each other.
//! `tests/stress.rs` runs all of this at once with panels being unplugged, and fails on a deadlock.

pub mod dimming;
pub mod hub;
pub mod overlay;
pub mod panel;
pub mod ports;
pub mod protocol;
#[cfg(windows)]
pub mod winports;
