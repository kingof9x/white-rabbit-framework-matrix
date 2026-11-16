//! Games & patterns (PlayTab in `tabs.py` and `games_tab.py`): the games played on the panels with the
//! arrow keys, the firmware's Game of Life and patterns, text, and the clock, countdown and random
//! equalizer. Everything here borrows the ticked panels from the system monitor.
//!
//! A game runs on a worker thread and sends each new picture to the ticked panels in greyscale. The
//! keys go to the picture in the window; the game pauses by itself when the picture loses the keyboard,
//! so clicking somewhere else never loses you a life.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::games::{self, Held, Key, Kind, Pic, Rng, Session, KINDS, PANEL_WIDTH};
use crate::panels::{Panel, Sender};
use crate::span::{self, Span};
use crate::{jobs, preview, settings, AppWindow, GamesPage};

const FRAME_SECONDS: f64 = 1.0 / 30.0;
const PREVIEW_SECONDS: f64 = 0.04;
const STOPPED: &str = "Stopped. The system monitor has the panels again.";

/// (name, firmware number) of the Game of Life starts (`GAME_OF_LIFE` in `tabs.py`)
pub const GAME_OF_LIFE: [(&str, u8); 20] = [
    ("Current picture", 0),
    ("Pattern 1", 1),
    ("Pattern 2", 17),
    ("Pattern 3", 18),
    ("Pattern 4", 19),
    ("Pattern 5", 20),
    ("Blinker", 2),
    ("Toad", 3),
    ("Beacon", 4),
    ("Glider", 5),
    ("R-pentomino", 7),
    ("Acorn", 8),
    ("Diehard", 9),
    ("Light spaceship", 10),
    ("Medium spaceship", 11),
    ("Heavy spaceship", 12),
    ("Spaceship fleet", 13),
    ("Octagon", 14),
    ("Tumbler", 15),
    ("Caterer", 16),
];

enum Event {
    Key(Key),
    Pause(bool),
}

/// What the game thread tells the window.
#[derive(Clone, Default, PartialEq)]
struct Info {
    score: u32,
    best: u32,
    level: Option<u32>,
    paused: bool,
    over: bool,
    finished: bool,
    new_best: bool,
}

#[derive(Default)]
struct Shared {
    events: VecDeque<Event>,
    held: Held,
    frame: Option<Pic>,
    version: u64,
    info: Info,
    error: Option<String>,
}

struct Run {
    kind: Kind,
    job: jobs::Job,
    shared: Arc<Mutex<Shared>>,
    panels: Vec<Panel>,
    shown_version: u64,
    shown_info: Option<Info>,
    saved: bool,
}

#[derive(Default)]
struct State {
    run: Option<Run>,
    scores: BTreeMap<String, u32>,
    /// Whether the game should be paused, as the window sees it (the game thread catches up)
    want_paused: bool,
    /// The last game started, for the Games & patterns switch
    last: Option<Kind>,
}

thread_local! {
    static STATE: Rc<RefCell<State>> = Rc::default();
}

fn page_state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.clone())
}

fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<GamesPage>().set_status(text.into());
}

fn status_fn() -> crate::app::Status {
    Arc::new(|app: &AppWindow, text: String| show(app, text))
}

fn best_text(scores: &BTreeMap<String, u32>) -> String {
    let parts: Vec<String> = KINDS
        .iter()
        .filter_map(|k| scores.get(k.key()).filter(|s| **s > 0).map(|s| format!("{} {s}", k.title())))
        .collect();
    if parts.is_empty() {
        "Best scores show up here once you've played.".into()
    } else {
        format!("Best scores: {}", parts.join(", "))
    }
}

/// The picture as the panels' preview.
pub fn render(frame: &Pic) -> slint::Image {
    let count = (frame.w / PANEL_WIDTH).max(1);
    let mut panels = vec![preview::dark(); count];
    for (p, levels) in panels.iter_mut().enumerate() {
        for (x, col) in levels.iter_mut().enumerate() {
            for (y, v) in col.iter_mut().enumerate() {
                *v = frame.get(p * PANEL_WIDTH + x, y);
            }
        }
    }
    preview::render(&panels, 2)
}

/// The commands that draw this frame on each panel (9 columns each, left to right).
pub fn frame_writes(frame: &Pic, panels: &[Panel]) -> Vec<(String, Vec<Vec<u8>>)> {
    let bytes = frame.bytes();
    panels
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut grid = [[0u8; cmd::HEIGHT]; cmd::WIDTH];
            for (x, col) in grid.iter_mut().enumerate() {
                for (y, v) in col.iter_mut().enumerate() {
                    let fx = i * PANEL_WIDTH + x;
                    if fx < frame.w {
                        *v = bytes[y * frame.w + fx];
                    }
                }
            }
            (p.device.clone(), cmd::greyscale(&grid))
        })
        .collect()
}

/// The game loop, on its own thread.
fn play(
    mut session: Session,
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
            let _ = sender.write(&p.device, &[cmd::animate(false)]);
        }
    }
    let mut last = Instant::now();
    let mut sent: Option<Pic> = None;
    while !job.stopped() {
        let held = {
            let mut s = shared.lock().unwrap();
            while let Some(e) = s.events.pop_front() {
                match e {
                    Event::Key(k) => session.press(k),
                    Event::Pause(p) => session.set_paused(p),
                }
            }
            s.held
        };
        let now = Instant::now();
        session.update((now - last).as_secs_f64().min(0.1), held);
        last = now;
        let frame = session.frame();
        let changed = sent.as_ref().map(|s| s.bytes() != frame.bytes()).unwrap_or(true);
        let leading = lead.as_ref().is_some_and(|l: &crate::lead_in::LeadIn| l.active());
        if changed || leading {
            if !panels.is_empty() {
                let writes = match lead.as_mut() {
                    Some(l) if leading => l.writes(&frame, &panels),
                    _ => {
                        lead = None;
                        frame_writes(&frame, &panels)
                    }
                };
                if let Err(e) = sender.write_many(&writes) {
                    wr_core::warn!("The game stopped: {e}");
                    shared.lock().unwrap().error = Some(format!("The game stopped: {e}"));
                    return;
                }
            }
            sent = Some(frame.clone());
        }
        {
            let mut s = shared.lock().unwrap();
            if changed {
                s.frame = Some(frame);
                s.version += 1;
            }
            s.info = Info {
                score: session.game.score(),
                best: session.best,
                level: session.game.level(),
                paused: session.paused,
                over: session.game.over(),
                finished: session.finished(),
                new_best: session.new_best,
            };
        }
        let spare = FRAME_SECONDS - now.elapsed().as_secs_f64();
        if spare > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(spare));
        }
    }
}

fn start(app: &AppWindow, kind: Kind) {
    let ctx = ctx();
    let state = page_state();
    if let Some(old) = state.borrow_mut().run.take() {
        old.job.stop();
    }
    crate::pages::math::stop_for_other(app);
    let panels = ctx.selected_panels();
    let count = if panels.is_empty() { 2 } else { panels.len() };
    let layout = if kind.spans() { Span::load() } else { Span::default() };
    let width = layout.picture_width(count);
    state.borrow_mut().last = Some(kind);
    let best = state.borrow().scores.get(kind.key()).copied().unwrap_or(0);
    let to_panels: Box<dyn Fn(&Pic) -> Pic + Send> = Box::new(move |p: &Pic| layout.to_panels(p, count));
    let session = Session::new(kind.make(width, Rng::from_time()), best, to_panels, PANEL_WIDTH * count);
    let ports: Vec<String> = panels.iter().map(|p| p.device.clone()).collect();
    // Ends a clock, equalizer or Math animation, and any earlier game, on these panels
    let job = jobs::start(&ports);
    let shared = Arc::new(Mutex::new(Shared::default()));
    {
        let mut s = state.borrow_mut();
        s.want_paused = false;
        s.run = Some(Run {
            kind,
            job: job.clone(),
            shared: shared.clone(),
            panels: panels.clone(),
            shown_version: 0,
            shown_info: None,
            saved: false,
        });
    }
    let page = app.global::<GamesPage>();
    page.set_title(kind.title().into());
    page.set_running(true);
    page.set_paused(false);
    page.set_can_pause(true);
    page.set_preview_width(preview::size(count).0 as f32);
    page.set_controls(
        format!(
            "{} Esc pauses.{}",
            kind.controls(),
            if panels.is_empty() { " No panel is ticked, so it only shows here." } else { "" }
        )
        .into(),
    );
    let where_ = panels.iter().map(|p| p.called()).collect::<Vec<_>>().join(" and ");
    show(
        app,
        if panels.is_empty() {
            format!("Playing {} in the window.", kind.title())
        } else {
            format!("Playing {} on the {where_}.", kind.title())
        },
    );
    page.set_score("Get ready...".into());
    page.set_focus_preview(page.get_focus_preview() + 1);
    let parts = ctx.clone_parts();
    std::thread::spawn(move || play(session, job, shared, panels, parts));
}

/// The Games & patterns switch turned on: plays the last game again (Snake the first time).
pub fn switch_on(app: &AppWindow) {
    let last = page_state().borrow().last;
    if let Some(kind) = last.or_else(|| KINDS.first().copied()) {
        start(app, kind);
    }
}

/// Stops the game, if one runs, for another page that uses the panels.
pub fn stop_for_other(app: &AppWindow) {
    if page_state().borrow().run.is_some() {
        stop_game(app);
    }
}

/// Stops the game (if any). Returns the panels it played on.
fn stop_game(app: &AppWindow) -> Vec<Panel> {
    let state = page_state();
    let run = state.borrow_mut().run.take();
    let page = app.global::<GamesPage>();
    page.set_title("No game running".into());
    page.set_score("Pick a game on the left.".into());
    page.set_controls("".into());
    page.set_running(false);
    page.set_paused(false);
    page.set_can_pause(false);
    match run {
        Some(run) => {
            run.job.stop();
            run.panels
        }
        None => Vec::new(),
    }
}

fn set_paused(app: &AppWindow, paused: bool) {
    let state = page_state();
    let mut s = state.borrow_mut();
    let Some(run) = &s.run else { return };
    if run.shown_info.as_ref().is_some_and(|i| i.over) {
        return;
    }
    run.shared.lock().unwrap().events.push_back(Event::Pause(paused));
    s.want_paused = paused;
    drop(s);
    let page = app.global::<GamesPage>();
    page.set_paused(paused);
    if paused {
        page.set_score("Paused. Press Esc or P (or click the game) to carry on.".into());
    } else {
        page.set_focus_preview(page.get_focus_preview() + 1);
    }
}

/// The labels, the preview and the high score, from what the game thread said.
fn tick(app: &AppWindow) {
    let state = page_state();
    let mut guard = state.borrow_mut();
    let s = &mut *guard;
    let want_paused = s.want_paused;
    let Some(run) = s.run.as_mut() else { return };
    let (frame, info, error) = {
        let mut sh = run.shared.lock().unwrap();
        let frame = (sh.version != run.shown_version).then(|| (sh.version, sh.frame.clone()));
        (frame, sh.info.clone(), sh.error.take())
    };
    let page = app.global::<GamesPage>();
    if let Some(e) = error {
        show(app, e);
    }
    if let Some((version, Some(frame))) = frame {
        run.shown_version = version;
        if app.global::<crate::App>().get_page() == "games" {
            page.set_preview(render(&frame));
        }
    }
    if run.shown_info.as_ref() == Some(&info) {
        return;
    }
    run.shown_info = Some(info.clone());
    let text = if want_paused && !info.over {
        "Paused. Press Esc or P (or click the game) to carry on.".to_string()
    } else if info.over {
        format!(
            "Game over: {}.{} Press space to play again.",
            info.score,
            if info.new_best { " A new best!".to_string() } else { format!(" Best: {}.", info.best) }
        )
    } else {
        let level = info.level.map(|l| format!("   Level {l}")).unwrap_or_default();
        format!("Score {}   Best {}{level}", info.score, info.best)
    };
    page.set_score(text.into());
    page.set_can_pause(!info.over);
    if info.over && !run.saved {
        run.saved = true;
        let key = run.kind.key();
        if games::save_high_score(key, info.score) {
            s.scores.insert(key.into(), info.score);
            page.set_best(best_text(&s.scores).into());
        }
    }
}

fn key(app: &AppWindow, name: &str, down: bool) {
    if name == "pause" {
        let paused = page_state().borrow().want_paused;
        set_paused(app, !paused);
        return;
    }
    let k = match name {
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        _ => Key::Action,
    };
    let state = page_state();
    let (want_paused, finished, kind) = {
        let s = state.borrow();
        let Some(run) = &s.run else { return };
        let mut sh = run.shared.lock().unwrap();
        match k {
            Key::Left => sh.held.left = down,
            Key::Right => sh.held.right = down,
            Key::Down => sh.held.down = down,
            _ => {}
        }
        if !down {
            return;
        }
        let finished = sh.info.finished;
        if !s.want_paused && !finished {
            sh.events.push_back(Event::Key(k));
        }
        (s.want_paused, finished, run.kind)
    };
    if want_paused {
        set_paused(app, false);
    } else if finished && k == Key::Action {
        start(app, kind);
    }
}

/// Firmware things (Game of Life, patterns, text, fill) end a game first.
fn firmware_act(app: &AppWindow, action: impl Fn(&Sender, &Panel) -> Result<(), String> + Send + Sync + 'static) {
    stop_game(app);
    crate::pages::math::stop_for_other(app);
    ctx().act(
        move |s, p| {
            jobs::stop_panel_jobs(std::slice::from_ref(&p.device));
            action(s, p)
        },
        status_fn(),
        None,
        true,
        None,
        false,
    );
}

/// The clock, countdown and equalizer: one thread per panel, until something else takes the panel.
fn per_panel(app: &AppWindow, what: &'static str, seconds: u64) {
    stop_game(app);
    crate::pages::math::stop_for_other(app);
    let sender = ctx().sender.clone();
    ctx().act(
        move |s, p| {
            let job = jobs::start(std::slice::from_ref(&p.device));
            s.write(&p.device, &[cmd::animate(false)])?;
            let device = p.device.clone();
            let sender = sender.clone();
            std::thread::spawn(move || {
                let result = match what {
                    "clock" => clock(&sender, &device, &job),
                    "countdown" => countdown(&sender, &device, &job, seconds),
                    _ => equalizer(&sender, &device, &job),
                };
                if let Err(e) = result {
                    wr_core::info!("{what} on {device} stopped: {e}");
                }
            });
            Ok(())
        },
        status_fn(),
        None,
        true,
        None,
        false,
    );
}

fn clock(sender: &Sender, device: &str, job: &jobs::Job) -> Result<(), String> {
    let mut shown = String::new();
    while !job.stopped() {
        let now = chrono::Local::now().format("%H:%M").to_string();
        if now != shown {
            sender.write(device, &[cmd::show_string(&now)])?;
            shown = now;
        }
        if !jobs::sleep(job, 1.0) {
            break;
        }
    }
    Ok(())
}

/// More LEDs light up until the time is over and all are lit, then the panel breathes.
fn countdown(sender: &Sender, device: &str, job: &jobs::Job, seconds: u64) -> Result<(), String> {
    let start = Instant::now();
    let total = seconds.max(1) as f64;
    let mut lit = usize::MAX;
    loop {
        let passed = start.elapsed().as_secs_f64();
        if passed >= total {
            break;
        }
        let leds = (306.0 * passed / total) as usize;
        if leds != lit {
            sender.write(device, &[cmd::light_leds(leds)])?;
            lit = leds;
        }
        if !jobs::sleep(job, 0.01) {
            return Ok(());
        }
    }
    sender.write(device, &[cmd::light_leds(306)])?;
    // Breathing: bright ranges look alike, so go through those faster
    while !job.stopped() {
        let steps: Vec<(f64, u8)> = (0..10)
            .map(|i| (0.03, 250 - i * 20))
            .chain((0..10).map(|i| (0.06, 50 - i * 5)))
            .chain((0..10).map(|i| (0.06, i * 5)))
            .chain((0..10).map(|i| (0.03, 50 + i * 20)))
            .collect();
        for (wait, level) in steps {
            if !jobs::sleep(job, wait) {
                return Ok(());
            }
            sender.write(device, &[cmd::brightness(level)])?;
        }
    }
    Ok(())
}

/// An equalizer with random values, lower ones more likely.
fn equalizer(sender: &Sender, device: &str, job: &jobs::Job) -> Result<(), String> {
    let mut rng = Rng::from_time();
    let weights: Vec<usize> = (1..=33).rev().map(|i| i * i).collect();
    let total: usize = weights.iter().sum();
    while !job.stopped() {
        let vals: Vec<usize> = (0..9)
            .map(|_| {
                let mut r = rng.below(total);
                for (i, w) in weights.iter().enumerate() {
                    if r < *w {
                        return i + 1;
                    }
                    r -= w;
                }
                33
            })
            .collect();
        sender.write(device, &[cmd::eq(&vals)])?;
        if !jobs::sleep(job, 0.2) {
            break;
        }
    }
    Ok(())
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<GamesPage>();
    let strings =
        |v: Vec<&str>| ModelRc::new(VecModel::from(v.into_iter().map(SharedString::from).collect::<Vec<_>>()));
    page.set_games(strings(KINDS.iter().map(|k| k.title()).collect()));
    page.set_life_starts(strings(GAME_OF_LIFE.iter().map(|(n, _)| *n).collect()));
    page.set_patterns(strings(cmd::PATTERNS.to_vec()));
    page.set_layouts(strings(span::MODES.iter().map(|(_, l)| *l).collect()));
    page.set_layout(Span::load().mode_index() as i32);
    page.set_preview(render(&Pic::new(2 * PANEL_WIDTH)));
    {
        let scores = games::high_scores(&settings::load());
        page.set_best(best_text(&scores).into());
        page_state().borrow_mut().scores = scores;
    }
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "games" {
            app.global::<GamesPage>().set_layout(Span::load().mode_index() as i32);
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
    let ui = ctx.ui.clone();
    let with_app = move |f: Box<dyn Fn(&AppWindow)>| {
        let ui = ui.clone();
        move || {
            if let Some(app) = ui.upgrade() {
                f(&app);
            }
        }
    };
    page.on_start(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        if let Some(kind) = KINDS.get(i as usize) {
            start(&app, *kind);
        }
    });
    page.on_pause(with_app(Box::new(|app| {
        let paused = page_state().borrow().want_paused;
        set_paused(app, !paused);
    })));
    page.on_stop(with_app(Box::new(|app| {
        stop_game(app);
        let ctx = crate::app::ctx();
        let panels = ctx.selected_panels();
        let ports: Vec<String> = panels.iter().map(|p| p.device.clone()).collect();
        jobs::stop_panel_jobs(&ports);
        if panels.iter().any(|p| ctx.lender.is_borrowed(&p.device)) {
            ctx.clone_parts().give_back(panels, None);
            show(app, STOPPED);
        } else {
            show(app, "Stopped.");
        }
    })));
    page.on_key(|name, down| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            key(&app, &name, down);
        }
    });
    page.on_preview_clicked(with_app(Box::new(|app| {
        if page_state().borrow().want_paused {
            set_paused(app, false);
        }
    })));
    page.on_preview_focus(|has| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        if has {
            return;
        }
        let state = page_state();
        let running = {
            let s = state.borrow();
            match &s.run {
                Some(run) => {
                    let mut sh = run.shared.lock().unwrap();
                    sh.held = Held::default();
                    !sh.info.over
                }
                None => false,
            }
        };
        if running {
            set_paused(&app, true);
        }
    });
    page.on_layout_chosen(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let mut layout = Span::load();
        layout.mode = span::MODES.get(i as usize).map(|(k, _)| *k).unwrap_or(span::SIDE_BY_SIDE).into();
        layout.save();
        // A game that's running starts again with the new layout
        let again = page_state().borrow().run.as_ref().and_then(|r| {
            let over = r.shared.lock().unwrap().info.over;
            (r.kind.spans() && !over).then_some(r.kind)
        });
        if let Some(kind) = again {
            start(&app, kind);
        }
    });
    page.on_life(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some(&(_, start)) = GAME_OF_LIFE.get(i as usize) else { return };
        firmware_act(&app, move |s, p| s.write(&p.device, &[cmd::game_of_life(start)]));
    });
    page.on_pattern(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some(name) = cmd::PATTERNS.get(i as usize) else { return };
        let writes = cmd::pattern(name);
        firmware_act(&app, move |s, p| {
            let mut all = vec![cmd::game_control(cmd::GAME_QUIT), cmd::animate(false)];
            all.extend(writes.iter().cloned());
            s.write(&p.device, &all)
        });
    });
    page.on_show_text(|text| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let text = text.to_uppercase();
        firmware_act(&app, move |s, p| {
            s.write(&p.device, &[cmd::game_control(cmd::GAME_QUIT), cmd::animate(false), cmd::show_string(&text)])
        });
    });
    page.on_fill_changed(|v| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let v = v.clamp(0, 100) as u8;
        firmware_act(&app, move |s, p| {
            s.write(&p.device, &[cmd::game_control(cmd::GAME_QUIT), cmd::animate(false), cmd::percentage(v)])
        });
    });
    page.on_scroll(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        firmware_act(&app, |s, p| s.write(&p.device, &[cmd::animate(true)]));
    });
    page.on_clock(|| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            per_panel(&app, "clock", 0);
        }
    });
    page.on_countdown(|seconds| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            per_panel(&app, "countdown", seconds.clamp(1, 600) as u64);
        }
    });
    page.on_equalizer(|| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            per_panel(&app, "eq", 0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_per_panel() {
        let mut p = Pic::new(18);
        p.set(9, 0, 1.0);
        let panels = vec![
            Panel { side: "left".into(), device: "COM4".into(), location: None, nickname: String::new() },
            Panel { side: "right".into(), device: "COM5".into(), location: None, nickname: String::new() },
        ];
        let w = frame_writes(&p, &panels);
        assert_eq!(w.len(), 2);
        // First column of the right panel, top row lit
        assert_eq!(w[1].1[0][4], 255);
        assert_eq!(w[1].1[0][5], 0);
        assert_eq!(w[0].1[0][4], 0);
    }

    #[test]
    fn best_scores_text() {
        let mut s = BTreeMap::new();
        assert!(best_text(&s).starts_with("Best scores show up"));
        s.insert("ledris".into(), 12);
        assert_eq!(best_text(&s), "Best scores: Ledris 12");
    }
}
