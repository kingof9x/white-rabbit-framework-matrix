//! White Rabbit's monitor in Rust (WhiteRabbit.exe): finds the LED panels, shows the layout's apps on them,
//! and puts an icon in the system tray. The window is the Python app, WhiteRabbitWindow.exe, which it opens. Windows only for now; the Python monitor stays for Linux.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod control;
mod icon;
mod launch;
mod monitor;
mod music;
mod paths;
mod power;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod background;
#[cfg(windows)]
mod gpu;
#[cfg(windows)]
mod media;
#[cfg(windows)]
mod sensors;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod thermal;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod weather;

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Rust monitor runs on Windows only for now; on Linux use the Python monitor (led_mon).");
    std::process::exit(1);
}
