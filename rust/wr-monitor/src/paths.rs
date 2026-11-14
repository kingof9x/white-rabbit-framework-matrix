//! Where White Rabbit keeps its files on Windows, the same places as the Python monitor
//! (`led_mon/platform_utils.py`): settings and layout in %APPDATA%\WhiteRabbit, the log in
//! %LOCALAPPDATA%\WhiteRabbit.

use std::path::PathBuf;

pub const APP_NAME: &str = "WhiteRabbit";
/// The Python app's single-instance mutex: only one monitor may drive the panels.
pub const SINGLE_INSTANCE_MUTEX: &str = "WhiteRabbit_single_instance";
/// The layout bundled with the app, used when the user has none yet.
pub const BUNDLED_LAYOUT: &str = include_str!("../../../led_mon/config.yaml");

fn env_dir(name: &str) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join(APP_NAME)
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn log_file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"));
    base.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(APP_NAME).join("WhiteRabbit-rust-test.log")
}

/// The layout file: CONFIG_FILE, then config-local.yaml, then config.yaml. None: use the bundled one.
pub fn layout_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CONFIG_FILE") {
        return Some(PathBuf::from(p));
    }
    let dir = config_dir();
    ["config-local.yaml", "config.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
}
