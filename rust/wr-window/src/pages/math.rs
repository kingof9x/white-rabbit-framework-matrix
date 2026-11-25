//! Math (MathTab in `math_tab.py`): graphs and simulations played on the ticked panels, with a
//! preview. With both panels ticked they act as one wide picture; "Both panels" can instead leave out
//! the columns behind the keyboard, or mirror the right panel onto the left (`span.rs`).
//!
//! The pictures themselves are made in `maths.rs` (graphs, Life-like rules, traffic, gas, 1-D automata,
//! effects); a formula typed by the user is parsed by `formula.rs`. This file only starts, stops and shows
//! them.
//!
//! How the Slint callbacks (`MathPage` in `ui/pages/math.slint`) map to Rust, all connected in `wire`:
//! each "Show"/"Start" button (`show-graph`, `show-formula`, `random-graphs`, `start-life`,
//! `start-traffic`, `start-gas`, `start-automaton`, `start-effect`, `noise`, `gap-test`) reads its
//! inputs, checks them, and calls `play` with a recipe for the animation. `speed-changed` sets `SPEED`;
//! `rule-chosen` and `kind-chosen` fill in default values; `span-changed` saves the panel layout and
//! restarts the picture with it; `stop()` stops and gives the panels back.
//!
//! Threads: `play` starts one animation thread (`run`) that makes each frame, sends it to the panels and
//! sleeps until the next one. Making a frame can be slow (a big simulation) and sending blocks on the
//! serial port, so neither may happen on the UI thread. The thread hands frames to the UI through the
//! `Shared` mailbox, and a UI timer (`tick`, every 50 ms) puts the newest one in the preview. The speed is
//! an atomic, so the slider changes a running animation without restarting it.
//!
//! What is saved: only the "span" layout (shared with the Games page) in settings.json. The last picture
//! started is remembered in memory, so the Math switch elsewhere can show it again.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::formula::Formula;
use crate::games::{Pic, Rng, PANEL_WIDTH};
use crate::maths::{self, Anim, Step};
use crate::pages::games::{frame_writes, render};
use crate::panels::Panel;
use crate::span::{self, Span};
use crate::{jobs, preview, AppWindow, MathPage};

/// How often (seconds) the UI timer looks for a new frame from the animation thread.
const PREVIEW_SECONDS: f64 = 0.05;
/// The last entry of the Life rule drop-down: the user types the birth and survival counts.
const CUSTOM_RULE: &str = "Your own rule (type B and S)";
/// The status after Stop when the panels were borrowed and have now been given back.
const STOPPED: &str = "Stopped. The system monitor has the panels again.";

/// The mailbox between the animation thread and the UI thread, behind an `Arc<Mutex>`; locked only to
/// copy values in or out.
#[derive(Default)]
struct Shared {
    /// The newest frame, as the panels show it.
    frame: Option<Pic>,
    /// Goes up by one per frame, so the UI can tell a new frame from one it has shown.
    version: u64,
    /// A message from the animation for the status line (e.g. a random graph's name), taken once.
    note: Option<String>,
    /// Set when sending to a panel failed and the animation thread stopped.
    error: Option<String>,
}

/// The animation now running, as the UI thread keeps track of it.
struct Run {
    /// The animation thread's stop flag.
    job: jobs::Job,
    /// The mailbox shared with the animation thread.
    shared: Arc<Mutex<Shared>>,
    /// The frame version last put in the preview.
    shown: u64,
}

/// What can be started again when the layout changes: (title, how to make the steps).
type Again = (String, Rc<dyn Fn() -> Step>);

/// The page's state, on the UI thread only.
#[derive(Default)]
struct State {
    /// The animation now running, if any.
    run: Option<Run>,
    /// The last animation started, kept after it stops so the Math switch and layout changes can restart it.
    last: Option<Again>,
}

// Thread-local because the state holds `Rc`s and is only used on the UI thread.
thread_local! {
    static STATE: Rc<RefCell<State>> = Rc::default();
}

/// The page's state. Only call it on the UI thread.
fn page_state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.clone())
}

/// Steps per second, read by the animation thread.
/// Written by the speed slider on the UI thread; `Relaxed` is enough because it is a single number with
/// no other data depending on it.
static SPEED: AtomicU32 = AtomicU32::new(8);

/// Sets the status line on the Math page.
fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<MathPage>().set_status(text.into());
}

/// The animation loop, on its own thread.
/// `layout` and `count` (the number of panels) turn the animation's picture into what each panel shows.
/// Borrows the panels from the monitor first, then loops: next frame, send (through the lead-in
/// transition at first), post to the UI, wait for the next step at `SPEED` steps a second. Ends when
/// the job is stopped or a write fails.
fn run(
    mut anim: Anim,
    layout: Span,
    count: usize,
    job: jobs::Job,
    shared: Arc<Mutex<Shared>>,
    panels: Vec<Panel>,
    parts: crate::app::Parts,
) {
    let sender = parts.sender.clone();
    let mut lead = None;
    if !panels.is_empty() {
        parts.lender.borrow(&sender, &panels);
        // The app transition from what the monitor showed
        lead = crate::lead_in::LeadIn::new(&parts.lender.take_last_frames(&panels));
        let p2 = parts.clone();
        Ctx::later(&parts.ui, move |app| p2.update_bar(app));
        for p in &panels {
            // Stop the firmware's own scrolling, if on. An error here shows up on the first frame anyway.
            let _ = sender.write(&p.device, &[cmd::animate(false)]);
        }
    }
    let mut next_at = Instant::now();
    while !job.stopped() {
        let frame = layout.to_panels(&anim.next_frame(), count);
        if !panels.is_empty() {
            let writes = match lead.as_mut() {
                Some(l) if l.active() => l.writes(&frame, &panels),
                _ => {
                    lead = None;
                    frame_writes(&frame, &panels)
                }
            };
            if let Err(e) = sender.write_many(&writes) {
                wr_core::warn!("The Math animation stopped: {e}");
                shared.lock().unwrap().error = Some(format!("Stopped: {e}"));
                return;
            }
        }
        {
            // Shown in the window once it's on the panels, so the window doesn't run ahead of them
            let mut s = shared.lock().unwrap();
            s.frame = Some(frame.clone());
            s.version += 1;
            if let Some(note) = anim.take_note() {
                s.note = Some(note);
            }
        }
        // Steps are timed from a fixed schedule (not "sleep after each frame"), so the rate holds even
        // when making a frame takes a while.
        next_at += Duration::from_secs_f64(1.0 / SPEED.load(Ordering::Relaxed).max(1) as f64);
        // While the app transition plays, move it on about 30 times a second between the animation's
        // own frames (a slow animation would make it jerky)
        while let Some(l) = lead.as_mut().filter(|l| l.active() && Instant::now() < next_at) {
            if !jobs::sleep(&job, 1.0 / 30.0) {
                return;
            }
            if let Err(e) = sender.write_many(&l.writes(&frame, &panels)) {
                wr_core::warn!("The Math animation stopped: {e}");
                return;
            }
        }
        let now = Instant::now();
        if next_at > now {
            if !jobs::sleep(&job, (next_at - now).as_secs_f64()) {
                break;
            }
        } else {
            // Running late (frames slower than the speed asks): don't try to catch up with a burst.
            next_at = now;
        }
    }
}

/// Starts an animation on the ticked panels (or just the preview if none is ticked).
/// `make` builds a fresh animation; it is kept in `State::last` so the same picture can be started again
/// later. Stops any earlier Math animation and any game first; `jobs::start` also ends a clock or other
/// job on those panels. Without ticked panels the picture is two panels wide and only shows in the window.
fn play(app: &AppWindow, title: &str, make: Rc<dyn Fn() -> Step>) {
    let ctx = ctx();
    let state = page_state();
    if let Some(old) = state.borrow_mut().run.take() {
        old.job.stop();
    }
    crate::pages::games::stop_for_other(app);
    let panels = ctx.selected_panels();
    let count = if panels.is_empty() { 2 } else { panels.len() };
    let layout = Span::load();
    let width = layout.picture_width(count);
    let anim = Anim::new(width, Rng::from_time(), make());
    let ports: Vec<String> = panels.iter().map(|p| p.device.clone()).collect();
    // Ends a clock, equalizer or game on these panels, and any earlier animation
    let job = jobs::start(&ports);
    let shared = Arc::new(Mutex::new(Shared::default()));
    {
        let mut s = state.borrow_mut();
        s.run = Some(Run { job: job.clone(), shared: shared.clone(), shown: 0 });
        s.last = Some((title.to_string(), make));
    }
    let page = app.global::<MathPage>();
    page.set_running(true);
    page.set_preview_width(preview::size(count).0 as f32);
    page.set_note(if panels.is_empty() { "No panel is ticked, so this only shows here.".into() } else { "".into() });
    let where_ = panels.iter().map(|p| p.called()).collect::<Vec<_>>().join(" and ");
    show(app, if panels.is_empty() { title.to_string() } else { format!("{title}, on the {where_}.") });
    let parts = ctx.clone_parts();
    std::thread::spawn(move || run(anim, layout, count, job, shared, panels, parts));
}

/// The Math switch turned on: shows the last picture again, or the effect picked on the page.
/// With nothing played yet, starts the effect chosen in the page's effect list.
pub fn switch_on(app: &AppWindow) {
    let last = page_state().borrow().last.clone();
    match last {
        Some((title, make)) => play(app, &title, make),
        None => app.global::<MathPage>().invoke_start_effect(),
    }
}

/// Stops the animation, if one runs. Other pages call this before using the panels.
/// Only stops the thread; the panels stay borrowed for whatever the caller does next.
pub fn stop_for_other(app: &AppWindow) {
    if let Some(run) = page_state().borrow_mut().run.take() {
        run.job.stop();
        app.global::<MathPage>().set_running(false);
    }
}

/// Runs on a UI timer every `PREVIEW_SECONDS`: shows any note or error from the animation thread and puts
/// a new frame in the preview (only while the Math page is showing, to save drawing work).
fn tick(app: &AppWindow) {
    let state = page_state();
    let mut s = state.borrow_mut();
    let Some(run) = s.run.as_mut() else { return };
    // Copy out of the mailbox and release the lock straight away.
    let (frame, note, error) = {
        let mut sh = run.shared.lock().unwrap();
        let frame = (sh.version != run.shown).then(|| (sh.version, sh.frame.clone()));
        (frame, sh.note.take(), sh.error.take())
    };
    if let Some(note) = note {
        show(app, note);
    }
    if let Some(e) = error {
        show(app, e);
    }
    if let Some((version, Some(frame))) = frame {
        run.shown = version;
        if app.global::<crate::App>().get_page() == "math" {
            app.global::<MathPage>().set_preview(render(&frame));
        }
    }
}

/// A number typed in a text box; anything that isn't a finite number gives `default`.
fn number(text: &str, default: f64) -> f64 {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(default)
}

/// Wraps a handler that needs the window into a plain Slint callback; does nothing once the window is gone.
fn with_app(f: impl Fn(&AppWindow) + 'static) -> impl Fn() + 'static {
    move || {
        if let Some(app) = ctx().ui.upgrade() {
            f(&app);
        }
    }
}

/// Shows the saved "Both panels" layout and keyboard-gap width on the page.
fn load_span(app: &AppWindow) {
    let s = Span::load();
    let page = app.global::<MathPage>();
    page.set_layout(s.mode_index() as i32);
    page.set_gap(s.gap_columns as i32);
}

/// Fills the page's lists from `maths.rs`, starts the preview timer and connects every callback (see the
/// file header). Runs once at start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<MathPage>();
    let strings =
        |v: Vec<&str>| ModelRc::new(VecModel::from(v.into_iter().map(SharedString::from).collect::<Vec<_>>()));
    page.set_graphs(strings(maths::GRAPHS.iter().map(|g| g.0).collect()));
    let mut rules: Vec<&str> = maths::LIFE_RULES.iter().map(|r| r.0).collect();
    rules.push(CUSTOM_RULE);
    page.set_rules(strings(rules));
    page.set_starts(strings(maths::STARTS.to_vec()));
    page.set_gas_starts(strings(maths::GAS_STARTS.to_vec()));
    page.set_kinds(strings(maths::AUTOMATA_KINDS.to_vec()));
    page.set_automaton_starts(strings(maths::AUTOMATA_STARTS.to_vec()));
    page.set_effects(strings(maths::EFFECTS.iter().map(|e| e.0).collect()));
    page.set_effect_about(strings(maths::EFFECTS.iter().map(|e| e.3).collect()));
    page.set_layouts(strings(span::MODES.iter().map(|(_, l)| *l).collect()));
    page.set_speed(SPEED.load(Ordering::Relaxed) as i32);
    page.set_preview(render(&Pic::new(2 * PANEL_WIDTH)));
    load_span(app);
    // The Games page can change the layout too
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "math" {
            load_span(app);
        }
    }));
    {
        let ui = ctx.ui.clone();
        ctx.every(PREVIEW_SECONDS, move || {
            if let Some(app) = ui.upgrade() {
                tick(&app);
            }
        });
    }
    // 1 to 30 steps a second; the animation thread reads it before each step.
    page.on_speed_changed(|v| SPEED.store(v.clamp(1, 30) as u32, Ordering::Relaxed));
    // A built-in graph: (name, formula, x range, optional fixed y range). The built-in formulas are
    // known to parse, hence the `expect`.
    page.on_show_graph(with_app(|app| {
        let page = app.global::<MathPage>();
        let Some(&(name, text, x_from, x_to, y_range)) = maths::GRAPHS.get(page.get_graph().max(0) as usize) else {
            return;
        };
        let axes = page.get_axes();
        play(
            app,
            &format!("{name}: y = {text}"),
            Rc::new(move || maths::graph(Formula::parse(text).expect("built in"), x_from, x_to, y_range, axes)),
        );
    }));
    // The user's own formula: a parse error is shown as it is (it says what is wrong). The x range
    // defaults to -10..10 when a box is empty or not a number.
    page.on_show_formula(with_app(|app| {
        let page = app.global::<MathPage>();
        let text = page.get_formula().to_string();
        let f = match Formula::parse(&text) {
            Ok(f) => f,
            Err(e) => return show(app, e.0),
        };
        let (x_from, x_to) = (number(&page.get_x_from(), -10.0), number(&page.get_x_to(), 10.0));
        if x_from >= x_to {
            return show(app, "The 'x from' number has to be smaller than the 'to' number.");
        }
        let axes = page.get_axes();
        play(app, &format!("y = {text}"), Rc::new(move || maths::graph(f.clone(), x_from, x_to, None, axes)));
    }));
    page.on_random_graphs(with_app(|app| {
        let axes = app.global::<MathPage>().get_axes();
        play(
            app,
            "Random graphs",
            Rc::new(move || {
                // A graph lasts about three seconds at the speed chosen
                let each = Arc::new(AtomicUsize::new((SPEED.load(Ordering::Relaxed) as usize * 3).max(8)));
                maths::random_graphs(each, axes)
            }),
        );
    }));
    // Choosing a named Life rule fills in its birth and survival counts; choosing "your own rule"
    // (past the end of the list) leaves the boxes for the user to type in.
    let rule_chosen = with_app(|app| {
        let page = app.global::<MathPage>();
        if let Some((_, birth, survive)) = maths::LIFE_RULES.get(page.get_rule().max(0) as usize) {
            page.set_birth((*birth).into());
            page.set_survive((*survive).into());
        }
    });
    page.on_rule_chosen(move |_| rule_chosen());
    page.on_start_life(with_app(|app| {
        let page = app.global::<MathPage>();
        let (birth, survive) = match (maths::parse_counts(&page.get_birth()), maths::parse_counts(&page.get_survive()))
        {
            (Ok(b), Ok(s)) => (b, s),
            (Err(e), _) | (_, Err(e)) => return show(app, e),
        };
        // With no birth counts nothing new is ever born, and the picture would just die out.
        if !birth.iter().any(|b| *b) {
            return show(app, "Type at least one number for 'Born with', or nothing new will ever appear.");
        }
        let start = maths::STARTS.get(page.get_start().max(0) as usize).copied().unwrap_or("Random").to_string();
        let rule = page.get_rule().max(0) as usize;
        let name = match maths::LIFE_RULES.get(rule) {
            Some(r) => r.0.to_string(),
            None => format!("B{}/S{}", maths::counts_text(&birth), maths::counts_text(&survive)),
        };
        // 0.35: the share of cells alive at a random start.
        play(app, &name, Rc::new(move || maths::life(birth, survive, start.clone(), 0.35)));
    }));
    // The traffic slider is a percentage of cells that hold a car.
    page.on_start_traffic(with_app(|app| {
        let density = app.global::<MathPage>().get_traffic() as f64 / 100.0;
        play(app, "Traffic", Rc::new(move || maths::traffic(density)));
    }));
    page.on_start_gas(with_app(|app| {
        let page = app.global::<MathPage>();
        let start =
            maths::GAS_STARTS.get(page.get_gas_start().max(0) as usize).copied().unwrap_or("Random").to_string();
        let density = page.get_gas() as f64 / 100.0;
        play(app, "Gas particles", Rc::new(move || maths::gas(start.clone(), density)));
    }));
    // A good number to start from for each kind of automaton: rule 30 (elementary), code 777
    // (3-colour totalistic).
    let kind_chosen = with_app(|app| {
        let page = app.global::<MathPage>();
        page.set_number(if page.get_kind() == 1 { 777 } else { 30 });
    });
    page.on_kind_chosen(move |_| kind_chosen());
    page.on_start_automaton(with_app(|app| {
        let page = app.global::<MathPage>();
        let totalistic = page.get_kind() == 1;
        // Elementary rules are 8-bit numbers (0..255); 3-colour totalistic codes have 7 base-3 digits
        // (0..3^7-1 = 2186).
        let top = if totalistic { 2186 } else { 255 };
        let value = page.get_number();
        if !(0..=top).contains(&value) {
            return show(app, format!("Pick a number from 0 to {top}."));
        }
        let random = page.get_automaton_start() == 1;
        let label = if totalistic { format!("Code {value}") } else { format!("Rule {value}") };
        play(app, &label, Rc::new(move || maths::automaton(totalistic, value as u32, random)));
    }));
    page.on_start_effect(with_app(|app| {
        let page = app.global::<MathPage>();
        let Some(&(name, make, speed, _)) = maths::EFFECTS.get(page.get_effect().max(0) as usize) else { return };
        // Each one has a speed it looks best at; the slider still changes it afterwards
        SPEED.store(speed, Ordering::Relaxed);
        page.set_speed(speed as i32);
        play(app, name, Rc::new(make));
    }));
    page.on_noise(with_app(|app| play(app, "Random grey noise", Rc::new(maths::noise))));
    page.on_gap_test(with_app(|app| play(app, "Gap test: a dot going across both panels", Rc::new(maths::gap_test))));
    // Layout or gap width changed: the gap is clamped to what `span.rs` allows. Nothing happens unless
    // it actually differs from what's saved; otherwise it is saved and a running picture restarts.
    page.on_span_changed(with_app(|app| {
        let page = app.global::<MathPage>();
        let mut s = Span::load();
        s.mode = span::MODES.get(page.get_layout().max(0) as usize).map(|m| m.0).unwrap_or(span::SIDE_BY_SIDE).into();
        s.gap_columns = page.get_gap().clamp(span::MIN_GAP as i32, span::MAX_GAP as i32) as usize;
        page.set_gap(s.gap_columns as i32);
        if s == Span::load() {
            return;
        }
        s.save();
        // Start whatever is showing again with the new layout
        let again = {
            let st = page_state();
            let st = st.borrow();
            if st.run.is_some() {
                st.last.clone()
            } else {
                None
            }
        };
        if let Some((title, make)) = again {
            play(app, &title, make);
        }
    }));
    // Stop: end the animation and give the ticked panels back to the monitor if they were borrowed.
    page.on_stop(with_app(|app| {
        stop_for_other(app);
        let ctx = crate::app::ctx();
        let panels = ctx.selected_panels();
        if panels.iter().any(|p| ctx.lender.is_borrowed(&p.device)) {
            ctx.clone_parts().give_back(panels, None);
            show(app, STOPPED);
        } else {
            show(app, "Stopped.");
        }
    }));
}
