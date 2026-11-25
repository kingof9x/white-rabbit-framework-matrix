//! WhiteRabbit.exe is this Rust monitor; the window is WhiteRabbitWindow.exe next to it (wr-window, also Rust).
//! Opening the app, the tray's "Open White Rabbit..." and anything meant for the window start that exe.
//!
//! `app::run` calls `start_from` first thing to decide what this launch is for, and `open_window`
//! is called from start-up, the tray menu and the control commands. Starting the window is
//! fire-and-forget: the child process is not waited for; the two talk later over the control channel.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use wr_core::{info, warn};

/// The window's exe name, next to WhiteRabbit.exe in the installed app.
pub const WINDOW_EXE: &str = "WhiteRabbitWindow.exe";

/// Options the window (WhiteRabbitWindow.exe) handles: the window, its layout editor and its reports.
const WINDOW_OPTIONS: [&str; 8] = [
    "--window",
    "--edit-layout",
    "--find-lhm",
    "--open-lhm",
    "--open-framework-control",
    "--diagnose",
    "--list-apps",
    "--tab",
];

/// What a start of WhiteRabbit.exe should do.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// Run the monitor and open the window (a plain start, e.g. the Start menu).
    MonitorAndWindow,
    /// Run the monitor only (Start with Windows).
    Background,
    /// Hand these arguments to the window exe and exit.
    Window(Vec<String>),
}

/// Decides what this start is for from the command-line arguments (program name already removed).
/// `--background` wins over everything; any window option sends all arguments to the window;
/// anything else (including unknown options) runs the monitor and opens the window.
pub fn start_from(args: &[String]) -> Start {
    if args.iter().any(|a| a == "--background") {
        Start::Background
    } else if args.iter().any(|a| WINDOW_OPTIONS.contains(&a.as_str())) {
        Start::Window(args.to_vec())
    } else {
        Start::MonitorAndWindow
    }
}

/// The full path WhiteRabbitWindow.exe should have: the folder of the running exe. None only if
/// Windows can't tell us where we are running from. Does not check that the file exists.
pub fn window_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(WINDOW_EXE))
}

/// Set once the "window exe is missing" warning has been logged, so it is logged only once per run.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Starts the window exe with these arguments. False when it isn't there or won't start.
///
/// Side effect: starts a separate process (not waited for) and logs the result.
pub fn open_window(args: &[&str]) -> bool {
    let Some(exe) = window_exe().filter(|p| p.exists()) else {
        // `swap` returns the old value: only the first caller to get here sees `false` and logs.
        if !WARNED.swap(true, Ordering::SeqCst) {
            warn!("The window ({WINDOW_EXE}) isn't next to WhiteRabbit.exe, so it can't be opened");
        }
        return false;
    };
    match Command::new(&exe).args(args).spawn() {
        Ok(_) => {
            info!("Opened the window {}", args.join(" "));
            true
        }
        Err(e) => {
            warn!("Couldn't open the window ({}): {e}", exe.display());
            false
        }
    }
}

/// Tests for how command-line options are split between the monitor and the window.
#[cfg(test)]
mod tests {
    use super::*;

    /// Turns string literals into the owned `Vec<String>` that `start_from` takes.
    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// No options, or unknown ones, run the monitor and open the window.
    #[test]
    fn plain_start_runs_both() {
        assert_eq!(start_from(&[]), Start::MonitorAndWindow);
        // Unknown options don't bounce between the two exes
        assert_eq!(start_from(&s(&["--something"])), Start::MonitorAndWindow);
    }

    /// `--background` starts the monitor only, even if a window option is also given.
    #[test]
    fn background_wins() {
        assert_eq!(start_from(&s(&["--background", "--window"])), Start::Background);
    }

    /// Window options hand over all the arguments unchanged, including the value after `--tab`.
    #[test]
    fn window_options_go_to_the_window() {
        assert_eq!(start_from(&s(&["--window", "--tab", "audio"])), Start::Window(s(&["--window", "--tab", "audio"])));
        assert_eq!(start_from(&s(&["--diagnose"])), Start::Window(s(&["--diagnose"])));
    }
}
