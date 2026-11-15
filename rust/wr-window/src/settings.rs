//! settings.json, shared with the monitor: read it whole, change a key, write it whole, so the keys the
//! window doesn't know about are kept (`led_mon/app_settings.py`).

use serde_json::{Map, Value};
use std::path::Path;

use crate::paths;

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

pub fn load() -> Map<String, Value> {
    load_from(&paths::settings_file())
}

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

pub fn save(settings: &Map<String, Value>) -> std::io::Result<()> {
    save_to(&paths::settings_file(), settings)
}

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

pub fn get(key: &str) -> Option<Value> {
    load().remove(key)
}

pub fn get_str(key: &str) -> Option<String> {
    get(key).and_then(|v| v.as_str().map(String::from))
}

pub const PANEL_SIDES: [&str; 2] = ["left", "right"];
pub const PANEL_NAME_LETTERS: usize = 5;
pub const PANEL_NAME_MAX: usize = 20;

/// The names the user gave the left and right panels, cleaned as Python's clean_panel_names does.
pub fn panel_names() -> Map<String, Value> {
    match get("panel_names") {
        Some(Value::Object(m)) => clean_panel_names(&m),
        _ => Map::new(),
    }
}

pub fn clean_panel_names(names: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for side in PANEL_SIDES {
        if let Some(name) = names.get(side).and_then(Value::as_str) {
            let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PANEL_NAME_MAX).collect();
            if !name.is_empty() {
                out.insert(side.to_string(), Value::String(name));
            }
        }
    }
    out
}
