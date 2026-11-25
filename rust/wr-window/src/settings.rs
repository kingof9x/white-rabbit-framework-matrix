//! settings.json, shared with the monitor: read it whole, change a key, write it whole, so the keys the
//! window doesn't know about are kept (`led_mon/app_settings.py`).
//!
//! The file is one JSON object at `paths::settings_file()`. Both programs read and write it; the
//! monitor checks it about once a second and applies changes, which is how the window changes the
//! monitor's behaviour (theme, effects, panel names and so on). There is no cache: every `get` reads
//! the file again, so the window always sees what the monitor last wrote. There is no lock between the
//! two programs either: if both change a key in the same instant, the later write wins. Writes go
//! through a temporary file and a rename so a reader never sees half a file.
//!
//! These functions do file I/O but are small and fast; they are called from the UI thread as well as
//! from workers.

use serde_json::{Map, Value};
use std::path::Path;

use crate::paths;

/// Reads a settings file. A missing or unreadable file gives empty settings silently; a file that is
/// there but not a JSON object is logged and also gives empty settings.
pub fn load_from(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => m,
            _ => {
                wr_core::warn!("settings.json isn't a JSON object; starting from empty settings");
                Map::new()
            }
        },
        Err(_) => Map::new(),
    }
}

/// Reads the shared settings.json.
pub fn load() -> Map<String, Value> {
    load_from(&paths::settings_file())
}

/// Writes all the settings, pretty-printed, creating the folder if needed. Err if the folder can't be
/// made or the file can't be written or swapped in.
pub fn save_to(path: &Path, settings: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(settings.clone()))?;
    // Write a new file and swap it in, so the monitor never reads half a file
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

/// Writes the shared settings.json.
pub fn save(settings: &Map<String, Value>) -> std::io::Result<()> {
    save_to(&paths::settings_file(), settings)
}

///
/// Reads the file fresh first, so keys the monitor changed meanwhile are kept.
/// Changes one key and saves. Logs a failure and returns false.
pub fn set(key: &str, value: Value) -> bool {
    let mut settings = load();
    settings.insert(key.to_string(), value);
    match save(&settings) {
        Ok(()) => true,
        Err(e) => {
            wr_core::warn!("Couldn't save {key} in settings.json: {e}");
            false
        }
    }
}

/// One key's value from settings.json, read fresh; None if it isn't there.
pub fn get(key: &str) -> Option<Value> {
    load().remove(key)
}

/// One key's value if it is a string; None if missing or another type.
pub fn get_str(key: &str) -> Option<String> {
    get(key).and_then(|v| v.as_str().map(String::from))
}

/// The sides that can have a name; names for anything else are dropped.
pub const PANEL_SIDES: [&str; 2] = ["left", "right"];
/// Longest panel name, in characters (the panels show it scrolling, so it is kept short).
pub const PANEL_NAME_MAX: usize = 20;

/// The names the user gave the left and right panels, cleaned as Python's clean_panel_names does.
pub fn panel_names() -> Map<String, Value> {
    match get("panel_names") {
        Some(Value::Object(m)) => clean_panel_names(&m),
        _ => Map::new(),
    }
}

/// Keeps only "left" and "right" names that are strings, squeezes runs of spaces to one space, trims
/// the ends and cuts them to `PANEL_NAME_MAX` characters; an empty result is dropped.
pub fn clean_panel_names(names: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for side in PANEL_SIDES {
        if let Some(name) = names.get(side).and_then(Value::as_str) {
            let name: String =
                name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PANEL_NAME_MAX).collect();
            if !name.is_empty() {
                out.insert(side.to_string(), Value::String(name));
            }
        }
    }
    out
}
