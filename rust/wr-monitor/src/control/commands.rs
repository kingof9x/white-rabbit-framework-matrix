//! The window's requests: a port of `handle_control_request` in `led_mon/led_system_monitor.py`
//! and the commands `white_rabbit/bridge.py` adds to it.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use wr_core::settings::{save_key, Settings};
use wr_core::{info, Grid, HEIGHT, WIDTH};

use super::bridge::Bridge;
use super::Handler;
use crate::monitor::Shared;

/// What only the tray window can do, asked from the control thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiRequest {
    /// Register the keyboard shortcuts again from the settings.
    Shortcuts,
    /// Turn the panels off or on again for the lid setting.
    Power,
    /// Quit like the tray's Quit.
    Quit,
}

const LID_SETTING: &str = "turn_off_with_lid";
const SCREEN_UNUSED_SETTING: &str = "turn_off_when_screen_unused";

pub struct Commands {
    pub shared: Arc<Shared>,
    pub bridge: Arc<Bridge>,
    pub settings_file: PathBuf,
    /// Passes a request to the tray window.
    pub ui: Box<dyn Fn(UiRequest) + Send + Sync>,
}

type Reply = Result<Map<String, Value>, String>;

fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// A {"left": rows, "right": rows} picture from the window: each side a list of 9 columns of 34 values.
fn preview_grid(value: Option<&Value>) -> Option<Grid> {
    let columns = value?.as_array()?;
    let mut grid = Grid::new();
    for (x, column) in columns.iter().take(WIDTH).enumerate() {
        for (y, v) in column.as_array()?.iter().take(HEIGHT).enumerate() {
            grid.set(x, y, v.as_f64().unwrap_or(0.0).clamp(0.0, 255.0) as i32);
        }
    }
    Some(grid)
}

impl Commands {
    fn settings_json(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.settings_file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(object)
            .unwrap_or_default()
    }

    fn save(&self, key: &str, value: Value) -> Result<(), String> {
        save_key(&self.settings_file, key, value).map_err(|e| format!("couldn't save settings.json: {e}"))
    }

    /// Reads settings.json again into what the main loop and the tray use.
    fn reload_settings(&self) -> Settings {
        let settings = Settings::load(&self.settings_file);
        *self.shared.settings.lock().unwrap() = settings.clone();
        settings
    }

    fn brightness_state(settings: &Settings) -> Value {
        json!({"follow_screen": settings.follow_screen_brightness, "level": settings.brightness_level})
    }

    fn status(&self) -> Reply {
        let settings = self.shared.settings.lock().unwrap().clone();
        let config_file = crate::paths::layout_file().map(|p| p.display().to_string());
        // The Python monitor also says where temperatures come from (sensor_source); the window copes
        // without it
        Ok(object(json!({
            "ports": self.bridge.open_ports(),
            "config_file": config_file,
            "brightness": Self::brightness_state(&settings),
        })))
    }

    fn brightness(&self, message: &Map<String, Value>) -> Reply {
        if let Some(follow) = message.get("follow_screen").filter(|v| !v.is_null()) {
            // The window sends a bool; Python takes bool(value)
            let follow = Settings::from_json(&json!({"follow_screen_brightness": follow}).to_string());
            self.save("follow_screen_brightness", json!(follow.follow_screen_brightness))?;
        }
        if let Some(level) = message.get("level").filter(|v| !v.is_null()) {
            // Settings reads the level the way Python's clamp_level does
            let level = Settings::from_json(&json!({"brightness_level": level}).to_string()).brightness_level;
            self.save("brightness_level", json!(level))?;
        }
        let settings = self.reload_settings();
        Ok(object(json!({"brightness": Self::brightness_state(&settings)})))
    }

    fn power(&self, message: &Map<String, Value>) -> Reply {
        let mut changed = false;
        for key in [LID_SETTING, SCREEN_UNUSED_SETTING] {
            if let Some(v) = message.get(key) {
                let on = Settings::from_json(&json!({LID_SETTING: v}).to_string()).turn_off_with_lid;
                self.save(key, json!(on))?;
                info!("{key} {}", if on { "on" } else { "off" });
                changed = true;
            }
        }
        if message.contains_key(SCREEN_UNUSED_SETTING) {
            info!("TODO: the Rust monitor doesn't turn the panels off when the laptop screen isn't used yet");
        }
        let settings = self.reload_settings();
        if changed {
            (self.ui)(UiRequest::Power);
        }
        let screen_unused = self.settings_json().get(SCREEN_UNUSED_SETTING).map(truthy).unwrap_or(true);
        Ok(object(json!({LID_SETTING: settings.turn_off_with_lid, SCREEN_UNUSED_SETTING: screen_unused})))
    }

    fn shortcuts(&self) -> Reply {
        let settings = self.reload_settings();
        (self.ui)(UiRequest::Shortcuts);
        let enabled: Map<String, Value> = wr_core::settings::SHORTCUT_LETTERS
            .iter()
            .map(|l| (l.to_string(), json!(settings.shortcut_on(*l))))
            .collect();
        Ok(object(json!({"shortcuts": enabled})))
    }

    fn preview(&self, message: &Map<String, Value>) -> Reply {
        let frames = message.get("frames").and_then(Value::as_object);
        let grids =
            [preview_grid(frames.and_then(|f| f.get("left"))), preview_grid(frames.and_then(|f| f.get("right")))];
        let seconds = message.get("seconds").and_then(Value::as_f64).unwrap_or(30.0).clamp(0.0, 3600.0);
        self.shared.show_preview(grids, Duration::from_secs_f64(seconds));
        Ok(Map::new())
    }

    fn release(&self, message: &Map<String, Value>) -> Reply {
        let released = self.bridge.release(message.get("ports"));
        if !released.is_empty() {
            self.shared.redraw.store(true, Ordering::SeqCst);
        }
        Ok(object(json!({"released": released})))
    }
}

/// Python's bool() of a JSON value, for the settings Settings doesn't keep.
fn truthy(v: &Value) -> bool {
    Settings::from_json(&json!({LID_SETTING: v}).to_string()).turn_off_with_lid
}

impl Handler for Commands {
    fn handle(&self, message: &Map<String, Value>) -> Reply {
        let cmd = message.get("cmd").and_then(Value::as_str).unwrap_or("");
        match cmd {
            "status" => self.status(),
            "brightness" => self.brightness(message),
            "reload" => {
                self.shared.clear_preview();
                self.shared.reload.store(true, Ordering::SeqCst);
                Ok(Map::new())
            }
            "preview" => self.preview(message),
            "resume" => {
                self.shared.clear_preview();
                Ok(Map::new())
            }
            "identify" => {
                self.shared.identify(3.0);
                Ok(Map::new())
            }
            "music" => {
                info!("TODO: music mode isn't in the Rust monitor yet (music {:?})", message.get("action"));
                Ok(object(json!({"music_mode": false})))
            }
            "shortcuts" => self.shortcuts(),
            "power" => self.power(message),
            "sensor_source" => {
                info!("TODO: the Rust monitor has no choice of sensor source yet");
                Ok(Map::new())
            }
            "quit" => {
                // The window is about to install an update: quit like the tray's Quit, after replying
                (self.ui)(UiRequest::Quit);
                Ok(Map::new())
            }
            "serial" => self.bridge.serial(message),
            "serial_many" => self.bridge.serial_many(message),
            "hold" => Ok(self.bridge.hold(message)),
            "release" => self.release(message),
            _ => Ok(object(json!({"ok": false, "error": format!("Unknown command {cmd}")}))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn commands() -> (Commands, Arc<Mutex<Vec<UiRequest>>>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("wr-commands-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"music": {"style": "bars"}}"#).unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = asked.clone();
        let (_, bridge) = super::super::bridge::tests::bridge();
        let c = Commands {
            shared: Arc::new(Shared::default()),
            bridge: Arc::new(bridge),
            settings_file: file,
            ui: Box::new(move |r| a.lock().unwrap().push(r)),
        };
        (c, asked, dir)
    }

    fn ask(c: &Commands, v: Value) -> Value {
        super::super::reply_to(c, v.to_string().as_bytes())
    }

    #[test]
    fn brightness_is_saved_and_reported() {
        let (c, _, dir) = commands();
        let reply = ask(&c, json!({"cmd": "brightness", "follow_screen": false, "level": 42.4}));
        assert_eq!(reply, json!({"ok": true, "brightness": {"follow_screen": false, "level": 42}}));
        let status = ask(&c, json!({"cmd": "status"}));
        assert_eq!(status["brightness"], json!({"follow_screen": false, "level": 42}));
        assert_eq!(status["ports"], json!([]));
        // Other keys in settings.json are kept
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["music"]["style"], "bars");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn power_and_shortcuts_reach_the_tray() {
        let (c, asked, dir) = commands();
        assert_eq!(
            ask(&c, json!({"cmd": "power"})),
            json!({"ok": true, "turn_off_with_lid": true, "turn_off_when_screen_unused": true})
        );
        assert_eq!(
            ask(&c, json!({"cmd": "power", "turn_off_with_lid": false})),
            json!({"ok": true, "turn_off_with_lid": false, "turn_off_when_screen_unused": true})
        );
        assert!(!c.shared.settings.lock().unwrap().turn_off_with_lid);
        std::fs::write(dir.join("settings.json"), r#"{"shortcuts": {"m": false}}"#).unwrap();
        let reply = ask(&c, json!({"cmd": "shortcuts"}));
        assert_eq!(reply["shortcuts"]["m"], json!(false));
        assert_eq!(reply["shortcuts"]["i"], json!(true));
        assert_eq!(*asked.lock().unwrap(), vec![UiRequest::Power, UiRequest::Shortcuts]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn flags_for_the_main_loop() {
        let (c, asked, dir) = commands();
        ask(&c, json!({"cmd": "reload"}));
        assert!(c.shared.reload.load(Ordering::SeqCst));
        ask(&c, json!({"cmd": "preview", "frames": {"left": [[0, 255]]}, "seconds": 5}));
        assert!(c.shared.preview_frame(0).is_some_and(|g| g.get(0, 1) == 255));
        assert!(c.shared.preview_frame(1).is_none());
        ask(&c, json!({"cmd": "resume"}));
        assert!(c.shared.preview_frame(0).is_none());
        ask(&c, json!({"cmd": "hold", "ports": ["COM4"], "pid": 1}));
        assert_eq!(ask(&c, json!({"cmd": "release", "ports": null})), json!({"ok": true, "released": ["com4"]}));
        assert!(c.shared.redraw.load(Ordering::SeqCst));
        assert_eq!(ask(&c, json!({"cmd": "quit"}))["ok"], json!(true));
        assert_eq!(*asked.lock().unwrap(), vec![UiRequest::Quit]);
        assert_eq!(ask(&c, json!({"cmd": "nope"}))["ok"], json!(false));
        assert_eq!(ask(&c, json!({"cmd": "music", "action": "status"})), json!({"ok": true, "music_mode": false}));
        let _ = std::fs::remove_dir_all(dir);
    }
}
