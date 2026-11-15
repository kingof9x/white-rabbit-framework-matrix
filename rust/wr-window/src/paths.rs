//! Where White Rabbit keeps its files on Windows, the same places as the monitor (`wr-monitor/src/paths.rs`)
//! and the Python app (`led_mon/platform_utils.py`): settings and layout in %APPDATA%\WhiteRabbit, the logs
//! and the control key in %LOCALAPPDATA%\WhiteRabbit. Away from Windows, the same names under $HOME.

use std::path::PathBuf;

pub const APP_NAME: &str = "WhiteRabbit";
/// Only one window: a second start brings the open one to the front (the installer checks it too).
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOW_MUTEX: &str = "WhiteRabbit_window";
/// The layout bundled with the app, used when the user has none yet.
pub const BUNDLED_LAYOUT: &str = include_str!("../../../led_mon/config.yaml");

fn env_dir(names: &[&str]) -> PathBuf {
    names
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

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

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn drawings_dir() -> PathBuf {
    config_dir().join("drawings")
}

pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WHITE_RABBIT_LOG_DIR") {
        return PathBuf::from(dir);
    }
    env_dir(&["LOCALAPPDATA", "APPDATA", "USERPROFILE", "HOME"]).join(APP_NAME)
}

pub fn log_file() -> PathBuf {
    log_dir().join("WhiteRabbit-window.log")
}

/// The control channel's key, written by the monitor when it starts.
pub fn control_key_file() -> PathBuf {
    log_dir().join("control.key")
}

/// The layout file: CONFIG_FILE, then config-local.yaml, then config.yaml. None: the bundled one is used.
pub fn layout_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CONFIG_FILE") {
        return Some(PathBuf::from(p));
    }
    let dir = config_dir();
    ["config-local.yaml", "config.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// The folder this exe is in (the installed app's folder, with the firmware and VERSION files).
pub fn app_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."))
}
