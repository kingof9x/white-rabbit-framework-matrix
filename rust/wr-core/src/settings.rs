//! `settings.json` in `%APPDATA%\WhiteRabbit`, shared with the Python window: brightness, the lid,
//! which keyboard shortcuts are on. Only the keys the monitor reads; everything else is left alone.

use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub follow_screen_brightness: bool,
    /// 0-100, used when not following the screen.
    pub brightness_level: u8,
    pub turn_off_with_lid: bool,
    /// The letters of the keyboard shortcuts that are turned off (Ctrl+Alt+Shift + letter).
    pub shortcuts_off: Vec<char>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            follow_screen_brightness: true,
            brightness_level: 70,
            turn_off_with_lid: true,
            shortcuts_off: vec![],
        }
    }
}

pub const SHORTCUT_LETTERS: [char; 5] = ['i', 'n', 'f', 'u', 'm'];

/// Python's `clamp_level`: a number from 0 to 100, or the default when it isn't a number.
fn clamp_level(v: &Value) -> u8 {
    let n = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    };
    match n {
        Some(n) if n.is_finite() => n.round_ties_even().clamp(0.0, 100.0) as u8,
        _ => 70,
    }
}

impl Settings {
    pub fn from_json(text: &str) -> Settings {
        let mut s = Settings::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else { return s };
        if let Some(v) = map.get("follow_screen_brightness") {
            s.follow_screen_brightness = truthy(v);
        }
        if let Some(v) = map.get("brightness_level") {
            s.brightness_level = clamp_level(v);
        }
        if let Some(v) = map.get("turn_off_with_lid") {
            s.turn_off_with_lid = truthy(v);
        }
        if let Some(Value::Object(sc)) = map.get("shortcuts") {
            s.shortcuts_off = SHORTCUT_LETTERS
                .iter()
                .copied()
                .filter(|l| sc.get(&l.to_string()).map(|v| !truthy(v)).unwrap_or(false))
                .collect();
        }
        s
    }

    /// Missing or unreadable file: the defaults, as in Python.
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::from_json(&t)).unwrap_or_default()
    }

    pub fn shortcut_on(&self, letter: char) -> bool {
        !self.shortcuts_off.contains(&letter)
    }
}

/// Python's `bool(value)` for JSON values.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Changes one key in settings.json and keeps every other key as it is (the window writes other keys).
pub fn save_key(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| Value::Object(Default::default()));
    root.as_object_mut().unwrap().insert(key.to_string(), value);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root).unwrap())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_keys_the_monitor_uses() {
        let s = Settings::from_json(
            r#"{"follow_screen_brightness": false, "brightness_level": "42.5",
            "turn_off_with_lid": 0, "shortcuts": {"m": false, "i": true}, "music": {"style": "bars"}}"#,
        );
        assert!(!s.follow_screen_brightness);
        assert_eq!(s.brightness_level, 42);
        assert!(!s.turn_off_with_lid);
        assert!(!s.shortcut_on('m'));
        assert!(s.shortcut_on('i') && s.shortcut_on('n'));
        assert_eq!(Settings::from_json("not json"), Settings::default());
        assert_eq!(Settings::from_json(r#"{"brightness_level": 900}"#).brightness_level, 100);
    }

    #[test]
    fn saving_one_key_keeps_the_others() {
        let dir = std::env::temp_dir().join(format!("wr-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"music": {"style": "bars"}, "turn_off_with_lid": true}"#).unwrap();
        save_key(&path, "turn_off_with_lid", Value::Bool(false)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["music"]["style"], "bars");
        assert_eq!(v["turn_off_with_lid"], false);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
