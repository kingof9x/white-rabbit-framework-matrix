//! Where White Rabbit keeps its files on Windows, the same places as the monitor (`wr-monitor/src/paths.rs`)
//! and the Python app (`led_mon/platform_utils.py`): settings and layout in %APPDATA%\WhiteRabbit, the logs
//! and the control key in %LOCALAPPDATA%\WhiteRabbit. Away from Windows, the same names under
//! $XDG_CONFIG_HOME (settings) and $HOME (logs); a folder whose variable isn't set falls back to the
//! current directory.
//!
//! Every function here only builds a path; nothing is created or read. Environment variables
//! WHITE_RABBIT_CONFIG_DIR, WHITE_RABBIT_LOG_DIR and CONFIG_FILE override the places, which the tests
//! and screenshots use to keep away from a real installation. The window and the monitor must agree on
//! all of these, or the window would not see the monitor's settings or find its control key.

use std::path::PathBuf;

/// The folder name used under %APPDATA% and %LOCALAPPDATA%.
pub const APP_NAME: &str = "WhiteRabbit";
/// Only one window: a second start brings the open one to the front (the installer checks it too).
/// The name of the Windows mutex that marks "a window is open" (see `focus_open_window` in main.rs).
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOW_MUTEX: &str = "WhiteRabbit_window";
/// The layout bundled with the app, used when the user has none yet.
/// Compiled into the exe from the repository's `led_mon/config.yaml`.
pub const BUNDLED_LAYOUT: &str = include_str!("../../../led_mon/config.yaml");

/// The first of these environment variables that is set and not empty, as a folder; "." if none is.
fn env_dir(names: &[&str]) -> PathBuf {
    names
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The settings folder: WHITE_RABBIT_CONFIG_DIR if set, else %APPDATA%\WhiteRabbit on Windows,
/// $XDG_CONFIG_HOME/WhiteRabbit elsewhere.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WHITE_RABBIT_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        env_dir(&["APPDATA"]).join(APP_NAME)
    } else {
        env_dir(&["XDG_CONFIG_HOME"]).join(APP_NAME)
    }
}

/// settings.json, shared with the monitor (see `settings.rs`).
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// The keyboard and macropad settings (wr-keyboard's settings.rs), shared with the monitor.
pub fn keyboard_file() -> PathBuf {
    config_dir().join("keyboard.txt")
}

/// Where the Draw page saves its drawings (`drawings.rs`).
pub fn drawings_dir() -> PathBuf {
    config_dir().join("drawings")
}

/// The log folder: WHITE_RABBIT_LOG_DIR if set, else the first of %LOCALAPPDATA%, %APPDATA%,
/// %USERPROFILE%, $HOME that exists, plus WhiteRabbit.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WHITE_RABBIT_LOG_DIR") {
        return PathBuf::from(dir);
    }
    env_dir(&["LOCALAPPDATA", "APPDATA", "USERPROFILE", "HOME"]).join(APP_NAME)
}

/// This window's own log (the monitor writes a separate one).
pub fn log_file() -> PathBuf {
    log_dir().join("WhiteRabbit-window.log")
}

///
/// Its existence is also how `link.rs` tells quickly that no monitor has run: no file, no request.
/// The control channel's key, written by the monitor when it starts.
pub fn control_key_file() -> PathBuf {
    log_dir().join("control.key")
}

///
/// Same order as the monitor. Only the user's files are looked for here; the caller falls back to
/// `BUNDLED_LAYOUT`.
/// The layout file: CONFIG_FILE, then config-local.yaml, then config.yaml. None: the bundled one is used.
pub fn layout_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CONFIG_FILE") {
        return Some(PathBuf::from(p));
    }
    let dir = config_dir();
    ["config-local.yaml", "config.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// The folder this exe is in (the installed app's folder, with the firmware and VERSION files).
///
/// Falls back to the current folder if Windows can't say where the exe is.
pub fn app_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."))
}
