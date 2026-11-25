//! Plug animation (`effects_tab.py`'s Effects): the plug animations, the text line, sliding between apps
//! and dimming. The monitor plays them; this page saves the settings in settings.json, asks the monitor
//! to reload them, and plays the same effects (wr-core's) in its preview. Also "Teach White Rabbit my
//! ports".
//!
//! The `EffectsPage` global in Slint is shared by three pages of the window (`ui/pages/effects.slint`,
//! `messages.slint` and `transitions.slint`), so all their switches end up here. The Draw page also reads
//! its "panels together" switch.
//!
//! How the Slint callbacks map to Rust (all are connected in `wire` and run on the UI thread):
//! `changed()` fires on any setting change; it reads every property back into an `EffectsSettings`,
//! saves it as "effects" in settings.json, reconfigures the preview and asks the monitor to "reload".
//! `try-transition()` and `try-plug(in)` play an effect in the preview (and, for plugs, on the real
//! panels through the monitor). `learn()` corrects the port of the last plug event. `say(text)` shows
//! a one-off message. `message-edited`, `add-message` and `remove-message` edit the list of the user's
//! own messages. `teach`, `teach-skip`, `teach-start-over` and `teach-close` run the port-teaching
//! walk-through.
//!
//! Worker threads: every message to the monitor (`ask_monitor`) goes through `link::request` on a
//! worker thread via `Ctx::in_background`, because the monitor may be slow or not running and the
//! request can wait up to 3 seconds. The reply comes back to the UI thread. Everything else (the
//! preview drawing, settings.json) runs on the UI thread: the preview is a few hundred pixels and
//! settings.json is a small file.
//!
//! What is saved: the "effects" object in settings.json, written by this page on every change. The
//! monitor also writes into the same object ("learned_ports", when the user teaches or corrects a
//! port), which is why `changed()` re-reads that part of the file just before saving.
//!
//! Two repeating timers keep the page alive: the preview animation (30 frames a second, only while one
//! of the three pages is showing) and a poll of the monitor's "last plug" status (every 2 seconds on
//! this page, every 0.6 seconds while teaching).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use wr_core::drawing;
use wr_core::effects::{position_label, Effects, EffectsSettings, Side, Spot, Transition, DRAIN, RIPPLE};
use wr_core::grid::{Grid, HEIGHT, WIDTH};
use wr_core::plug_looks::Look;

use crate::app::Ctx;
use crate::{link, preview, settings, App, AppWindow, EffectsPage};

/// Seconds between preview frames: 30 frames a second, smooth enough for the ripples and slides.
const FRAME_SECONDS: f64 = 1.0 / 30.0;
/// Seconds between "what was last plugged in?" questions to the monitor while the page is showing.
const POLL_SECONDS: f64 = 2.0;
/// Seconds between those questions while teaching ports, so the page reacts quickly to a plug. It is
/// also the tick of the poll timer itself; the slower rate is made by skipping ticks.
const TEACH_POLL_SECONDS: f64 = 0.6;
/// Shown while teaching when the user plugs in the charger: the charger is reported differently by
/// Windows (as a power source, not a device with a port key), so it can't be taught this way.
const CHARGER: &str = "That's the charger. Plug it in, then use 'Wrong port?' below to set the port you charge from.";

/// "left-back" ... "right-front", the order of the port lists.
/// The port positions as keys, side first then spot, in the order the page's port lists and the teaching
/// walk-through use. Index into this to turn a list position from Slint into a key and back.
fn positions() -> Vec<String> {
    Side::ALL
        .iter()
        .flat_map(|side| Spot::ALL.iter().map(move |spot| format!("{}-{}", side.name(), spot.name())))
        .collect()
}

/// Turns the monitor's "how" code (how it worked out which port a plug used) into words for the
/// "Last: ..." line. An unknown code gives an empty string, and the line then leaves the brackets off.
fn how(how: &str) -> &'static str {
    match how {
        "learned" => "the port you picked for it",
        "framework_tool" => "as Framework's framework_tool reported",
        "side" => "Windows only says which side",
        "remembered" => "where it was plugged in",
        "unknown" => "White Rabbit couldn't tell which port",
        _ => "",
    }
}

/// Seconds since 1970 (wall clock), the same time base the monitor stamps plug events with. Used to ignore
/// plug events that happened before the teacher started waiting. Gives 0 if the clock is before 1970.
fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// A typical monitor picture: CPU and memory on the left, disk and network on the right.
/// The preview's pretend apps are drawn with wr-core's own app drawing functions, so the preview looks
/// like the real panels. `fg` (160) and `bg` (35) are LED brightness levels out of 255 for the app and
/// its border; the last number in each call is the row (0 = top, 16 = lower half of the 34-row panel).
fn sample_frames() -> [Grid; 2] {
    let (fg, bg) = (160, 35);
    let mut left = Grid::new();
    drawing::draw_cpu_vals(&mut left, &[0.9, 0.35, 0.6, 0.2, 0.75, 0.45, 0.3, 0.55], fg, 0);
    drawing::draw_app_border("cpu", &mut left, bg, 0, 8);
    drawing::draw_memory(&mut left, 0.5, fg, 16);
    drawing::draw_app_border("mem", &mut left, bg, 16, 0);
    let mut right = Grid::new();
    drawing::draw_disk_net(&mut right, (0.2, 0.8), (1.0, 0.4), fg, 0);
    drawing::draw_app_border("disk-net", &mut right, bg, 0, 0);
    drawing::draw_disk_net(&mut right, (0.55, 0.05), (0.3, 0.95), fg, 16);
    drawing::draw_app_border("disk-net", &mut right, bg, 16, 0);
    [left, right]
}

/// The picture upside down (row y becomes row HEIGHT-1-y). The preview flips its sample pictures each time
/// it "changes app", so a slide or fade between two different-looking pictures can be seen.
fn flipped(grid: &Grid) -> Grid {
    let mut out = Grid::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            out.set(x, y, grid.get(x, HEIGHT - 1 - y));
        }
    }
    out
}

/// The "Teach White Rabbit my ports" walk-through: it asks for a plug in each port of `positions()` in
/// turn and records which port key Windows reports, so the monitor can map that key to the position.
struct Teacher {
    /// Index into `positions()` of the port being asked for; equal to its length when finished.
    step: usize,
    /// position: the port's key, None when skipped
    /// The steps answered so far, in order, as (position key such as "left-back", port key or None).
    done: Vec<(String, Option<String>)>,
    /// Unix time (seconds) the current step started. Plug events stamped earlier are old news and ignored.
    waiting_since: f64,
}

/// Everything the page keeps between callbacks. It lives only on the UI thread (in an `Rc<RefCell>`),
/// so callbacks and timers share it without locks.
struct State {
    /// The settings as last saved (or loaded at start-up).
    settings: EffectsSettings,
    /// wr-core's effects engine, the same one the monitor runs, driving the on-page preview.
    preview: Effects,
    /// The left and right sample pictures from `sample_frames`.
    frames: [Grid; 2],
    /// Whether the preview currently shows the sample pictures upside down (see `flipped`).
    slide_flip: bool,
    /// When the page was wired; the preview's clock (`State::now`) counts from here.
    start: Instant,
    /// The monitor's last plug event (name, key, position, how, time...), kept for the "Wrong port?" fix.
    last_plug: Map<String, Value>,
    /// The teaching walk-through, while it is open.
    teacher: Option<Teacher>,
}

impl State {
    /// Seconds since the page was wired: the time base the `Effects` preview uses for its animations.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// The page state as shared by all of the page's callbacks and timers.
type Shared = Rc<RefCell<State>>;

// The page state, reachable from reply callbacks that are built as `Send` closures (they run on the UI
// thread, but can't capture the non-`Send` `Rc`). Thread-local because the state belongs to the UI thread.
thread_local! {
    static STATE: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// The page's state, on the window's thread.
/// Panics if called before `wire`, which can't happen because replies only come after `wire` sent the
/// first request.
fn page_state() -> Shared {
    STATE.with(|s| s.borrow().clone()).expect("effects page wired")
}

/// The vertical messages' letter styles, as the Letters list shows them.
const VERTICAL_STYLES: [&str; 3] = ["ticker", "sideways", "stacked"];

/// How your message's lines line up, as the list shows them.
const ALIGNS: [&str; 3] = ["left", "center", "right"];

/// The unplug animations in the list's order: the drain first, then the [`Look`]s.
fn unplug_keys() -> Vec<String> {
    std::iter::once(DRAIN.to_string()).chain(Look::PLUG.iter().map(|l| l.key().to_string())).collect()
}

/// The plug-in animations in the list's order: the ripple first, then the [`Look`]s.
fn plug_keys() -> Vec<String> {
    std::iter::once(RIPPLE.to_string()).chain(Look::PLUG.iter().map(|l| l.key().to_string())).collect()
}

/// The key at a list position, `fallback` when nothing sensible is picked.
/// A negative index (Slint's "nothing selected") or one past the end gives `fallback`.
fn pick(keys: &[String], index: i32, fallback: &str) -> String {
    usize::try_from(index).ok().and_then(|i| keys.get(i)).cloned().unwrap_or_else(|| fallback.to_string())
}

/// The drop-down labels for an animation list: `first` (the original effect) followed by each [`Look`]'s
/// label, in the same order as `unplug_keys` / `plug_keys`.
fn look_labels(first: &str) -> ModelRc<SharedString> {
    let labels: Vec<SharedString> =
        std::iter::once(first.into()).chain(Look::PLUG.iter().map(|l| l.label().into())).collect();
    ModelRc::new(VecModel::from(labels))
}

/// Reads the "effects" object from settings.json. A missing or broken entry gives the defaults (see
/// `EffectsSettings::from_json`), so the page always has something to show.
fn load_settings() -> EffectsSettings {
    EffectsSettings::from_json(&settings::get("effects").unwrap_or(Value::Null))
}

/// Copies the settings into the page's properties. Drop-downs are set by position in their lists; a saved
/// value that isn't in the list (an older or newer version's name) selects the first entry.
fn show_settings(app: &AppWindow, s: &EffectsSettings) {
    let page = app.global::<EffectsPage>();
    page.set_plug_animation(s.plug_animation);
    page.set_plug_charger(s.plug_charger);
    page.set_plug_headphones(s.plug_headphones);
    page.set_plug_usb(s.plug_usb);
    page.set_plug_displays(s.plug_displays);
    page.set_plug_left(s.plug_left);
    page.set_plug_right(s.plug_right);
    page.set_text_line(s.text_line);
    page.set_text_song(s.text_song);
    page.set_text_charger(s.text_charger);
    page.set_text_battery(s.text_battery);
    page.set_text_speed(s.text_speed as f32);
    page.set_slide(s.slide);
    page.set_fade(s.fade);
    page.set_transition_look(s.transition_look.as_str().into());
    page.set_panels_together(s.panels_together);
    page.set_unplug_look(unplug_keys().iter().position(|k| *k == s.unplug_look).unwrap_or(0) as i32);
    page.set_plug_look(plug_keys().iter().position(|k| *k == s.plug_look).unwrap_or(0) as i32);
    page.set_pinned(s.show_messages);
    page.set_messages_align(ALIGNS.iter().position(|a| *a == s.messages_align).unwrap_or(0) as i32);
    page.set_vertical_left(s.vertical_left.clone().into());
    page.set_vertical_right(s.vertical_right.clone().into());
    page.set_vertical_style(VERTICAL_STYLES.iter().position(|v| *v == s.vertical_style).unwrap_or(0) as i32);
    page.set_show_vertical(s.show_vertical);
    page.set_max_messages(EffectsSettings::MAX_MESSAGES as i32);
    // With no saved messages, keep the page's default (one empty line to type into).
    if !s.messages.is_empty() {
        let lines: Vec<slint::SharedString> = s.messages.iter().map(|m| m.as_str().into()).collect();
        page.set_messages(slint::ModelRc::new(slint::VecModel::from(lines)));
    }
    page.set_full_shades(s.full_shades);
}

/// Asks the monitor (on a worker thread); `done` gets the reply, None when it isn't running.
/// Every request here is an "effects" control command with an "action"; the monitor answers all of them
/// with its last plug event ("last_plug") and its open panel ports. 3 seconds is enough for a monitor
/// that is alive; if it isn't, `done` gets None and the page says so where it matters.
fn ask_monitor(ctx: &Ctx, message: Value, done: impl FnOnce(&AppWindow, Option<Value>) + Send + 'static) {
    let mut message = message;
    message["cmd"] = json!("effects");
    ctx.in_background(move || link::request(message, Duration::from_secs(3)), done);
}

/// Shows the monitor's last plug event in the "Last: ..." line and remembers it for the "Wrong port?"
/// fix. An empty or missing event (nothing plugged since the monitor started) leaves the page as it was.
fn got_reply(state: &Shared, app: &AppWindow, reply: &Option<Value>) {
    let Some(plug) = reply.as_ref().and_then(|r| r.get("last_plug")).and_then(|p| p.as_object()) else { return };
    if plug.is_empty() {
        return;
    }
    state.borrow_mut().last_plug = plug.clone();
    let page = app.global::<EffectsPage>();
    let what = plug.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("Something");
    let verb =
        if plug.get("plugged_in").and_then(|v| v.as_bool()).unwrap_or(false) { "plugged in" } else { "unplugged" };
    // "time" is Unix seconds from the monitor; show it as local hours:minutes. A bad value shows nothing.
    let when = plug
        .get("time")
        .and_then(|v| v.as_f64())
        .and_then(|t| chrono::DateTime::from_timestamp(t as i64, 0))
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    let position = plug.get("position").and_then(|v| v.as_str());
    let how = how(plug.get("how").and_then(|v| v.as_str()).unwrap_or(""));
    page.set_last_plug(
        format!(
            "Last: {what}, {verb} at {when}. Ripple came from: {}{}",
            position_label(position),
            if how.is_empty() { ".".to_string() } else { format!(" ({how}).") }
        )
        .into(),
    );
    // A port key means the event can be corrected: offer "Wrong port?" and preselect where the
    // monitor thinks it was, so the user only has to change it if it is wrong.
    if plug.get("key").and_then(|v| v.as_str()).is_some_and(|k| !k.is_empty()) {
        page.set_can_fix(true);
        if let Some(i) = position.and_then(|p| positions().iter().position(|q| q == p)) {
            page.set_fix_position(i as i32);
        }
    }
}

// Teaching the ports -----------------------------------------------------------------------------------

/// Draws the walk-through: one line per port with a mark (✓ learned, – skipped, ▶ asking now, ○ still
/// to come), and the question or the final summary.
fn show_teacher(app: &AppWindow, t: &Teacher) {
    let page = app.global::<EffectsPage>();
    let all = positions();
    let marks: Vec<SharedString> = all
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // The list is grouped by side on the page, so each line only names the spot ("back"...).
            let spot = p.split('-').nth(1).unwrap_or("");
            match t.done.iter().find(|(q, _)| q == p) {
                Some((_, Some(_))) => format!("✓  {spot}"),
                Some((_, None)) => format!("–  {spot} (skipped)"),
                None if i == t.step => format!("▶  {spot}"),
                None => format!("○  {spot}"),
            }
            .into()
        })
        .collect();
    page.set_teach_marks(ModelRc::new(VecModel::from(marks)));
    if t.step >= all.len() {
        let learned = t.done.iter().filter(|(_, k)| k.is_some()).count();
        page.set_teach_ask("All done".into());
        page.set_teach_heard(
            format!("White Rabbit knows {learned} of your ports now. The ripple starts from them from now on. You can run this again any time.")
                .into(),
        );
        page.set_teach_can_skip(false);
    } else {
        page.set_teach_ask(
            format!("Plug something into the {} port", position_label(Some(&all[t.step])).to_lowercase()).into(),
        );
        page.set_teach_can_skip(true);
    }
}

/// Records the answer for the current step (`key` is None when skipped), moves to the next port and
/// restarts the wait, so the plug that answered this step isn't counted for the next one.
fn advance(app: &AppWindow, t: &mut Teacher, key: Option<String>) {
    let all = positions();
    t.done.push((all[t.step].clone(), key));
    t.step += 1;
    t.waiting_since = unix_now();
    show_teacher(app, t);
}

/// Handles a status reply while teaching: decides whether the last plug event answers the current step.
/// It must be a plug-in (not an unplug), newer than when the step started, not the charger, carry a port
/// key, and not be a port already learned in this run. If it passes, the window tells the monitor to
/// "learn" key -> position (the monitor saves it in settings.json) and moves to the next step.
fn teacher_heard(ctx: &Ctx, state: &Shared, app: &AppWindow, reply: Option<Value>) {
    let page = app.global::<EffectsPage>();
    let Some(reply) = reply else {
        if state.borrow().teacher.is_some() {
            page.set_teach_heard(
                "The monitor isn't running, so White Rabbit can't see plugs. Start it on the Home page.".into(),
            );
        }
        return;
    };
    // Keep the "Last: ..." line up to date too, then look at the event for the walk-through.
    got_reply(state, app, &Some(reply.clone()));
    let mut s = state.borrow_mut();
    let Some(t) = s.teacher.as_mut() else { return };
    let all = positions();
    let plug = reply.get("last_plug").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let plugged_in = plug.get("plugged_in").and_then(|v| v.as_bool()).unwrap_or(false);
    let time = plug.get("time").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // Finished, an unplug, or an event from before this step: nothing to do.
    if t.step >= all.len() || !plugged_in || time < t.waiting_since {
        return;
    }
    // Restarting the wait after each rejected plug means the same event isn't reported again on
    // the next poll.
    if plug.get("kind").and_then(|v| v.as_str()) == Some("charger") {
        page.set_teach_heard(CHARGER.into());
        t.waiting_since = unix_now();
        return;
    }
    let name = plug.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("Something").to_string();
    let Some(key) = plug.get("key").and_then(|v| v.as_str()).filter(|k| !k.is_empty()).map(String::from) else {
        page.set_teach_heard(
            format!("{name} was plugged in, but Windows didn't say where. Try something else.").into(),
        );
        t.waiting_since = unix_now();
        return;
    };
    // The same physical port answered twice: the user plugged into the wrong one.
    if let Some((same, _)) = t.done.iter().find(|(_, k)| k.as_deref() == Some(key.as_str())) {
        page.set_teach_heard(
            format!(
                "That is the same port as {}. Unplug it and use the port shown.",
                position_label(Some(same)).to_lowercase()
            )
            .into(),
        );
        t.waiting_since = unix_now();
        return;
    }
    let position = all[t.step].clone();
    // The reply is ignored: the next status poll will show the corrected position anyway.
    ask_monitor(ctx, json!({"action": "learn", "key": key, "position": position}), |_, _| {});
    page.set_teach_heard(
        format!("Got it: {name}. That is the {} port.", position_label(Some(&position)).to_lowercase()).into(),
    );
    advance(app, t, Some(key));
}

// Wiring -----------------------------------------------------------------------------------------------

/// Sets up the page state and preview, fills the lists from settings.json and connects every callback
/// (see the file header for the list). Runs once at start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let settings = load_settings();
    let mut effects = Effects::new();
    effects.configure(settings.clone());
    // The preview always shows both panels, whatever is plugged in.
    for side in Side::ALL {
        effects.panel_present(side, true);
    }
    let state: Shared = Rc::new(RefCell::new(State {
        settings: settings.clone(),
        preview: effects,
        frames: sample_frames(),
        slide_flip: false,
        start: Instant::now(),
        last_plug: Map::new(),
        teacher: None,
    }));
    STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    let page = app.global::<EffectsPage>();
    let labels: Vec<SharedString> = positions().iter().map(|p| position_label(Some(p)).into()).collect();
    page.set_positions(ModelRc::new(VecModel::from(labels)));
    page.set_try_position(positions().iter().position(|p| p == "right-middle").unwrap_or(0) as i32);
    // -1: nothing chosen for "Wrong port?" until a plug event with a port key arrives.
    page.set_fix_position(-1);
    page.set_unplug_looks(look_labels("Drain (original)"));
    page.set_plug_looks(look_labels("Water ripple (original)"));
    show_settings(app, &settings);
    // Two dark panels until the first preview frame; 2 = drawn at twice the size (see `preview::render`).
    page.set_preview(preview::render(&[preview::dark(), preview::dark()], 2));

    // Settings changed: save them, tell the monitor, and play them in the preview
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        page.on_changed(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let mut s = state.borrow_mut();
            // Remember the old transition, to replay the preview only when it actually changed.
            let was = s.settings.transition();
            let mut new = s.settings.clone();
            new.plug_animation = page.get_plug_animation();
            new.plug_charger = page.get_plug_charger();
            new.plug_headphones = page.get_plug_headphones();
            new.plug_usb = page.get_plug_usb();
            new.plug_displays = page.get_plug_displays();
            new.plug_left = page.get_plug_left();
            new.plug_right = page.get_plug_right();
            new.text_line = page.get_text_line();
            new.text_song = page.get_text_song();
            new.text_charger = page.get_text_charger();
            new.text_battery = page.get_text_battery();
            new.text_speed = page.get_text_speed().round() as i64;
            new.slide = page.get_slide();
            new.fade = page.get_fade();
            new.transition_look = page.get_transition_look().to_string();
            new.panels_together = page.get_panels_together();
            new.unplug_look = pick(&unplug_keys(), page.get_unplug_look(), DRAIN);
            new.plug_look = pick(&plug_keys(), page.get_plug_look(), RIPPLE);
            // Empty lines stay as a gap between lines, unless every line is empty
            new.messages = page.get_messages().iter().map(|m| m.trim().to_string()).collect();
            if new.messages.iter().all(String::is_empty) {
                new.messages.clear();
            }
            new.messages_align = ALIGNS[page.get_messages_align().clamp(0, 2) as usize].to_string();
            new.show_messages = page.get_pinned() && !new.messages.is_empty();
            new.vertical_left = page.get_vertical_left().trim().to_string();
            new.vertical_right = page.get_vertical_right().trim().to_string();
            new.vertical_style = VERTICAL_STYLES[page.get_vertical_style().clamp(0, 2) as usize].to_string();
            new.show_vertical =
                page.get_show_vertical() && !(new.vertical_left.is_empty() && new.vertical_right.is_empty());
            new.full_shades = page.get_full_shades();
            // The monitor may have learned ports since this page read them
            new.learned_ports = load_settings().learned_ports;
            // A round trip through JSON applies the same cleaning and clamping as loading does, so the
            // preview, the file and the monitor all see exactly the same values.
            let new = EffectsSettings::from_json(&serde_json::to_value(&new).unwrap_or(Value::Null));
            let saved = settings::set("effects", serde_json::to_value(&new).unwrap_or(Value::Null));
            s.preview.configure(new.clone());
            // Show the new way of changing apps in the preview
            if new.transition() != was && new.transition() != Transition::Cut {
                let now = s.now();
                s.preview.start_slide(Side::Left, now);
                s.preview.start_slide(Side::Right, now);
                s.slide_flip = !s.slide_flip;
            }
            s.settings = new;
            // The state isn't needed any more; ending the RefCell borrow here keeps it free for any
            // other callback or timer that runs next (a second borrow while one is held panics).
            drop(s);
            page.set_status(if saved { "Saved.".into() } else { "Couldn't save the settings.".into() });
            // The monitor reads the "effects" object from settings.json again. Its reply isn't needed,
            // and with no monitor running the saved file is used when it starts.
            ask_monitor(&ctx2, json!({"action": "reload"}), |_, _| {});
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        let state2 = state.clone();
        page.on_try_transition(move || {
            // Change the preview's apps with the chosen transition: the sample pictures are turned
            // upside down (see `flipped`), so there is a visibly different "new app" to slide or fade in.
            let mut s = state2.borrow_mut();
            let now = s.now();
            s.preview.start_slide(Side::Left, now);
            s.preview.start_slide(Side::Right, now);
            s.slide_flip = !s.slide_flip;
        });
        // `try-plug(in)`: play the plug-in (or unplug) animation at the chosen port in the preview,
        // and ask the monitor to play it on the real panels too. Its reply updates "Last: ...".
        page.on_try_plug(move |plugged_in| {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let all = positions();
            let i = app.global::<EffectsPage>().get_try_position().max(0) as usize;
            let Some(position) = all.get(i).cloned() else { return };
            {
                let mut s = state.borrow_mut();
                let now = s.now();
                s.preview.plug(Some(&position), plugged_in, now);
            }
            ask_monitor(
                &ctx2,
                json!({"action": "try", "position": position, "event": if plugged_in { "in" } else { "out" }}),
                |app, reply| got_reply(&page_state(), app, &reply),
            );
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `learn()`: "Wrong port?" - the user says the last plug used the port picked in the list.
        // The monitor remembers that port key at that position from now on.
        page.on_learn(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let key = state.borrow().last_plug.get("key").and_then(|v| v.as_str()).map(String::from);
            let all = positions();
            // -1 (nothing picked) turns into a huge usize, which `get` rejects, so nothing happens.
            let (Some(key), Some(position)) = (key, all.get(page.get_fix_position().max(-1) as usize)) else { return };
            ask_monitor(&ctx2, json!({"action": "learn", "key": key, "position": position}), |_, _| {});
            page.set_status(format!("Remembered: {}.", position_label(Some(position))).into());
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `say(text)`: show a message once on the text line, in the preview and on the panels.
        page.on_say(move |text| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().preview.say(&text);
            ask_monitor(&ctx2, json!({"action": "say", "text": text}), |_, _| {});
        });
    }
    // Your own messages: editing changes the list (saved by Show message, Enter, or adding and removing)
    {
        let ctx2 = ctx.clone();
        // `message-edited(i, text)`: keep the model in step with the text box; not saved yet.
        page.on_message_edited(move |i, text| {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let model = app.global::<EffectsPage>().get_messages();
            if (i as usize) < model.row_count() {
                model.set_row_data(i as usize, text);
            }
        });
    }
    {
        let ctx2 = ctx.clone();
        // `add-message()`: add an empty line. A new model is set so the list on the page rebuilds.
        page.on_add_message(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let mut lines: Vec<slint::SharedString> = page.get_messages().iter().collect();
            lines.push("".into());
            page.set_messages(slint::ModelRc::new(slint::VecModel::from(lines)));
        });
    }
    {
        let ctx2 = ctx.clone();
        // `remove-message(i)`: remove a line (never the last one, so there is always a box to type in),
        // then save through `changed()`.
        page.on_remove_message(move |i| {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let mut lines: Vec<slint::SharedString> = page.get_messages().iter().collect();
            if (i as usize) < lines.len() && lines.len() > 1 {
                lines.remove(i as usize);
            }
            page.set_messages(slint::ModelRc::new(slint::VecModel::from(lines)));
            page.invoke_changed();
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `teach()`: open the walk-through at the first port.
        page.on_teach(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let t = Teacher { step: 0, done: Vec::new(), waiting_since: unix_now() };
            show_teacher(&app, &t);
            state.borrow_mut().teacher = Some(t);
            let page = app.global::<EffectsPage>();
            page.set_teach_heard("".into());
            page.set_teaching(true);
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `teach-skip()`: this laptop has no port there, or the user doesn't want to teach it.
        page.on_teach_skip(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let mut s = state.borrow_mut();
            if let Some(t) = s.teacher.as_mut() {
                if t.step < positions().len() {
                    app.global::<EffectsPage>().set_teach_heard("".into());
                    advance(&app, t, None);
                }
            }
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `teach-start-over()`: tells the monitor to forget every learned port (it clears
        // "learned_ports" in settings.json), then starts the walk-through again.
        page.on_teach_start_over(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            ask_monitor(&ctx2, json!({"action": "forget_ports"}), |_, _| {});
            let t = Teacher { step: 0, done: Vec::new(), waiting_since: unix_now() };
            show_teacher(&app, &t);
            state.borrow_mut().teacher = Some(t);
            app.global::<EffectsPage>().set_teach_heard("Forgot every port White Rabbit had learned.".into());
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // `teach-close()`: stop teaching; what was learned so far stays learned.
        page.on_teach_close(move || {
            state.borrow_mut().teacher = None;
            if let Some(app) = ctx2.ui.upgrade() {
                app.global::<EffectsPage>().set_teaching(false);
            }
        });
    }

    // The preview, 30 times a second while the page is showing
    {
        let (ui, state) = (ctx.ui.clone(), state.clone());
        ctx.every(FRAME_SECONDS, move || {
            let Some(app) = ui.upgrade() else { return };
            // The Display messages page shows the same preview
            if !matches!(app.global::<App>().get_page().as_str(), "effects" | "messages" | "transitions") {
                return;
            }
            let mut s = state.borrow_mut();
            let now = s.now();
            // Each side: the sample picture (flipped or not) run through the effects engine as the
            // monitor would before sending it to the panel, then turned into preview brightness levels.
            let mut out = Vec::new();
            for (i, side) in Side::ALL.into_iter().enumerate() {
                let frame = if s.slide_flip { flipped(&s.frames[i]) } else { s.frames[i].clone() };
                let grid = s.preview.apply(side, &frame, false, now);
                out.push(preview::levels(&grid));
            }
            app.global::<EffectsPage>().set_preview(preview::render(&out, 2));
        });
    }
    // What was last plugged in, every few seconds (more often while teaching)
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        // When the last question was sent. Starts a minute in the past so the first tick asks at once.
        let last = RefCell::new(Instant::now() - Duration::from_secs(60));
        ctx.every(TEACH_POLL_SECONDS, move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let teaching = state.borrow().teacher.is_some();
            let wait = if teaching { TEACH_POLL_SECONDS } else { POLL_SECONDS };
            if !teaching && app.global::<App>().get_page() != "effects" {
                return;
            }
            // The timer ticks every 0.6 s; when not teaching, skip ticks until 2 s have passed. The
            // 0.05 s slack stops timer jitter from skipping a tick that is just a little early.
            if last.borrow().elapsed().as_secs_f64() < wait - 0.05 {
                return;
            }
            *last.borrow_mut() = Instant::now();
            ask_monitor(&ctx2, json!({"action": "status"}), |app, reply| {
                // The reply closure must be `Send`, so it finds the state through the thread-local
                // instead of capturing it. Teaching may have started or stopped while it waited.
                let st = page_state();
                if st.borrow().teacher.is_some() {
                    teacher_heard(&crate::app::ctx(), &st, app, reply);
                } else {
                    got_reply(&st, app, &reply);
                }
            });
        });
    }
}
