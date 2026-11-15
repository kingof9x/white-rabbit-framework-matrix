//! WhiteRabbit.exe is this Rust monitor; the window is the Python app next to it, WhiteRabbitWindow.exe.
//! Opening the app, the tray's "Open White Rabbit..." and anything meant for the window start that exe.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use wr_core::{info, warn};

/// The window's exe name, next to WhiteRabbit.exe in the installed app.
pub const WINDOW_EXE: &str = "WhiteRabbitWindow.exe";

/// Options the Python app handles: the window, its layout editor and its reports.
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

pub fn start_from(args: &[String]) -> Start {
    if args.iter().any(|a| a == "--background") {
        Start::Background
    } else if args.iter().any(|a| WINDOW_OPTIONS.contains(&a.as_str())) {
        Start::Window(args.to_vec())
    } else {
        Start::MonitorAndWindow
    }
}

pub fn window_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(WINDOW_EXE))
}

static WARNED: AtomicBool = AtomicBool::new(false);

/// Starts the window exe with these arguments. False when it isn't there or won't start.
pub fn open_window(args: &[&str]) -> bool {
    let Some(exe) = window_exe().filter(|p| p.exists()) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn plain_start_runs_both() {
        assert_eq!(start_from(&[]), Start::MonitorAndWindow);
        // Unknown options don't bounce between the two exes
        assert_eq!(start_from(&s(&["--something"])), Start::MonitorAndWindow);
    }

    #[test]
    fn background_wins() {
        assert_eq!(start_from(&s(&["--background", "--window"])), Start::Background);
    }

    #[test]
    fn window_options_go_to_the_window() {
        assert_eq!(start_from(&s(&["--window", "--tab", "audio"])), Start::Window(s(&["--window", "--tab", "audio"])));
        assert_eq!(start_from(&s(&["--diagnose"])), Start::Window(s(&["--diagnose"])));
    }
}
