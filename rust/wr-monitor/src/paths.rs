//! Where White Rabbit keeps its files on Windows, the same places as the Python monitor
//! (`led_mon/platform_utils.py`): settings and layout in %APPDATA%\WhiteRabbit, the log in
//! %LOCALAPPDATA%\WhiteRabbit.
//!
//! These functions only build paths; none of them creates a folder or a file. They are called from
//! many threads (start-up, main loop, control channel), which is fine because they only read
//! environment variables. The window (wr-window) uses the same folders, so both programs see the same
//! settings.json and layout.

use std::path::PathBuf;

/// The folder name used under %APPDATA% and %LOCALAPPDATA%.
pub const APP_NAME: &str = "WhiteRabbit";
/// The Python app's single-instance mutex: only one monitor may drive the panels.
pub const SINGLE_INSTANCE_MUTEX: &str = "WhiteRabbit_single_instance";
/// The layout bundled with the app, used when the user has none yet.
pub const BUNDLED_LAYOUT: &str = include_str!("../../../led_mon/config.yaml");

/// The folder an environment variable names, or the current folder when the variable is missing
/// (so a broken environment writes next to the exe instead of failing).
fn env_dir(name: &str) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// %APPDATA%\WhiteRabbit: the roaming settings folder (settings.json, layout, keyboard.txt).
pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join(APP_NAME)
}

/// settings.json: the monitor's and the window's shared settings (brightness, effects, apps' options).
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// The keyboard and macropad settings, written by the window (wr-keyboard's settings.rs).
pub fn keyboard_file() -> PathBuf {
    config_dir().join("keyboard.txt")
}

/// %LOCALAPPDATA%\WhiteRabbit: Python's `user_log_dir` (an empty variable counts as unset there).
///
/// Tries LOCALAPPDATA, then APPDATA, USERPROFILE and HOME, and falls back to the current folder.
pub fn log_dir() -> PathBuf {
    let base = ["LOCALAPPDATA", "APPDATA", "USERPROFILE", "HOME"]
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|s| !s.is_empty()));
    base.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(APP_NAME)
}

/// The monitor's log file, written by `wr_core::log` from every thread.
pub fn log_file() -> PathBuf {
    log_dir().join("WhiteRabbit-monitor.log")
}

/// The control channel's key, which the window reads to talk to the monitor (`led_mon/control.py`).
///
/// Kept in the local (not roaming) folder so the secret does not travel to other computers.
pub fn control_key_file() -> PathBuf {
    log_dir().join("control.key")
}

/// The layout file: CONFIG_FILE, then config-local.yaml, then config.yaml. None: use the bundled one.
///
/// The CONFIG_FILE environment variable wins even if that file does not exist (useful for testing a
/// layout). Otherwise the first of the two files that exists is used.
pub fn layout_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CONFIG_FILE") {
        return Some(PathBuf::from(p));
    }
    let dir = config_dir();
    ["config-local.yaml", "config.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
}
