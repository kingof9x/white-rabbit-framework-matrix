//! White Rabbit's monitor in Rust (WhiteRabbit.exe): finds the LED panels, shows the layout's apps on them,
//! and puts an icon in the system tray. The window is WhiteRabbitWindow.exe (wr-window), which it opens.
//! Windows only for now; the Python monitor stays for Linux.
//!
//! # How it runs
//!
//! One process that stays running from login (`--background`, the "Start with Windows" Run key) or
//! from opening the app, until Quit. `app::run` sets everything up in this order:
//!
//! 1. the log, then `launch::start_from`: options meant for the window are handed to
//!    WhiteRabbitWindow.exe and this process ends;
//! 2. the single-instance mutex: if a monitor already runs, this one only opens the window and ends;
//! 3. the keyboard part (`keyboard.rs`), then the LED panels (`wr_panel::ports::discover`, waiting up
//!    to 30 s at login for USB to settle);
//! 4. the shared state (`monitor::Shared`), the two drawing threads, the control channel, the plug
//!    watcher, the main loop; and last the tray window, which runs on the process's main thread until
//!    Quit. Quitting sets `Shared::quit`; the main loop darkens the panels and stops the drawing threads.
//!
//! # Threads
//!
//! | thread                    | what it does                                         | where            |
//! |---------------------------|------------------------------------------------------|------------------|
//! | main (tray)               | the hidden tray window: menu, hotkeys, power messages | `ui.rs`         |
//! | `main-loop`               | 10 times a second: pick the apps, draw, queue frames | `monitor.rs`     |
//! | `panel-0`, `panel-1`      | send queued frames to each panel, find it again       | `wr_panel::panel` |
//! | `control`                 | answers the window, one request at a time             | `control/`      |
//! | `holders`                 | every 2 s, takes back panels from a closed window     | `app.rs`        |
//! | `music-0`, `music-1`      | the audio visualizer of a panel, while one runs       | `music.rs`      |
//! | `audio-visualizer-*`      | sound capture (supervisor + one recorder per device)  | `wr_music::capture` |
//! | `keyboard`, `hid-engine`  | the RGB keyboard/macropad while the window is closed  | `keyboard.rs`, `wr_keyboard::engine` |
//! | `audio-shortcut`          | the low-level hook for the audio-show key             | `wr_keyboard::shortcut` |
//! | `plug-events`             | USB, charger and display plug notifications           | `plugwin.rs`    |
//! | `brightness`              | the laptop screen's brightness (WMI)                  | `sensors.rs`    |
//! | readers (`background.rs`) | sensors, GPU, weather, media, FPS: each on its own thread, so the main loop never waits for WMI, a web service or a tool | `background.rs` |
//!
//! The threads share state in three ways, never anything more clever:
//! - `monitor::Shared`: atomics for flags (pause, quit, music mode, redraw ...) and small mutexes for
//!   values (settings, preview frames, identify timers). The main loop reads them every pass; the tray
//!   and the control thread set them. A lock is held only to copy a value in or out.
//! - `wr_core::queue::FrameQueue`, one per panel: the main loop (or a visualizer) puts frames, the
//!   panel's drawing thread takes them. A new frame replaces the waiting one, so a slow panel always
//!   gets the newest picture and memory never builds up.
//! - `wr_panel::panel::Panels` and its `Hub`: which serial port each side uses, the open ports, and
//!   which ports the window holds. See `wr-panel/src/lib.rs` for its locks and their order.
//!
//! Requests that only the tray window may carry out (re-registering hotkeys, quitting) are posted to it
//! as window messages (`ui::request`), never done from the control thread.
//!
//! In a release build any panic, on any thread, ends the whole program (`panic = "abort"` in
//! `Cargo.toml`, which keeps the exe small); `monitor::log_panics` writes it to the log first. So the
//! code that runs all the time (the main loop, the drawing threads, the control channel) is written
//! not to panic on bad input, and the fuzz tests check that.
//!
//! # Where things are kept
//!
//! `paths.rs`: settings.json and the layout in `%APPDATA%\WhiteRabbit` (shared with the window), the
//! log and the control channel's key in `%LOCALAPPDATA%\WhiteRabbit`.

// In a release build on Windows the exe is a "windows subsystem" program: no console window opens when it
// starts. Debug builds keep the console so log lines and panics are visible while developing.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// On Linux only the platform-neutral modules compile (for the tests); much of their code is then unused.
#![cfg_attr(not(windows), allow(dead_code))]

/// The control channel: the named pipe the window (wr-window) uses to talk to this process.
mod control;
/// End-to-end tests that drive the main loop with fake panels and fake readings (tests only).
#[cfg(test)]
mod e2e_test;
/// Frames per second of a game, measured with the PresentMon tool.
mod fps;
/// The tray icon picture (the rabbit), drawn in code at any size.
mod icon;
/// The RGB keyboard and macropad job that runs while the window is closed.
mod keyboard;
/// Command-line parsing: which options start the monitor and which belong to the window.
mod launch;
/// Where the computer is (for the weather app).
mod location;
/// The main loop and the state shared between threads (`Shared`).
mod monitor;
/// The per-panel audio visualizer threads.
mod music;
/// Where settings, layout, log and key files live on disk.
mod paths;
/// Platform-neutral logic for plug/unplug events (which port, which effect).
mod plug;
/// When the panels should turn off: lid closed, screen off, sleep, laptop screen unused.
mod power;

/// Start-up order on Windows (see the header above).
#[cfg(windows)]
mod app;
/// Runs slow readers (sensors, weather, media ...) on their own threads and caches the latest value.
#[cfg(windows)]
mod background;
/// Windows performance counters (PDH): all disks' bytes and each GPU's memory use.
#[cfg(windows)]
mod counters;
/// GPU load and names.
#[cfg(windows)]
mod gpu;
/// The "now playing" song from Windows' media controls.
#[cfg(windows)]
mod media;
/// The Windows side of plug events: a hidden window that receives device-change and power messages.
#[cfg(windows)]
mod plugwin;
/// Collects every reading the apps draw (`WindowsReadings`) and the screen brightness thread.
#[cfg(windows)]
mod sensors;
/// The "Start with Windows" Run-key entry.
#[cfg(windows)]
mod startup;
/// Temperatures and fan speeds from Framework Control, Libre Hardware Monitor or framework_tool.
#[cfg(windows)]
mod thermal;
/// The tray icon, its menu, hotkeys and the single-instance mutex.
#[cfg(windows)]
mod ui;
/// Weather download and caching.
#[cfg(windows)]
mod weather;

/// Windows entry point: everything happens in `app::run`, whose return value becomes the exit code
/// (0 for a normal Quit or for handing over to an already-running copy).
#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

/// Non-Windows entry point: the Rust monitor is Windows-only, so it just explains that and exits with 1.
#[cfg(not(windows))]
fn main() {
    eprintln!("The Rust monitor runs on Windows only for now; on Linux use the Python monitor (led_mon).");
    std::process::exit(1);
}
