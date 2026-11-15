//! Plug animation (`effects_tab.py`'s Effects): the plug animations, the text line, sliding between apps and dimming. The
//! monitor plays them; this page saves the settings in settings.json, asks the monitor to reload them,
//! and plays the same effects (wr-core's) in its preview. Also "Teach White Rabbit my ports".

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

const FRAME_SECONDS: f64 = 1.0 / 30.0;
const POLL_SECONDS: f64 = 2.0;
const TEACH_POLL_SECONDS: f64 = 0.6;
const CHARGER: &str = "That's the charger. Plug it in, then use 'Wrong port?' below to set the port you charge from.";

/// "left-back" ... "right-front", the order of the port lists.
fn positions() -> Vec<String> {
    Side::ALL
        .iter()
        .flat_map(|side| Spot::ALL.iter().map(move |spot| format!("{}-{}", side.name(), spot.name())))
        .collect()
}

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

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// A typical monitor picture: CPU and memory on the left, disk and network on the right.
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

fn flipped(grid: &Grid) -> Grid {
    let mut out = Grid::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            out.set(x, y, grid.get(x, HEIGHT - 1 - y));
        }
    }
    out
}

struct Teacher {
    step: usize,
    /// position: the port's key, None when skipped
    done: Vec<(String, Option<String>)>,
    waiting_since: f64,
}

struct State {
    settings: EffectsSettings,
    preview: Effects,
    frames: [Grid; 2],
    slide_flip: bool,
    start: Instant,
    last_plug: Map<String, Value>,
    teacher: Option<Teacher>,
}

impl State {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

type Shared = Rc<RefCell<State>>;

thread_local! {
    static STATE: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// The page's state, on the window's thread.
fn page_state() -> Shared {
    STATE.with(|s| s.borrow().clone()).expect("effects page wired")
}

/// The vertical messages' letter styles, as the Letters list shows them.
const VERTICAL_STYLES: [&str; 3] = ["ticker", "sideways", "stacked"];

/// The unplug animations in the list's order: the drain first, then the [`Look`]s.
fn unplug_keys() -> Vec<String> {
    std::iter::once(DRAIN.to_string()).chain(Look::PLUG.iter().map(|l| l.key().to_string())).collect()
}

/// The plug-in animations in the list's order: the ripple first, then the [`Look`]s.
fn plug_keys() -> Vec<String> {
    std::iter::once(RIPPLE.to_string()).chain(Look::PLUG.iter().map(|l| l.key().to_string())).collect()
}

/// The key at a list position, `fallback` when nothing sensible is picked.
fn pick(keys: &[String], index: i32, fallback: &str) -> String {
    usize::try_from(index).ok().and_then(|i| keys.get(i)).cloned().unwrap_or_else(|| fallback.to_string())
}

fn look_labels(first: &str) -> ModelRc<SharedString> {
    let labels: Vec<SharedString> =
        std::iter::once(first.into()).chain(Look::PLUG.iter().map(|l| l.label().into())).collect();
    ModelRc::new(VecModel::from(labels))
}

fn load_settings() -> EffectsSettings {
    EffectsSettings::from_json(&settings::get("effects").unwrap_or(Value::Null))
}

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
    page.set_vertical_left(s.vertical_left.clone().into());
    page.set_vertical_right(s.vertical_right.clone().into());
    page.set_vertical_style(VERTICAL_STYLES.iter().position(|v| *v == s.vertical_style).unwrap_or(0) as i32);
    page.set_show_vertical(s.show_vertical);
    page.set_max_messages(EffectsSettings::MAX_MESSAGES as i32);
    if !s.messages.is_empty() {
        let lines: Vec<slint::SharedString> = s.messages.iter().map(|m| m.as_str().into()).collect();
        page.set_messages(slint::ModelRc::new(slint::VecModel::from(lines)));
    }
    page.set_full_shades(s.full_shades);
}

/// Asks the monitor (on a worker thread); `done` gets the reply, None when it isn't running.
fn ask_monitor(ctx: &Ctx, message: Value, done: impl FnOnce(&AppWindow, Option<Value>) + Send + 'static) {
    let mut message = message;
    message["cmd"] = json!("effects");
    ctx.in_background(move || link::request(message, Duration::from_secs(3)), done);
}

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
    if plug.get("key").and_then(|v| v.as_str()).is_some_and(|k| !k.is_empty()) {
        page.set_can_fix(true);
        if let Some(i) = position.and_then(|p| positions().iter().position(|q| q == p)) {
            page.set_fix_position(i as i32);
        }
    }
}

// Teaching the ports -----------------------------------------------------------------------------------

fn show_teacher(app: &AppWindow, t: &Teacher) {
    let page = app.global::<EffectsPage>();
    let all = positions();
    let marks: Vec<SharedString> = all
        .iter()
        .enumerate()
        .map(|(i, p)| {
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

fn advance(app: &AppWindow, t: &mut Teacher, key: Option<String>) {
    let all = positions();
    t.done.push((all[t.step].clone(), key));
    t.step += 1;
    t.waiting_since = unix_now();
    show_teacher(app, t);
}

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
    got_reply(state, app, &Some(reply.clone()));
    let mut s = state.borrow_mut();
    let Some(t) = s.teacher.as_mut() else { return };
    let all = positions();
    let plug = reply.get("last_plug").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let plugged_in = plug.get("plugged_in").and_then(|v| v.as_bool()).unwrap_or(false);
    let time = plug.get("time").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if t.step >= all.len() || !plugged_in || time < t.waiting_since {
        return;
    }
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
    ask_monitor(ctx, json!({"action": "learn", "key": key, "position": position}), |_, _| {});
    page.set_teach_heard(
        format!("Got it: {name}. That is the {} port.", position_label(Some(&position)).to_lowercase()).into(),
    );
    advance(app, t, Some(key));
}

// Wiring -----------------------------------------------------------------------------------------------

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let settings = load_settings();
    let mut effects = Effects::new();
    effects.configure(settings.clone());
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
    page.set_fix_position(-1);
    page.set_unplug_looks(look_labels("Drain (original)"));
    page.set_plug_looks(look_labels("Water ripple (original)"));
    show_settings(app, &settings);
    page.set_preview(preview::render(&[preview::dark(), preview::dark()], 2));

    // Settings changed: save them, tell the monitor, and play them in the preview
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        page.on_changed(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let mut s = state.borrow_mut();
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
            new.messages = page.get_messages().iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
            new.show_messages = page.get_pinned() && !new.messages.is_empty();
            new.vertical_left = page.get_vertical_left().trim().to_string();
            new.vertical_right = page.get_vertical_right().trim().to_string();
            new.vertical_style = VERTICAL_STYLES[page.get_vertical_style().clamp(0, 2) as usize].to_string();
            new.show_vertical =
                page.get_show_vertical() && !(new.vertical_left.is_empty() && new.vertical_right.is_empty());
            new.full_shades = page.get_full_shades();
            // The monitor may have learned ports since this page read them
            new.learned_ports = load_settings().learned_ports;
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
            drop(s);
            page.set_status(if saved { "Saved.".into() } else { "Couldn't save the settings.".into() });
            ask_monitor(&ctx2, json!({"action": "reload"}), |_, _| {});
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        let state2 = state.clone();
        page.on_try_transition(move || {
            // Change the preview's apps (the sample pictures swap sides) with the chosen transition
            let mut s = state2.borrow_mut();
            let now = s.now();
            s.preview.start_slide(Side::Left, now);
            s.preview.start_slide(Side::Right, now);
            s.slide_flip = !s.slide_flip;
        });
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
        page.on_learn(move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let page = app.global::<EffectsPage>();
            let key = state.borrow().last_plug.get("key").and_then(|v| v.as_str()).map(String::from);
            let all = positions();
            let (Some(key), Some(position)) = (key, all.get(page.get_fix_position().max(-1) as usize)) else { return };
            ask_monitor(&ctx2, json!({"action": "learn", "key": key, "position": position}), |_, _| {});
            page.set_status(format!("Remembered: {}.", position_label(Some(position))).into());
        });
    }
    {
        let (ctx2, state) = (ctx.clone(), state.clone());
        page.on_say(move |text| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().preview.say(&text);
            ask_monitor(&ctx2, json!({"action": "say", "text": text}), |_, _| {});
        });
    }
    // Your own messages: editing changes the list (saved by Show them, Enter, or adding and removing)
    {
        let ctx2 = ctx.clone();
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
        let last = RefCell::new(Instant::now() - Duration::from_secs(60));
        ctx.every(TEACH_POLL_SECONDS, move || {
            let Some(app) = ctx2.ui.upgrade() else { return };
            let teaching = state.borrow().teacher.is_some();
            let wait = if teaching { TEACH_POLL_SECONDS } else { POLL_SECONDS };
            if !teaching && app.global::<App>().get_page() != "effects" {
                return;
            }
            if last.borrow().elapsed().as_secs_f64() < wait - 0.05 {
                return;
            }
            *last.borrow_mut() = Instant::now();
            ask_monitor(&ctx2, json!({"action": "status"}), |app, reply| {
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
