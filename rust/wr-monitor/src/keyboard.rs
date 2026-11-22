//! The RGB keyboard and macropad while the window is closed. Most looks run on the devices themselves
//! and need nothing from the PC; an app-drawn effect (Lighting page "Drawn by the app") and the audio
//! show are drawn here, so they keep going after the window closes.
//!
//! Only one program can drive the keyboard: while the window is open it holds the keyboard (the
//! "keyboard" control command with "hold": true) and this part stops; when it closes it gives the
//! keyboard back, saying whether the audio show was on. The "Open an app" programs are read from the
//! same settings file (keyboard.txt); the tray window registers their keys.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use wr_core::info;
use wr_keyboard::deck::Choice;
use wr_keyboard::effects::EffectParams;
use wr_keyboard::engine::{self, AudioSettings, Command, EngineHandle};
use wr_keyboard::layout::MacropadSide;
use wr_keyboard::settings::{self, Saved};

/// What the keyboard needs from the PC.
#[derive(Clone, Debug, PartialEq)]
enum Want {
    /// The devices run their own saved effects.
    Nothing,
    Drawn(EffectParams, MacropadSide),
    Audio(AudioSettings),
}

fn want_of(s: &Saved, audio_on: bool) -> Want {
    if audio_on {
        return Want::Audio(AudioSettings {
            look: s.audio_look,
            sensitivity: s.audio_sensitivity,
            microphone: s.audio_mic,
            device: s.audio_device.clone(),
            colors: s.audio_colors,
            keys: s.audio_keys,
        });
    }
    match s.kb.choice {
        Choice::Pc(e) => {
            let side = if s.setup.module_left { MacropadSide::Left } else { MacropadSide::Right };
            Want::Drawn(s.kb.pc_params(e), side)
        }
        _ => Want::Nothing,
    }
}

struct Inner {
    settings: Saved,
    modified: Option<SystemTime>,
    /// The window holding the keyboard (its process id).
    holder: Option<u32>,
    audio_on: bool,
    engine: Option<EngineHandle>,
    running: Want,
}

pub struct KeyboardJob {
    file: PathBuf,
    /// The pretend devices (tests)
    demo: bool,
    inner: Mutex<Inner>,
    /// Called after the settings were read again: the tray window registers the "Open an app" keys.
    on_reload: Box<dyn Fn() + Send + Sync>,
}

static JOB: OnceLock<Arc<KeyboardJob>> = OnceLock::new();

/// The running keyboard part, once the monitor has started it.
pub fn job() -> Option<&'static Arc<KeyboardJob>> {
    JOB.get()
}

fn modified(file: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(file).and_then(|m| m.modified()).ok()
}

impl KeyboardJob {
    pub fn new(file: PathBuf, demo: bool, on_reload: Box<dyn Fn() + Send + Sync>) -> Arc<KeyboardJob> {
        let settings = settings::load(&file);
        wr_keyboard::shortcut::set_enabled(settings.audio_hotkey);
        Arc::new(KeyboardJob {
            inner: Mutex::new(Inner {
                settings,
                modified: modified(&file),
                holder: None,
                audio_on: false,
                engine: None,
                running: Want::Nothing,
            }),
            file,
            demo,
            on_reload,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts the keyboard part: checks the settings file and the window every second.
    pub fn start(self: &Arc<Self>, alive: fn(u32) -> bool) {
        let _ = JOB.set(self.clone());
        self.apply(&mut self.lock());
        let me = self.clone();
        let started = std::thread::Builder::new().name("keyboard".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            me.check(alive);
        });
        if let Err(e) = started {
            wr_core::warn!("Can't start the keyboard thread: {e}");
        }
    }

    /// Takes the keyboard back from a window that closed without giving it back, and follows changes
    /// to the settings file.
    pub fn check(&self, alive: fn(u32) -> bool) {
        let mut inner = self.lock();
        if let Some(pid) = inner.holder {
            if !alive(pid) {
                info!("The window closed without giving the keyboard back; taking it back");
                inner.holder = None;
            }
        }
        if modified(&self.file) != inner.modified {
            self.reload(&mut inner);
        }
        self.apply(&mut inner);
    }

    fn reload(&self, inner: &mut Inner) {
        inner.settings = settings::load(&self.file);
        inner.modified = modified(&self.file);
        wr_keyboard::shortcut::set_enabled(inner.settings.audio_hotkey);
        (self.on_reload)();
    }

    /// Starts, changes or stops drawing to match the settings, unless the window holds the keyboard.
    fn apply(&self, inner: &mut Inner) {
        let want = if inner.holder.is_some() { Want::Nothing } else { want_of(&inner.settings, inner.audio_on) };
        if want == inner.running {
            return;
        }
        info!(
            "Keyboard: {}",
            match &want {
                Want::Nothing => "the devices run their own effects".to_string(),
                Want::Drawn(p, _) => format!("drawing {:?}", p.effect),
                Want::Audio(a) => format!("audio show {:?}", a.look),
            }
        );
        match &want {
            // Dropping the engine hands the lights back to the devices' saved effects
            Want::Nothing => inner.engine = None,
            Want::Drawn(params, side) => {
                let engine = inner.engine.get_or_insert_with(|| engine::start(|| {}, self.demo));
                engine.send(Command::SetAudio(None));
                engine.send(Command::SetSide(*side));
                engine.send(Command::SetLinked(Some(*params)));
            }
            Want::Audio(set) => {
                let engine = inner.engine.get_or_insert_with(|| engine::start(|| {}, self.demo));
                engine.send(Command::SetAudio(Some(set.clone())));
            }
        }
        inner.running = want;
    }

    /// The window's "keyboard" command:
    /// - "hold": true, "pid": the window's process: stop drawing, the window drives the keyboard now.
    ///   The reply says whether the audio show was on, so the window carries on with it.
    /// - "hold": false, "audio": on/off: the window closed; carry on with what it showed.
    /// - neither: the settings changed (an "Open an app" key, the shortcut); read them again.
    pub fn command(&self, message: &Map<String, Value>) -> Map<String, Value> {
        let mut inner = self.lock();
        match message.get("hold").and_then(Value::as_bool) {
            Some(true) => {
                inner.holder = message.get("pid").and_then(Value::as_u64).map(|p| p as u32).or(Some(0));
                // Stopped before replying, so the window finds the devices free
                self.apply(&mut inner);
            }
            Some(false) => {
                inner.holder = None;
                inner.audio_on = message.get("audio").and_then(Value::as_bool).unwrap_or(false);
                self.reload(&mut inner);
                self.apply(&mut inner);
            }
            None => {
                self.reload(&mut inner);
                self.apply(&mut inner);
            }
        }
        let audio = inner.audio_on;
        match json!({"ok": true, "audio": audio}) {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    /// Ctrl+Alt+Shift+M with the window closed: the audio show on or off.
    pub fn toggle_audio(&self) {
        let mut inner = self.lock();
        if inner.holder.is_some() {
            return; // the window answers it
        }
        inner.audio_on = !inner.audio_on;
        self.apply(&mut inner);
    }

    /// The "Open an app" programs, one per key (Ctrl+Shift+Alt+F13 onwards; "" = not set).
    pub fn apps(&self) -> Vec<String> {
        self.lock().settings.apps.clone()
    }

    /// Hands the lights back to the devices' own effects (Quit).
    pub fn stop(&self) {
        let mut inner = self.lock();
        inner.engine = None;
        inner.running = Want::Nothing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wr_keyboard::effects::LinkedEffect;

    fn job(name: &str, text: Option<&str>) -> (Arc<KeyboardJob>, PathBuf, Arc<AtomicUsize>) {
        let dir = std::env::temp_dir().join(format!("wr-kb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keyboard.txt");
        if let Some(t) = text {
            std::fs::write(&file, t).unwrap();
        }
        let reloads = Arc::new(AtomicUsize::new(0));
        let r = reloads.clone();
        let job = KeyboardJob::new(
            file.clone(),
            true,
            Box::new(move || {
                r.fetch_add(1, Ordering::SeqCst);
            }),
        );
        (job, file, reloads)
    }

    fn drawn() -> String {
        let mut s = Saved::default();
        s.kb.choice = Choice::Pc(LinkedEffect::Ripple);
        settings::to_text(&s)
    }

    fn ask(job: &KeyboardJob, v: Value) -> Map<String, Value> {
        job.command(v.as_object().unwrap())
    }

    #[test]
    fn device_effects_need_nothing_from_the_pc() {
        let (job, _, _) = job("own", Some(&settings::to_text(&Saved::default())));
        job.apply(&mut job.lock());
        assert!(job.lock().engine.is_none());
        assert_eq!(job.lock().running, Want::Nothing);
    }

    #[test]
    fn an_app_drawn_effect_stops_while_the_window_holds_the_keyboard() {
        let (job, _, _) = job("drawn", Some(&drawn()));
        job.apply(&mut job.lock());
        assert!(matches!(job.lock().running, Want::Drawn(..)));
        assert!(job.lock().engine.is_some());

        let reply = ask(&job, json!({"cmd": "keyboard", "hold": true, "pid": 42}));
        assert_eq!(reply["audio"], json!(false));
        assert!(job.lock().engine.is_none());
        // The shortcut is the window's while it is open
        job.toggle_audio();
        assert!(!job.lock().audio_on);

        ask(&job, json!({"cmd": "keyboard", "hold": false, "audio": false}));
        assert!(matches!(job.lock().running, Want::Drawn(..)));
    }

    #[test]
    fn the_audio_show_carries_on_after_the_window_closes() {
        let (job, _, _) = job("audio", None);
        ask(&job, json!({"cmd": "keyboard", "hold": true, "pid": 42}));
        ask(&job, json!({"cmd": "keyboard", "hold": false, "audio": true}));
        assert!(matches!(job.lock().running, Want::Audio(_)));
        // and the window picks it up again when it opens
        let reply = ask(&job, json!({"cmd": "keyboard", "hold": true, "pid": 43}));
        assert_eq!(reply["audio"], json!(true));
        // Ctrl+Alt+Shift+M with the window closed
        ask(&job, json!({"cmd": "keyboard", "hold": false, "audio": true}));
        job.toggle_audio();
        assert_eq!(job.lock().running, Want::Nothing);
    }

    #[test]
    fn a_window_that_died_gives_the_keyboard_back() {
        let (job, _, _) = job("died", Some(&drawn()));
        ask(&job, json!({"cmd": "keyboard", "hold": true, "pid": 42}));
        job.check(|_| true);
        assert_eq!(job.lock().running, Want::Nothing);
        job.check(|_| false);
        assert!(matches!(job.lock().running, Want::Drawn(..)));
    }

    #[test]
    fn changed_settings_are_read_again() {
        let (job, file, reloads) = job("reload", None);
        assert_eq!(job.apps().iter().filter(|a| !a.is_empty()).count(), 0);
        let mut s = Saved::default();
        s.apps[0] = "C:\\Windows\\notepad.exe".into();
        std::fs::write(&file, settings::to_text(&s)).unwrap();
        ask(&job, json!({"cmd": "keyboard"}));
        assert_eq!(job.apps()[0], "C:\\Windows\\notepad.exe");
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
    }
}
