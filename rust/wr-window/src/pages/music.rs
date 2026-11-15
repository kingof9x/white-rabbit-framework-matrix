//! Music (`music.py`): music mode, and the look and sound input the audio visualizer uses for it.
//! Changes are saved straight away ("music" in settings.json) and the monitor picks them up while
//! music mode is on. The preview plays the real sound through wr-music, the code the panels use, so it
//! doubles as a check that the microphone is heard.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use wr_music::capture::{self, Capture, DEFAULT_MICROPHONE, FFT_SIZE};
use wr_music::settings::{FallSpeed, InputMode, NO_BAR_OPTIONS};
use wr_music::{MusicPanel, Settings, Stereo, STYLES};

use crate::app::Ctx;
use crate::{link, preview, settings, App, AppWindow, MusicPage};

const FRAME_SECONDS: f64 = 0.05;
const POLL_SECONDS: f64 = 2.0;

/// The settings as settings.json keeps them (Python's `looks.normalize` output).
pub fn to_json(s: &Settings) -> Value {
    let mut m = Map::new();
    m.insert("input-mode".into(), json!(s.input_mode.as_str()));
    m.insert("input-device".into(), json!(s.input_device));
    m.insert("style".into(), json!(s.style));
    m.insert("span-panels".into(), json!(s.span_panels));
    m.insert("peak-dots".into(), json!(s.peak_dots));
    m.insert("shaded".into(), json!(s.shaded));
    m.insert("flip".into(), json!(s.flip));
    m.insert(
        "fall-speed".into(),
        json!(match s.fall_speed {
            FallSpeed::Slow => "slow",
            FallSpeed::Normal => "normal",
            FallSpeed::Fast => "fast",
        }),
    );
    m.insert("sensitivity".into(), json!(s.sensitivity));
    m.insert("auto-sensitivity".into(), json!(s.auto_sensitivity));
    for (key, value) in [
        ("level-gain", &s.level_gain),
        ("noise-gate-level", &s.noise_gate_level),
        ("silence-level-sum-threshold", &s.silence_level_sum_threshold),
    ] {
        if let Some(v) = value {
            m.insert(key.into(), v.clone());
        }
    }
    Value::Object(m)
}

/// One line about what music mode is hearing, from the monitor's reply.
pub fn hearing_text(hearing: Option<&Value>) -> String {
    let Some(hearing) = hearing.filter(|h| h.as_object().is_some_and(|m| !m.is_empty())) else {
        return "Audio mode is starting...".into();
    };
    let mut parts = Vec::new();
    for side in ["left", "right"] {
        let Some(h) = hearing.get(side).filter(|h| h.as_object().is_some_and(|m| !m.is_empty())) else { continue };
        let device =
            h.get("device").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("no sound device yet");
        let state = if let Some(problem) = h.get("problem").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            problem.to_string()
        } else if h.get("silent").and_then(|v| v.as_bool()).unwrap_or(false) {
            "too quiet".into()
        } else {
            "hearing sound".into()
        };
        parts.push(format!("{side} panel: {state} ({device})"));
    }
    if parts.is_empty() {
        "Audio mode is starting...".into()
    } else {
        format!("Audio mode is listening. {}", parts.join("; "))
    }
}

pub fn loudness_db(samples: &[Stereo]) -> f64 {
    let peak = samples.iter().flat_map(|s| s.iter()).fold(0.0f64, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -120.0
    }
}

struct State {
    settings: Settings,
    microphones: Vec<String>,
    capture: Option<Arc<Capture>>,
    capture_key: Option<(InputMode, String)>,
    capture_problem: Option<String>,
    panels: Option<[MusicPanel; 2]>,
    last_frame: Instant,
    showing: bool,
}

impl State {
    fn listen(&mut self) {
        let key = (self.settings.input_mode, self.settings.input_device.clone());
        if self.capture.is_some() && self.capture_key.as_ref() == Some(&key) {
            return;
        }
        self.stop_listening();
        match capture::acquire(key.0, &key.1, None) {
            Ok(c) => {
                self.capture = Some(c);
                self.capture_key = Some(key);
                self.capture_problem = None;
            }
            Err(e) => {
                wr_core::warn!("The Audio page couldn't listen: {e}");
                self.capture_problem = Some(format!("Couldn't listen: {e}"));
                self.capture_key = Some(key);
            }
        }
    }

    fn stop_listening(&mut self) {
        if let Some(c) = self.capture.take() {
            capture::release(&c);
        }
        self.capture_key = None;
    }
}

fn load_settings() -> Settings {
    wr_music::from_json(&settings::get("music").unwrap_or(Value::Null))
}

fn fall_index(f: FallSpeed) -> i32 {
    match f {
        FallSpeed::Slow => 0,
        FallSpeed::Normal => 1,
        FallSpeed::Fast => 2,
    }
}

fn show_settings(app: &AppWindow, s: &Settings) {
    let page = app.global::<MusicPage>();
    page.set_input_mode(if s.input_mode == InputMode::Microphone { 1 } else { 0 });
    let look = STYLES.iter().position(|(k, _, _)| *k == s.style).unwrap_or(0);
    page.set_look(look as i32);
    page.set_look_help(STYLES[look].2.into());
    page.set_bar_options(!NO_BAR_OPTIONS.contains(&s.style.as_str()));
    page.set_span_panels(s.span_panels);
    page.set_peak_dots(s.peak_dots);
    page.set_shaded(s.shaded);
    page.set_flip(s.flip);
    page.set_auto_sensitivity(s.auto_sensitivity);
    page.set_fall_speed(fall_index(s.fall_speed));
    page.set_sensitivity(s.sensitivity as f32);
}

fn fill_microphones(app: &AppWindow, state: &mut State, names: Vec<String>) {
    state.microphones = names;
    let mut values: Vec<SharedString> = vec![DEFAULT_MICROPHONE.into()];
    values.extend(state.microphones.iter().map(|n| SharedString::from(n.as_str())));
    let wanted = state.settings.input_device.clone();
    if !wanted.is_empty() && !state.microphones.contains(&wanted) {
        // Picked before, but not plugged in now
        state.microphones.push(wanted.clone());
        values.push(format!("{wanted} (not found)").into());
    }
    let page = app.global::<MusicPage>();
    page.set_microphones(ModelRc::new(VecModel::from(values)));
    let index =
        if wanted.is_empty() { 0 } else { state.microphones.iter().position(|m| *m == wanted).map_or(0, |i| i + 1) };
    page.set_microphone(index as i32);
}

fn read_settings(app: &AppWindow, state: &State) -> Settings {
    let page = app.global::<MusicPage>();
    let mut raw = to_json(&state.settings);
    raw["input-mode"] = json!(if page.get_input_mode() == 1 { "microphone" } else { "playback" });
    let index = page.get_microphone();
    raw["input-device"] = json!(if index > 0 {
        state.microphones.get(index as usize - 1).cloned().unwrap_or_default()
    } else if index < 0 {
        state.settings.input_device.clone()
    } else {
        String::new()
    });
    if let Some((key, _, _)) = STYLES.get(page.get_look().max(0) as usize) {
        raw["style"] = json!(key);
    }
    raw["span-panels"] = json!(page.get_span_panels());
    raw["peak-dots"] = json!(page.get_peak_dots());
    raw["shaded"] = json!(page.get_shaded());
    raw["flip"] = json!(page.get_flip());
    raw["auto-sensitivity"] = json!(page.get_auto_sensitivity());
    raw["fall-speed"] = json!(["slow", "normal", "fast"][page.get_fall_speed().clamp(0, 2) as usize]);
    raw["sensitivity"] = json!(page.get_sensitivity().round() as i64);
    wr_music::from_json(&raw)
}

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
}

fn page_state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.borrow().clone()).expect("music page wired")
}

/// The music mode line, from the monitor's reply to {"cmd": "music"}.
fn show_music_mode(app: &AppWindow, reply: Option<Value>) {
    let page = app.global::<MusicPage>();
    let Some(on) = reply.as_ref().and_then(|r| r.get("music_mode")).and_then(|v| v.as_bool()) else {
        page.set_mode_status("The system monitor isn't running, so audio mode can't start.".into());
        page.set_mode_hearing("".into());
        return;
    };
    app.global::<App>().set_music_mode(on);
    page.set_mode_status(
        if on { "On: both panels show the audio visualizer." } else { "Off: the panels show your layout." }.into(),
    );
    page.set_mode_hearing(if on {
        hearing_text(reply.as_ref().and_then(|r| r.get("hearing"))).into()
    } else {
        "".into()
    });
}

fn find_microphones(ctx: &Ctx) {
    ctx.in_background(capture::list_microphones, |app, names| {
        let st = page_state();
        fill_microphones(app, &mut st.borrow_mut(), names);
    });
}

fn draw_frame(app: &AppWindow, state: &mut State) {
    let now = Instant::now();
    let dt = now.duration_since(state.last_frame).as_secs_f64().min(0.5);
    state.last_frame = now;
    let samples: Vec<Stereo> = match &state.capture {
        Some(c) => c.snapshot_f64(),
        None => vec![[0.0, 0.0]; FFT_SIZE],
    };
    let settings = state.settings.clone();
    let panels = state.panels.get_or_insert_with(|| {
        [
            MusicPanel::new(&settings, false, wr_music::panel::IDLE_AFTER_SEC),
            MusicPanel::new(&settings, true, wr_music::panel::IDLE_AFTER_SEC),
        ]
    });
    let frames: Vec<preview::Levels> = panels.iter_mut().map(|p| p.frame(&samples, dt)).collect();
    let silent = panels[0].is_silent();
    let page = app.global::<MusicPage>();
    page.set_preview(preview::render(&frames, 2));

    // The sound check
    let tail = &samples[samples.len().saturating_sub(2048)..];
    let db = loudness_db(tail);
    page.set_meter(((db + 80.0) / 80.0).clamp(0.0, 1.0) as f32);
    let text = if let Some(problem) = &state.capture_problem {
        problem.clone()
    } else if let Some(c) = &state.capture {
        let name = c.device_name().unwrap_or_else(|| "the sound device".into());
        let mic = settings.input_mode == InputMode::Microphone;
        if let Some(problem) = c.problem() {
            problem
        } else if db <= -100.0 {
            format!("{} ({name})", if mic { "Silence" } else { "Nothing is playing" })
        } else if silent {
            if mic {
                format!("Listening to {name}, but it's too quiet for the bars. Play music nearer the microphone, or turn the sensitivity up.")
            } else {
                format!("Listening to {name}, but it's too quiet for the bars. Turn the volume or the sensitivity up.")
            }
        } else {
            format!("Hearing sound from {name}")
        }
    } else {
        String::new()
    };
    page.set_hearing(text.into());
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let state = Rc::new(RefCell::new(State {
        settings: load_settings(),
        microphones: Vec::new(),
        capture: None,
        capture_key: None,
        capture_problem: None,
        panels: None,
        last_frame: Instant::now(),
        showing: false,
    }));
    STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    let page = app.global::<MusicPage>();
    page.set_shortcut(crate::pages::shortcuts::keys_text("m").into());
    let looks: Vec<SharedString> = STYLES.iter().map(|(_, label, _)| SharedString::from(*label)).collect();
    page.set_looks(ModelRc::new(VecModel::from(looks)));
    page.set_preview(preview::render(&[preview::dark(), preview::dark()], 2));
    show_settings(app, &state.borrow().settings);
    fill_microphones(app, &mut state.borrow_mut(), Vec::new());
    find_microphones(ctx);

    {
        let ctx2 = ctx.clone();
        page.on_find_microphones(move || find_microphones(&ctx2));
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        page.on_changed(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let mut s = state.borrow_mut();
            let new = read_settings(&app, &s);
            if new == s.settings {
                return;
            }
            s.settings = new.clone();
            s.panels = None;
            if s.showing {
                s.listen();
            }
            drop(s);
            show_settings(&app, &new);
            let page = app.global::<MusicPage>();
            if !settings::set("music", to_json(&new)) {
                page.set_status("Couldn't save the change (see the log).".into());
                return;
            }
            page.set_status("Saved. Audio mode uses this look from now on.".into());
            // The monitor starts the visualizer again with the new look, if music mode is on
            ctx2.in_background(
                || link::request(json!({"cmd": "music", "action": "reload"}), Duration::from_secs(3)),
                |_, _| {},
            );
        });
    }
    // Music mode switched here or elsewhere: show it
    {
        let ctx2 = ctx.clone();
        ctx.on_page.lock().unwrap().push(Box::new(move |_app, key| {
            let st = page_state();
            let mut s = st.borrow_mut();
            let showing = key == "music";
            if showing == s.showing {
                return;
            }
            s.showing = showing;
            if showing {
                s.listen();
                s.last_frame = Instant::now();
                drop(s);
                ctx2.in_background(
                    || link::request(json!({"cmd": "music", "action": "status"}), Duration::from_secs(3)),
                    show_music_mode,
                );
            } else {
                s.stop_listening();
            }
        }));
    }
    {
        let (ui, state) = (ctx.ui.clone(), state.clone());
        ctx.every(FRAME_SECONDS, move || {
            let Some(app) = ui.upgrade() else { return };
            let mut s = state.borrow_mut();
            if !s.showing {
                return;
            }
            draw_frame(&app, &mut s);
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        ctx.every(POLL_SECONDS, move || {
            if !state.borrow().showing {
                return;
            }
            ctx2.in_background(
                || link::request(json!({"cmd": "music", "action": "status"}), Duration::from_secs(3)),
                show_music_mode,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_with_python_keys() {
        let s = wr_music::from_json(&json!({"style": "bars", "fall-speed": "fast", "sensitivity": 7, "level-gain": 2}));
        let j = to_json(&s);
        assert_eq!(j["style"], "bars");
        assert_eq!(j["fall-speed"], "fast");
        assert_eq!(j["sensitivity"], 7);
        assert_eq!(j["level-gain"], 2);
        assert_eq!(wr_music::from_json(&j), s);
    }

    #[test]
    fn hearing_line() {
        assert_eq!(hearing_text(None), "Audio mode is starting...");
        let h = json!({"left": {"device": "Speakers", "silent": true}, "right": {"problem": "No device"}});
        assert_eq!(
            hearing_text(Some(&h)),
            "Audio mode is listening. left panel: too quiet (Speakers); right panel: No device (no sound device yet)"
        );
    }

    #[test]
    fn loudness() {
        assert_eq!(loudness_db(&[[0.0, 0.0]]), -120.0);
        assert!((loudness_db(&[[0.5, -1.0]]) - 0.0).abs() < 1e-9);
    }
}
