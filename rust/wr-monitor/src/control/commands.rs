//! The window's requests: a port of `handle_control_request` in `led_mon/led_system_monitor.py`
//! and the commands `white_rabbit/bridge.py` adds to it.
//!
//! Each request is a JSON object with "cmd"; the reply is a JSON object with "ok" (true unless it
//! says otherwise) and, on failure, "error". The commands:
//!
//! | cmd             | does                                                             |
//! |-----------------|------------------------------------------------------------------|
//! | `status`        | the ports in use, the layout file, brightness, the GPUs' names    |
//! | `brightness`    | save "follow_screen" and/or "level" (0-100)                       |
//! | `reload`        | read the layout file again (and end any preview)                  |
//! | `preview`       | show still pictures on the panels for "seconds" (layout editor)   |
//! | `resume`        | end the preview                                                   |
//! | `identify`      | show each panel's name, then its apps' names                      |
//! | `music`         | audio reactivity: "action" on / off / toggle / reload / status    |
//! | `effects`       | Effects pages: reload, try an effect, say a text, learn a port    |
//! | `shortcuts`     | re-register the Ctrl+Alt+Shift shortcuts from settings.json       |
//! | `power`         | the lid / unused-screen settings                                  |
//! | `sensor_source` | where temperatures come from now                                  |
//! | `quit`          | quit (the window installs an update)                              |
//! | `serial`, `serial_many`, `hold`, `release`, `frames` | the panels (`bridge.rs`)   |
//! | `keyboard`      | hand the RGB keyboard to the window and back (`keyboard.rs`)      |
//!
//! Requests come from the control thread, one at a time. They change the main loop's `Shared` state
//! (it acts on it within a tenth of a second), write settings.json with `save_key`, or ask the tray
//! window (`UiRequest`). None of them waits for the main loop. Bad input gives an error reply, never
//! a panic: `fuzzed_requests_always_get_a_reply` below sends thousands of random requests.

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
    /// Register the keyboard's "Open an app" keys again from the keyboard settings.
    AppKeys,
}

/// settings.json key of the tray's "Turn off when lid closes".
const LID_SETTING: &str = "turn_off_with_lid";
/// settings.json key of the tray's "Turn off when laptop screen isn't used".
const SCREEN_UNUSED_SETTING: &str = "turn_off_when_screen_unused";

/// The control channel's request handler (`Handler`), built once in `app::run`.
pub struct Commands {
    /// The main loop's shared flags and values.
    pub shared: Arc<Shared>,
    /// The window's access to the panels' serial ports.
    pub bridge: Arc<Bridge>,
    /// settings.json (a temp file in tests).
    pub settings_file: PathBuf,
    /// Passes a request to the tray window.
    pub ui: Box<dyn Fn(UiRequest) + Send + Sync>,
    /// Windows' side of the window's questions: where temperatures come from, the GPUs.
    pub system: Box<dyn System>,
}

/// What the control commands read from Windows (stand-ins in tests).
pub trait System: Send + Sync {
    /// The "sensor_source" reply: {"choice", "reading", "temps_from", "named", ...}.
    fn sensor_source(&self, reload: bool) -> Value;
    /// The GPUs' names for the layout editor.
    fn gpus(&self) -> Vec<String>;
}

/// A command's answer: the reply object, or an error text that becomes {"ok": false, "error": ...}.
type Reply = Result<Map<String, Value>, String>;

/// A JSON value as an object map; anything else becomes an empty map.
fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// A {"left": rows, "right": rows} picture from the window: each side a list of 9 columns of 34 values.
///
/// Extra columns or rows are ignored, missing ones stay dark, non-numbers count as 0 and values are
/// clamped to 0-255 (LED brightness). None when the side is missing or isn't a list of lists.
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
    /// settings.json as raw JSON, for keys `Settings` doesn't model; empty if missing or broken.
    fn settings_json(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.settings_file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(object)
            .unwrap_or_default()
    }

    /// Writes one top-level key into settings.json, keeping every other key (the window's too).
    fn save(&self, key: &str, value: Value) -> Result<(), String> {
        save_key(&self.settings_file, key, value).map_err(|e| format!("couldn't save settings.json: {e}"))
    }

    /// Reads settings.json again into what the main loop and the tray use.
    fn reload_settings(&self) -> Settings {
        let settings = Settings::load(&self.settings_file);
        *self.shared.settings.lock().unwrap() = settings.clone();
        settings
    }

    /// The brightness part of a reply: whether it follows the screen, and the fixed level (0-100).
    fn brightness_state(settings: &Settings) -> Value {
        json!({"follow_screen": settings.follow_screen_brightness, "level": settings.brightness_level})
    }

    /// "status": the open ports, the layout file in use (null = bundled), brightness and GPU names.
    fn status(&self) -> Reply {
        // Copy the settings out so the lock isn't held while asking Windows for the GPU names.
        let settings = self.shared.settings.lock().unwrap().clone();
        let config_file = crate::paths::layout_file().map(|p| p.display().to_string());
        Ok(object(json!({
            "ports": self.bridge.open_ports(),
            "config_file": config_file,
            "brightness": Self::brightness_state(&settings),
            "gpus": self.system.gpus(),
        })))
    }

    /// "brightness": saves whichever of "follow_screen" / "level" is given, then reports both. The
    /// main loop uses the new values on its next pass.
    fn brightness(&self, message: &Map<String, Value>) -> Reply {
        if let Some(follow) = message.get("follow_screen").filter(|v| !v.is_null()) {
            // The window sends a bool; Python takes bool(value)
            // Run the value through `Settings`' own parsing so it's read the same way as from the file.
            let follow = Settings::from_json(&json!({"follow_screen_brightness": follow}).to_string());
            self.save("follow_screen_brightness", json!(follow.follow_screen_brightness))?;
        }
        if let Some(level) = message.get("level").filter(|v| !v.is_null()) {
            // Settings reads the level the way Python's clamp_level does
            let level = Settings::from_json(&json!({"brightness_level": level}).to_string()).brightness_level;
            self.save("brightness_level", json!(level))?;
        }
        // Re-read the whole file so `Shared::settings` matches what was just saved.
        let settings = self.reload_settings();
        Ok(object(json!({"brightness": Self::brightness_state(&settings)})))
    }

    /// "power": saves the lid and/or "laptop screen unused" settings and asks the tray window to apply
    /// them now. Replies with both settings.
    fn power(&self, message: &Map<String, Value>) -> Reply {
        let mut changed = false;
        for key in [LID_SETTING, SCREEN_UNUSED_SETTING] {
            if let Some(v) = message.get(key) {
                // Both settings are booleans read the same way, so the lid key is borrowed to parse
                // either value with Python's truthiness rules.
                let on = Settings::from_json(&json!({LID_SETTING: v}).to_string()).turn_off_with_lid;
                self.save(key, json!(on))?;
                info!("{key} {}", if on { "on" } else { "off" });
                changed = true;
            }
        }
        let settings = self.reload_settings();
        if changed {
            (self.ui)(UiRequest::Power);
        }
        // Read from the raw JSON with the same default (true) as `Settings::turn_off_when_screen_unused`.
        let screen_unused = self.settings_json().get(SCREEN_UNUSED_SETTING).map(truthy).unwrap_or(true);
        Ok(object(json!({LID_SETTING: settings.turn_off_with_lid, SCREEN_UNUSED_SETTING: screen_unused})))
    }

    /// "shortcuts": re-reads settings.json, asks the tray to re-register the hotkeys, and replies
    /// which letters are on.
    fn shortcuts(&self) -> Reply {
        let settings = self.reload_settings();
        (self.ui)(UiRequest::Shortcuts);
        let enabled: Map<String, Value> = wr_core::settings::SHORTCUT_LETTERS
            .iter()
            .map(|l| (l.to_string(), json!(settings.shortcut_on(*l))))
            .collect();
        Ok(object(json!({"shortcuts": enabled})))
    }

    /// "preview": the layout editor's still pictures, for "seconds" (default 30, at most an hour).
    fn preview(&self, message: &Map<String, Value>) -> Reply {
        let frames = message.get("frames").and_then(Value::as_object);
        let grids =
            [preview_grid(frames.and_then(|f| f.get("left"))), preview_grid(frames.and_then(|f| f.get("right")))];
        let seconds = message.get("seconds").and_then(Value::as_f64).unwrap_or(30.0).clamp(0.0, 3600.0);
        self.shared.show_preview(grids, Duration::from_secs_f64(seconds));
        Ok(Map::new())
    }

    /// "music": action on | off | toggle | reload | status. Music mode takes back the panels lent to
    /// the window. The reply says what music mode hears, for the window's Audio tab.
    fn music(&self, message: &Map<String, Value>) -> Map<String, Value> {
        let on = match message.get("action").and_then(Value::as_str).unwrap_or("status") {
            "on" => Some(true),
            "off" => Some(false),
            // Read-then-set is safe here: requests are handled one at a time on the control thread.
            // (The tray's toggle could race it, but at worst one of two near-simultaneous toggles wins.)
            "toggle" => Some(!self.shared.music_mode.load(Ordering::SeqCst)),
            // The Audio tab changed the look: the main loop restarts the visualizers with it.
            "reload" => {
                self.shared.music_restart.store(true, Ordering::SeqCst);
                None
            }
            _ => None,
        };
        // `on` is None for "status" and "reload", which change nothing here.
        if let Some(on) = on {
            if on {
                // Music draws on every panel, so a panel the window was borrowing is taken back.
                let released = self.bridge.release(None);
                if !released.is_empty() {
                    info!("Audio reactivity took back the panels lent to the window: {}", released.join(", "));
                    self.shared.redraw.store(true, Ordering::SeqCst);
                }
            }
            self.shared.set_music_mode(on);
        }
        let mut reply = object(json!({"music_mode": self.shared.music_mode.load(Ordering::SeqCst)}));
        let hearing = self.shared.hearing.lock().unwrap().clone();
        // "hearing" is only included while at least one visualizer runs.
        if hearing.as_object().is_some_and(|h| !h.is_empty()) {
            reply.insert("hearing".into(), hearing);
        }
        reply
    }

    /// "effects" (the Effects tab): reload the settings, try an effect, show a message, learn which
    /// port a device was plugged into ("learn", fed by the plug watcher's last event), or forget all
    /// learned ports. The reply always has the last plug event and the open ports.
    fn effects(&self, message: &Map<String, Value>) -> Map<String, Value> {
        // Clone the overlay handle so the `effects` lock isn't held while it works. Without an
        // overlay (it is set up at start-up) every action is skipped and only the status is replied.
        if let Some(fx) = self.shared.effects.lock().unwrap().clone() {
            match message.get("action").and_then(Value::as_str).unwrap_or("") {
                "reload" => {
                    let raw = self.settings_json().get("effects").cloned().unwrap_or(Value::Null);
                    fx.effects().configure(wr_core::effects::EffectsSettings::from_json(&raw));
                }
                // Plays the plug-in (or with "event": "out", the unplug) effect at a position.
                "try" => {
                    let position = message.get("position").and_then(Value::as_str);
                    fx.plug(position, message.get("event").and_then(Value::as_str) != Some("out"));
                }
                "say" => fx.effects().say(message.get("text").and_then(Value::as_str).unwrap_or("")),
                "learn" => {
                    let key = message.get("key").and_then(Value::as_str).unwrap_or("");
                    let position = message.get("position").and_then(Value::as_str).unwrap_or("");
                    // Only a key and a valid position ("left", "right-back" ...) are saved.
                    if !key.is_empty() && wr_core::effects::parse_position(position).0.is_some() {
                        self.change_learned_ports(&fx, |ports| {
                            ports.insert(key.into(), Value::String(position.into()));
                        });
                        // If the corrected port is the one just plugged in, show it as learned at once.
                        let mut last = self.shared.last_plug.lock().unwrap();
                        if last.get("key").and_then(Value::as_str) == Some(key) {
                            last["position"] = json!(position);
                            last["how"] = json!("learned");
                        }
                    }
                }
                "forget_ports" => self.change_learned_ports(&fx, Map::clear),
                _ => {}
            }
        }
        let last_plug = self.shared.last_plug.lock().unwrap().clone();
        // Nothing plugged in yet is sent as an empty object, which the window expects.
        let last_plug = if last_plug.is_null() { json!({}) } else { last_plug };
        object(json!({"last_plug": last_plug, "panels": self.bridge.open_ports()}))
    }

    /// Changes the Effects tab's corrected ports in settings.json and uses them straight away.
    fn change_learned_ports(
        &self,
        fx: &wr_panel::overlay::EffectsOverlay,
        change: impl FnOnce(&mut Map<String, Value>),
    ) {
        // Read-modify-write of the "effects" block, so its other settings are kept. A save failure is
        // logged but the change still applies until the next restart.
        let mut effects = self.settings_json().get("effects").cloned().map(object).unwrap_or_default();
        let mut ports = effects.get("learned_ports").cloned().map(object).unwrap_or_default();
        change(&mut ports);
        effects.insert("learned_ports".into(), Value::Object(ports));
        let effects = Value::Object(effects);
        if let Err(e) = self.save("effects", effects.clone()) {
            wr_core::warn!("{e}");
        }
        fx.effects().configure(wr_core::effects::EffectsSettings::from_json(&effects));
    }

    /// "release": gives panels back (all when "ports" isn't a list); the main loop redraws them.
    fn release(&self, message: &Map<String, Value>) -> Reply {
        let released = self.bridge.release(message.get("ports"));
        if !released.is_empty() {
            self.shared.redraw.store(true, Ordering::SeqCst);
        }
        Ok(object(json!({"released": released})))
    }
}

/// Python's bool() of a JSON value, for the settings Settings doesn't keep.
/// Uses the same parsing as `Settings` by borrowing the lid key (see `power`).
fn truthy(v: &Value) -> bool {
    Settings::from_json(&json!({LID_SETTING: v}).to_string()).turn_off_with_lid
}

impl Handler for Commands {
    /// Dispatches one request by its "cmd". An unknown command gets {"ok": false} with an error, not
    /// an Err, so it isn't logged as a failure. Command names are case-sensitive.
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
            "music" => Ok(self.music(message)),
            "effects" => Ok(self.effects(message)),
            "shortcuts" => self.shortcuts(),
            "power" => self.power(message),
            "sensor_source" => {
                // The readings re-read sensor_source and sensor_refresh_seconds from settings.json each time
                let reload = message.get("reload").is_some_and(truthy);
                Ok(object(json!({"sensor_source": self.system.sensor_source(reload)})))
            }
            "quit" => {
                // The window is about to install an update: quit like the tray's Quit, after replying
                (self.ui)(UiRequest::Quit);
                Ok(Map::new())
            }
            "serial" => self.bridge.serial(message),
            "serial_many" => self.bridge.serial_many(message),
            "hold" => Ok(self.bridge.hold(message)),
            "frames" => Ok(self.bridge.frames()),
            "release" => self.release(message),
            // None only before `app::run` starts the keyboard part (or in tests).
            "keyboard" => Ok(match crate::keyboard::job() {
                Some(job) => job.command(message),
                None => object(json!({"ok": false, "error": "The keyboard part isn't running"})),
            }),
            _ => Ok(object(json!({"ok": false, "error": format!("Unknown command {cmd}")}))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Commands over a fresh temp settings.json (one folder per test thread), a fake bridge and a
    /// fake system; tray requests are collected in the returned list.
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
            system: Box::new(FakeSystem),
        };
        (c, asked, dir)
    }

    /// Fixed answers for the Windows-only questions.
    struct FakeSystem;

    impl System for FakeSystem {
        fn sensor_source(&self, reload: bool) -> Value {
            json!({"choice": "auto", "reading": !reload, "named": [["CPU", 50.0]]})
        }
        fn gpus(&self) -> Vec<String> {
            vec!["Radeon 7700S".into()]
        }
    }

    /// sensor_source passes "reload" through and status includes the GPU names.
    #[test]
    fn says_where_temperatures_come_from_and_names_the_gpus() {
        let (c, _, _) = commands();
        let reply = ask(&c, json!({"cmd": "sensor_source"}));
        assert_eq!(reply["sensor_source"]["named"], json!([["CPU", 50.0]]));
        assert_eq!(reply["sensor_source"]["reading"], json!(true));
        assert_eq!(ask(&c, json!({"cmd": "sensor_source", "reload": true}))["sensor_source"]["reading"], json!(false));
        assert_eq!(ask(&c, json!({"cmd": "status"}))["gpus"], json!(["Radeon 7700S"]));
    }

    /// Sends a request through the same path as the control channel (`reply_to`).
    fn ask(c: &Commands, v: Value) -> Value {
        super::super::reply_to(c, v.to_string().as_bytes())
    }

    /// Brightness is saved (level rounded), reported by status, and other settings keys survive.
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

    /// Power and shortcuts are saved/read and each asks the tray window once.
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

    /// Reload, preview, resume, hold/release, quit and music set the right shared flags or requests.
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
        assert_eq!(ask(&c, json!({"cmd": "music", "action": "toggle"})), json!({"ok": true, "music_mode": true}));
        assert_eq!(ask(&c, json!({"cmd": "music", "action": "off"})), json!({"ok": true, "music_mode": false}));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// "learn" saves the port, applies it, and marks the last plug event as learned; "forget_ports"
    /// clears them.
    #[test]
    fn effects_tab_corrects_a_port() {
        let (c, _, dir) = commands();
        let fx = wr_panel::overlay::EffectsOverlay::new(wr_core::effects::EffectsSettings::default());
        *c.shared.effects.lock().unwrap() = Some(fx.clone());
        *c.shared.last_plug.lock().unwrap() = json!({"key": "K", "position": null, "how": "unknown"});
        let reply = ask(&c, json!({"cmd": "effects", "action": "learn", "key": "K", "position": "left-back"}));
        assert_eq!(reply["last_plug"]["position"], json!("left-back"));
        assert_eq!(reply["last_plug"]["how"], json!("learned"));
        assert_eq!(fx.effects().settings.learned_ports.get("K").map(String::as_str), Some("left-back"));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["effects"]["learned_ports"], json!({"K": "left-back"}));
        assert_eq!(saved["music"], json!({"style": "bars"}));
        ask(&c, json!({"cmd": "effects", "action": "forget_ports"}));
        assert!(fx.effects().settings.learned_ports.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Every command, with random and odd fields: always a JSON reply with "ok", never a panic.
    /// ("quit" and "keyboard" are left out: they reach the tray and the keyboard thread.)
    #[test]
    fn fuzzed_requests_always_get_a_reply() {
        let (c, _, dir) = commands();
        let mut seed = 0x5EED_0000_1234_5678u64;
        // xorshift64, a tiny repeatable pseudo-random generator.
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let cmds = [
            "status",
            "brightness",
            "reload",
            "preview",
            "resume",
            "identify",
            "music",
            "effects",
            "shortcuts",
            "power",
            "sensor_source",
            "serial",
            "serial_many",
            "hold",
            "frames",
            "release",
            "",
            "STATUS",
        ];
        let keys = [
            "action",
            "port",
            "ports",
            "write",
            "writes",
            "read",
            "timeout",
            "reset_input",
            "pid",
            "frames",
            "left",
            "right",
            "seconds",
            "level",
            "follow_screen",
            "turn_off_with_lid",
            "key",
            "position",
            "text",
            "event",
            "reload",
        ];
        let values = [
            json!(null),
            json!(true),
            json!(-1),
            json!(1e300),
            json!(-1e300),
            json!(0.5),
            json!(4294967296u64),
            json!(""),
            json!("COM4"),
            json!("zz"),
            json!("32ac08"),
            json!("on"),
            json!("toggle"),
            json!("left-back"),
            json!([]),
            json!(["COM4", 5, null]),
            json!(["32ac", "zz"]),
            json!([[0, 255, -3, 999]]),
            json!([{"port": "COM5", "writes": ["32ac"]}, {"nope": 1}]),
            json!({"left": [[1]], "right": "x"}),
        ];
        // Each round: one command and up to four random fields with odd values.
        for _ in 0..50_000 {
            let mut m = Map::new();
            m.insert("cmd".into(), json!(cmds[(next() % cmds.len() as u64) as usize]));
            for _ in 0..next() % 5 {
                let k = keys[(next() % keys.len() as u64) as usize];
                m.insert(k.into(), values[(next() % values.len() as u64) as usize].clone());
            }
            let reply = ask(&c, Value::Object(m.clone()));
            assert!(reply.get("ok").is_some_and(Value::is_boolean), "{m:?} -> {reply}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
