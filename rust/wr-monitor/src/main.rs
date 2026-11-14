//! White Rabbit's monitor in Rust (test build): finds the LED panels, shows the layout's apps on them,
//! and puts an icon in the system tray. Windows only for now; the Python monitor stays for Linux.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod icon;
mod log;
mod monitor;
mod panel;
mod paths;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod sensors;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod winports;

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Rust monitor runs on Windows only for now; on Linux use the Python monitor (led_mon).");
    std::process::exit(1);
}
