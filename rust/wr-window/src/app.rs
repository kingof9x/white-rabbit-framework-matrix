//! The window's shared state and the things every page does: which panels the controls act on, doing
//! something on the panels on a worker thread, borrowing panels from the monitor and giving them back
//! (`white_rabbit/window.py`).
//!
//! # Threads: the UI thread and workers
//!
//! Slint (the GUI toolkit) runs one event loop on the main thread, and the window and everything in it
//! (`AppWindow`, its globals and models) may only be touched there. Talking to a panel or the monitor
//! can take seconds, which would freeze the window, so it never happens on that thread. Instead:
//!
//! - `Ctx::in_background(work, done)` runs `work` on a new thread, then hands its result to `done` on
//!   the UI thread through `slint::Weak::upgrade_in_event_loop` (which queues a closure onto the event
//!   loop; if the window has closed the closure is dropped, which is why its error is ignored).
//! - `Ctx::later` does just the second half, from any thread.
//! - `Ctx::after` and `Ctx::every` are Slint timers, so their closures run on the UI thread.
//!
//! `Ctx` itself holds `Rc` and Slint handles and so stays on the UI thread (one copy, kept in a
//! thread-local and handed to the pages). The parts a worker needs are all `Arc`s; `Ctx::clone_parts`
//! copies them into a `Parts`, which can be moved to another thread.
//!
//! # Panels and the monitor
//!
//! `Ctx::act` is how most buttons reach the panels: on a worker it borrows the ticked panels from the
//! monitor (`Lender::borrow`), runs the action on each, reports "Done." or what failed, and can give
//! them back after a delay. A 5-second timer (set up in `wire`) asks the monitor whether it is running
//! and whether audio reactivity is on, and refreshes the bar at the bottom of the menu and Home's
//! panel cards (`Parts::update_bar`).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::commands as cmd;
use crate::link::{request, Link};
use crate::panels::{find_panels, Generations, Lender, Panel, Sender};
use crate::{settings, theme, App, AppWindow, PanelRow};

/// What a panel told us about itself.
///
/// Filled by `read_modules` when the window opens (and after firmware changes); the Firmware, Startup
/// and Home pages read it.
#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    /// The firmware version text, e.g. "0.2.0"; None when it couldn't be read
    pub version: Option<String>,
    /// White Rabbit firmware's build number, when the firmware reports one
    pub build: Option<u16>,
    /// (mode, animation, set) when the firmware has the startup animation setting
    pub startup: Option<(u8, u8, u64)>,
    /// Another program had the port open
    pub in_use: bool,
}

/// Shared by the pages; the parts used on worker threads are behind Arc.
///
/// Lives on the UI thread only (see the file header); `ctx()` returns it from anywhere on that thread.
pub struct Ctx {
    /// The window, held weakly so this doesn't keep it alive; `upgrade()` is None once it has closed
    pub ui: slint::Weak<AppWindow>,
    /// The monitor's control channel, with its 2-second "is it running" cache
    pub link: Arc<Link>,
    /// Sends panel commands (through the monitor or straight to the port)
    pub sender: Arc<Sender>,
    /// Which panels are borrowed from the monitor, and how to give them back
    pub lender: Arc<Lender>,
    /// The panels found when the window opened, left first
    pub panels: Arc<Mutex<Vec<Panel>>>,
    /// One per panel, same order: ticked under "Controls act on" (all ticked at the start)
    pub checked: Arc<Mutex<Vec<bool>>>,
    /// What the last 5-second check found: is the monitor running
    pub monitor_running: Arc<AtomicBool>,
    /// Is the monitor's audio reactivity (music mode) on, as it last said
    pub music_on: Arc<AtomicBool>,
    /// Port -> what that panel's firmware said about itself
    pub modules: Arc<Mutex<HashMap<String, ModuleInfo>>>,
    /// Called on the window's thread after the panels' firmware has been read
    pub on_modules: Mutex<Vec<Box<dyn Fn(&AppWindow)>>>,
    /// Called when a page is shown (key)
    pub on_page: Mutex<Vec<Box<dyn Fn(&AppWindow, &str)>>>,
    /// Repeating timers from `every`; a Slint timer stops when dropped, so they are kept here
    pub timers: Mutex<Vec<slint::Timer>>,
}

/// A callback that shows a result text somewhere on a page (called on the UI thread). `Arc` + Send so it
/// can travel with the work to a worker thread and back.
pub type Status = Arc<dyn Fn(&AppWindow, String) + Send + Sync>;

thread_local! {
    /// The one `Ctx`, set by `Ctx::new`. Thread-local because `Rc` can't be shared between threads.
    static CTX: std::cell::RefCell<Option<Rc<Ctx>>> = const { std::cell::RefCell::new(None) };
}

/// The window's state, on the window's thread (for work finishing there).
///
/// Panics if called on any other thread, or before the window is built.
pub fn ctx() -> Rc<Ctx> {
    CTX.with(|c| c.borrow().clone()).expect("the window's state is only on the window's thread")
}

impl Ctx {
    /// Makes the window's state: finds the connected panels (reads the port list and the layout file),
    /// ticks them all, and stores the result in the thread-local for `ctx()`.
    pub fn new(ui: &AppWindow) -> Rc<Ctx> {
        let link = Arc::new(Link::default());
        let panels = find_panels();
        let checked = vec![true; panels.len()];
        let ctx = Rc::new(Ctx {
            ui: ui.as_weak(),
            sender: Arc::new(Sender::new(link.clone())),
            link,
            lender: Arc::new(Lender::default()),
            panels: Arc::new(Mutex::new(panels)),
            checked: Arc::new(Mutex::new(checked)),
            monitor_running: Arc::new(AtomicBool::new(false)),
            music_on: Arc::new(AtomicBool::new(false)),
            modules: Arc::default(),
            on_modules: Mutex::default(),
            on_page: Mutex::default(),
            timers: Mutex::default(),
        });
        // Keep a copy where `ctx()` can find it; it lives until the UI thread ends.
        CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        ctx
    }

    /// A copy of the panel list (so the lock is not held while using it).
    pub fn panels(&self) -> Vec<Panel> {
        self.panels.lock().unwrap().clone()
    }

    /// The panels ticked under "Controls act on", left first.
    pub fn selected_panels(&self) -> Vec<Panel> {
        let checked = self.checked.lock().unwrap();
        self.panels().into_iter().zip(checked.iter()).filter(|(_, c)| **c).map(|(p, _)| p).collect()
    }

    /// Runs work() on a worker thread, then done(result) on the window's thread.
    ///
    /// A new OS thread per call (these are rare and short, so no pool). If the window has closed by the
    /// time `work` finishes, `done` is silently skipped.
    pub fn in_background<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&AppWindow, R) + Send + 'static,
    ) {
        // Only the weak handle goes to the worker: it is Send, and the window may close meanwhile.
        let ui = self.ui.clone();
        std::thread::spawn(move || {
            let result = work();
            // Fails only when the event loop has ended (the window closed); then there is nobody to tell.
            let _ = ui.upgrade_in_event_loop(move |app| done(&app, result));
        });
    }

    /// Something to run on the window's thread from any thread.
    pub fn later(ui: &slint::Weak<AppWindow>, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = ui.upgrade_in_event_loop(move |app| f(&app));
    }

    /// A one-off timer on the window's thread.
    ///
    /// Does nothing if the window has closed by then.
    pub fn after(&self, seconds: f64, f: impl FnOnce(&AppWindow) + 'static) {
        let ui = self.ui.clone();
        slint::Timer::single_shot(Duration::from_secs_f64(seconds), move || {
            if let Some(app) = ui.upgrade() {
                f(&app);
            }
        });
    }

    /// A repeating timer on the window's thread, kept for as long as the window is open.
    ///
    /// `f` runs on the UI thread every `seconds`, the first time after one interval.
    pub fn every(&self, seconds: f64, f: impl FnMut() + 'static) {
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_secs_f64(seconds), f);
        self.timers.lock().unwrap().push(timer);
    }

    /// Runs action(panel) for the chosen panels on a worker thread. borrow: take the panels from the
    /// monitor first. return_after: give them back after that many seconds. status(text) shows the
    /// result. Returns false when no panel is chosen.
    ///
    /// `panels`: None means the ticked ones. `one_panel`: refuse unless exactly one is chosen. The steps,
    /// on the worker: borrow (if asked), refresh the bar so it says the panels are borrowed, then run
    /// `action` on each panel in turn, collecting failures as "left panel: reason". Back on the UI
    /// thread: show "Done." or the failures, and start the give-back timer. The give-back only returns
    /// panels not borrowed again since (it passes the generations from this borrow).
    #[allow(clippy::too_many_arguments)]
    pub fn act(
        self: &Rc<Self>,
        action: impl Fn(&Sender, &Panel) -> Result<(), String> + Send + Sync + 'static,
        status: Status,
        panels: Option<Vec<Panel>>,
        borrow: bool,
        return_after: Option<f64>,
        one_panel: bool,
    ) -> bool {
        // Checks first, on the UI thread, so the user gets the message at once.
        let panels = panels.unwrap_or_else(|| self.selected_panels());
        let Some(app) = self.ui.upgrade() else { return false };
        if panels.is_empty() {
            status(&app, "Tick a panel under \"Controls act on\", bottom left, first.".into());
            return false;
        }
        if one_panel && panels.len() != 1 {
            status(&app, "Tick exactly one panel under \"Controls act on\", bottom left, for this.".into());
            return false;
        }
        let (sender, lender, ui) = (self.sender.clone(), self.lender.clone(), self.ui.clone());
        // `Parts` can cross threads; `self` (an Rc) can't.
        let me = self.clone_parts();
        let worker = me.clone();
        self.in_background(
            move || {
                // Borrowing talks to the monitor and reads each panel's brightness, so it is done here on the
                // worker.
                let generations = borrow.then(|| lender.borrow(&sender, &panels));
                let me2 = worker.clone();
                // The bar now says "Showing White Rabbit controls on ..." while the action runs.
                Ctx::later(&ui, move |app| me2.update_bar(app));
                // Every panel is tried even if one fails, so one unplugged panel doesn't stop the other.
                let problems: Vec<String> = panels
                    .iter()
                    .filter_map(|p| action(&sender, p).err().map(|e| format!("{}: {e}", p.called())))
                    .collect();
                (generations, problems, panels)
            },
            move |app, (generations, problems, panels)| {
                status(
                    app,
                    if problems.is_empty() {
                        "Done.".into()
                    } else {
                        format!("Didn't work on the {}", problems.join("; "))
                    },
                );
                // Give back only what this call borrowed (and only if not borrowed again since): `act` calls
                // without `borrow` have no generations and never give back.
                if let (Some(seconds), Some(generations)) = (return_after, generations) {
                    let me2 = me.clone();
                    slint::Timer::single_shot(Duration::from_secs_f64(seconds), move || {
                        me2.give_back(panels, Some(generations));
                    });
                }
                me.update_bar(app);
            },
        );
        true
    }

    /// The parts of the context a worker thread or timer can keep.
    ///
    /// Cheap: every field is an `Arc` or a weak handle, so this only bumps reference counts.
    pub fn clone_parts(&self) -> Parts {
        Parts {
            ui: self.ui.clone(),
            link: self.link.clone(),
            sender: self.sender.clone(),
            lender: self.lender.clone(),
            panels: self.panels.clone(),
            checked: self.checked.clone(),
            monitor_running: self.monitor_running.clone(),
            music_on: self.music_on.clone(),
            modules: self.modules.clone(),
        }
    }

    /// Refreshes the bottom of the menu and the Home page's panel cards (see `Parts::update_bar`).
    pub fn update_bar(&self, app: &AppWindow) {
        self.clone_parts().update_bar(app)
    }

    /// Gives every borrowed panel back to the monitor, on a worker thread.
    pub fn give_back_all(&self) {
        let p = self.clone_parts();
        p.give_back(self.panels(), None);
    }

    /// Reads each panel's firmware version and startup setting, then tells the pages.
    ///
    /// The questions go to each panel in turn on a worker thread (a panel that doesn't answer costs up to
    /// a few seconds); afterwards `modules_changed` runs on the UI thread.
    pub fn refresh_modules(self: &Rc<Self>) {
        let (sender, panels, modules) = (self.sender.clone(), self.panels(), self.modules.clone());
        self.in_background(
            move || {
                *modules.lock().unwrap() = read_modules(&sender, &panels);
            },
            // `self` is an Rc and can't go into this Send closure, so it fetches the Ctx again from the
            // thread-local; the closure runs on the UI thread, where that is allowed.
            |app, ()| ctx().modules_changed(app),
        );
    }

    /// Tells the pages that the panels' firmware information changed.
    ///
    /// Runs every callback the pages registered in `on_modules`, then refreshes the bar.
    pub fn modules_changed(&self, app: &AppWindow) {
        for f in self.on_modules.lock().unwrap().iter() {
            f(app);
        }
        self.update_bar(app);
    }

    /// Tells every page that registered in `on_page` which page was just shown (its key, e.g. "math"), so
    /// a page can refresh itself when opened.
    pub fn page_shown(&self, app: &AppWindow, key: &str) {
        for f in self.on_page.lock().unwrap().iter() {
            f(app, key);
        }
    }
}

/// Asks each panel for its firmware version and startup setting (on a worker thread).
///
/// Blocking: two questions per panel. The startup question gets 2 s (firmware without that command
/// never answers); the version question gets the default 3 s. If either can't be sent or read at all
/// the panel is marked `in_use` (most often another program has its port open) and its version is
/// left unknown.
pub fn read_modules(sender: &Sender, panels: &[Panel]) -> HashMap<String, ModuleInfo> {
    let mut info = HashMap::new();
    for p in panels {
        let startup = sender.ask(&p.device, cmd::command(cmd::STARTUP_ANIMATION, &[]), Some(2.0));
        let version = sender.ask(&p.device, cmd::command(cmd::VERSION, &[]), None);
        // A panel that doesn't answer in time gives a short or empty reply, not an error (firmware
        // without the startup command gives `startup: None` that way). An error means the port itself
        // couldn't be used.
        let entry = match (startup, version) {
            (Ok(s), Ok(v)) => ModuleInfo {
                version: Some(cmd::parse_version(&v)),
                build: cmd::parse_wr_build(&v),
                startup: cmd::parse_startup(&s),
                in_use: false,
            },
            (s, v) => {
                wr_core::info!("Could not read {}: {:?} {:?}", p.device, s.err(), v.err());
                ModuleInfo { in_use: true, ..Default::default() }
            }
        };
        info.insert(p.device.clone(), entry);
    }
    info
}

/// The parts of the window's state that can go to another thread.
///
/// The same fields as `Ctx` (see there) minus the UI-thread-only callbacks and timers, all `Arc`s, so
/// it is `Send` and can go into a worker or a timer closure.
#[derive(Clone)]
pub struct Parts {
    pub ui: slint::Weak<AppWindow>,
    pub link: Arc<Link>,
    pub sender: Arc<Sender>,
    pub lender: Arc<Lender>,
    pub panels: Arc<Mutex<Vec<Panel>>>,
    pub checked: Arc<Mutex<Vec<bool>>>,
    pub monitor_running: Arc<AtomicBool>,
    pub music_on: Arc<AtomicBool>,
    pub modules: Arc<Mutex<HashMap<String, ModuleInfo>>>,
}

impl Parts {
    /// The bottom of the menu, and the Home page's panel cards.
    ///
    /// Must run on the UI thread (it sets Slint properties). The text says, in order of priority: the
    /// monitor isn't running; the window is showing its own controls on some panels; the panels react
    /// to audio; or they show the system monitor. Each panel card gets the same kind of text for that
    /// panel, plus its firmware version when known.
    pub fn update_bar(&self, app: &AppWindow) {
        let panels = self.panels.lock().unwrap().clone();
        let running = self.monitor_running.load(Ordering::SeqCst);
        let music = self.music_on.load(Ordering::SeqCst);
        let borrowed: Vec<&Panel> = panels.iter().filter(|p| self.lender.is_borrowed(&p.device)).collect();
        let text = if !running {
            "The system monitor isn't running".to_string()
        } else if !borrowed.is_empty() {
            format!(
                "Showing White Rabbit controls on: {}",
                borrowed.iter().map(|p| p.called()).collect::<Vec<_>>().join(", ")
            )
        } else if music {
            "Panels are reacting to audio".to_string()
        } else {
            "Panels are showing the system monitor".to_string()
        };
        let g = app.global::<App>();
        g.set_bar_status(text.into());
        g.set_any_borrowed(!borrowed.is_empty());
        g.set_monitor_running(running);
        g.set_music_mode(music);
        g.set_monitor_showing(running && borrowed.is_empty() && !music);
        let checked = self.checked.lock().unwrap().clone();
        let modules = self.modules.lock().unwrap().clone();
        let rows: Vec<PanelRow> = panels
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let doing = if !running {
                    "The system monitor isn't running"
                } else if self.lender.is_borrowed(&p.device) {
                    "Showing something from this window"
                } else if music {
                    "Reacting to audio"
                } else {
                    "Showing the system monitor"
                };
                // A panel whose firmware couldn't be read (in use) shows no firmware part rather than a wrong one.
                let firmware = modules.get(&p.device).filter(|m| !m.in_use).map(crate::pages::firmware::short_name);
                PanelRow {
                    device: p.device.clone().into(),
                    label: p.label().into(),
                    doing: match firmware {
                        Some(v) => format!("{doing}.  Firmware: {v}"),
                        None => format!("{doing}."),
                    }
                    .into(),
                    checked: checked.get(i).copied().unwrap_or(true),
                }
            })
            .collect();
        // The Home cards and the menu's list share this one model.
        let model = g.get_panels();
        // Change the rows in place when the count is the same, so a ticked box doesn't flicker
        if model.row_count() == rows.len() {
            for (i, r) in rows.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&r) {
                    model.set_row_data(i, r);
                }
            }
        } else {
            g.set_panels(ModelRc::new(VecModel::from(rows)));
        }
    }

    /// Gives these panels back to the monitor on a worker thread, then updates the window.
    ///
    /// `only`: return a panel only if it hasn't been borrowed again since these generations were taken
    /// (see `Lender::give_back`).
    pub fn give_back(&self, panels: Vec<Panel>, only: Option<Generations>) {
        let me = self.clone();
        std::thread::spawn(move || {
            me.lender.give_back(&me.sender, &panels, only.as_ref());
            let me2 = me.clone();
            Ctx::later(&me.ui, move |app| me2.update_bar(app));
        });
    }

    /// The ticked panels, as `Ctx::selected_panels`, for code that only has `Parts`.
    pub fn selected_panels(&self) -> Vec<Panel> {
        let checked = self.checked.lock().unwrap();
        self.panels.lock().unwrap().iter().zip(checked.iter()).filter(|(_, c)| **c).map(|(p, _)| p.clone()).collect()
    }
}

/// Wires up the parts of the window that aren't a page of their own: the menu's panel list, Home's
/// actions, the theme and the Name panels dialog.
///
/// Called once by `build` in main.rs. Each block below clones the `Rc<Ctx>` into one callback; the
/// callbacks run on the UI thread when the user clicks. Also starts two timers: the 5-second monitor
/// check and the 2-second check for a change in Windows' look (light/dark, accent colour, text size,
/// high contrast).
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let g = app.global::<App>();
    {
        let ctx = ctx.clone();
        // A tick box under "Controls act on" changed; i is the panel's index in the list.
        g.on_panel_checked(move |i, on| {
            // The lock is released at the end of this statement, before update_bar takes it again.
            if let Some(c) = ctx.checked.lock().unwrap().get_mut(i as usize) {
                *c = on;
            }
            if let Some(app) = ctx.ui.upgrade() {
                ctx.update_bar(&app);
            }
        });
    }
    {
        let ctx = ctx.clone();
        g.on_give_back_all(move || ctx.give_back_all());
    }
    {
        let ctx = ctx.clone();
        // "Which is which": the monitor shows each panel's name on it, then the names of its apps.
        g.on_identify(move || {
            ctx.in_background(
                || request(json!({"cmd": "identify"}), Duration::from_secs(3)).is_some(),
                |app, sent| {
                    app.global::<App>().set_home_status(
                        if sent {
                            "Look at the panels."
                        } else {
                            "The system monitor isn't running, so the panels can't be shown."
                        }
                        .into(),
                    )
                },
            );
        });
    }
    {
        let ctx = ctx.clone();
        // Audio reactivity takes over the panels, so a running game or Math picture stops first.
        g.on_music_toggled(move |on| {
            if on {
                if let Some(app) = ctx.ui.upgrade() {
                    crate::pages::games::stop_for_other(&app);
                    crate::pages::math::stop_for_other(&app);
                }
            }
            set_music_mode(&ctx, on, None)
        });
    }
    {
        let ctx = ctx.clone();
        g.on_section_toggled(move |key, on| section_toggled(&ctx, &key, on));
    }
    {
        let ctx = ctx.clone();
        // Light / dark / system: saved in settings.json, then applied at once.
        g.on_theme_chosen(move |i| {
            let choice = theme::CHOICES.get(i as usize).copied().unwrap_or("system");
            settings::set(theme::SETTING, Value::String(choice.into()));
            if let Some(app) = ctx.ui.upgrade() {
                theme::apply(&app, choice, &theme::read_windows());
            }
        });
    }
    {
        let ctx = ctx.clone();
        // Opens the "Name the panels" dialog, filled with the saved names.
        g.on_name_panels(move || {
            let Some(app) = ctx.ui.upgrade() else { return };
            let names = settings::panel_names();
            let name = |side: &str| SharedString::from(names.get(side).and_then(Value::as_str).unwrap_or(""));
            let g = app.global::<App>();
            g.set_left_name(name("left"));
            g.set_right_name(name("right"));
            g.set_naming(true);
        });
    }
    {
        let ctx = ctx.clone();
        // Saves the names, updates the in-memory panel list, and if the monitor runs asks it to show them
        // on the panels so the user sees which name went where (the reply doesn't matter).
        g.on_save_names(move |left, right| {
            let names = settings::clean_panel_names(
                &json!({"left": left.as_str(), "right": right.as_str()}).as_object().cloned().unwrap_or_default(),
            );
            settings::set("panel_names", Value::Object(names.clone()));
            for p in ctx.panels.lock().unwrap().iter_mut() {
                // Panels with no name saved get an empty nickname, which means "no name".
                p.nickname = names.get(&p.side).and_then(Value::as_str).unwrap_or("").to_string();
            }
            if let Some(app) = ctx.ui.upgrade() {
                ctx.update_bar(&app);
            }
            if ctx.monitor_running.load(Ordering::SeqCst) {
                ctx.in_background(|| request(json!({"cmd": "identify"}), Duration::from_secs(3)), |_, _| {});
            }
        });
    }
    {
        let ctx = ctx.clone();
        g.on_page_shown(move |key| {
            if let Some(app) = ctx.ui.upgrade() {
                ctx.page_shown(&app, &key);
                wr_core::info!("Page {key} shown");
            }
        });
    }
    g.on_open_url(|url| open_url(&url));

    // Keep up with the monitor, and with Windows' look
    {
        let ctx2 = ctx.clone();
        let check = move || {
            let (link, running, music, ui) =
                (ctx2.link.clone(), ctx2.monitor_running.clone(), ctx2.music_on.clone(), ctx2.ui.clone());
            let parts = ctx2.clone_parts();
            std::thread::spawn(move || {
                // On a fresh thread each time: the status request can take up to 1.5 s and must not stall the UI.
                let is_running = link.running(true);
                running.store(is_running, Ordering::SeqCst);
                if is_running {
                    if let Some(reply) = request(json!({"cmd": "music", "action": "status"}), Duration::from_secs(3)) {
                        music
                            .store(reply.get("music_mode").and_then(Value::as_bool).unwrap_or(false), Ordering::SeqCst);
                    }
                } else {
                    music.store(false, Ordering::SeqCst);
                }
                Ctx::later(&ui, move |app| parts.update_bar(app));
            });
        };
        // Once now, so the bar is right straight away, then every 5 seconds.
        check();
        ctx.every(5.0, check);
    }
    {
        let ui = ctx.ui.clone();
        // Reading Windows' theme is a quick registry read, so this one stays on the UI thread; the window is
        // only restyled when something actually changed.
        let mut last = theme::read_windows();
        ctx.every(2.0, move || {
            let now = theme::read_windows();
            if now != last {
                last = now.clone();
                if let Some(app) = ui.upgrade() {
                    theme::apply(&app, &theme::load_choice(), &now);
                }
            }
        });
    }
}

/// Turns music mode on or off through the monitor. Panels lent to this window don't show the monitor's
/// drawing, so they are given back first. done(reply) gets the monitor's answer (None: not running).
///
/// The new state is stored from the monitor's reply (off if it didn't answer). Turning it on with no
/// monitor shows a note on Home saying the monitor is needed.
pub fn set_music_mode(ctx: &Rc<Ctx>, on: bool, done: Option<Box<dyn FnOnce(&AppWindow, Option<Value>) + Send>>) {
    let parts = ctx.clone_parts();
    ctx.in_background(
        move || {
            if on {
                // Clone the list so the panel lock isn't held during the give-back's serial and pipe I/O.
                parts.lender.give_back(&parts.sender, &parts.panels.lock().unwrap().clone(), None);
            }
            let reply =
                request(json!({"cmd": "music", "action": if on { "on" } else { "off" }}), Duration::from_secs(5));
            parts.music_on.store(
                reply.as_ref().and_then(|r| r.get("music_mode")).and_then(Value::as_bool).unwrap_or(false),
                Ordering::SeqCst,
            );
            (parts, reply)
        },
        move |app, (parts, reply)| {
            if reply.is_none() && on {
                app.global::<App>()
                    .set_home_status("Audio reactivity needs the system monitor, which isn't running.".into());
            }
            parts.update_bar(app);
            if let Some(done) = done {
                done(app, reply);
            }
        },
    );
}

/// The System monitor, Games & patterns and Math switches. These take over the panels, so turning one
/// on turns the others (and audio reactivity) off.
///
/// Turning the System monitor off borrows every panel and leaves them dark (a blank frame), so the
/// monitor stops drawing; turning it on gives every panel back.
fn section_toggled(ctx: &Rc<Ctx>, key: &str, on: bool) {
    let Some(app) = ctx.ui.upgrade() else { return };
    let music_off = |ctx: &Rc<Ctx>| {
        if ctx.music_on.load(Ordering::SeqCst) {
            set_music_mode(ctx, false, None);
        }
    };
    match (key, on) {
        ("games", true) => {
            music_off(ctx);
            crate::pages::games::switch_on(&app);
        }
        // Turning a section off is the same as pressing its page's Stop button.
        ("games", false) => app.global::<crate::GamesPage>().invoke_stop(),
        ("math", true) => {
            music_off(ctx);
            crate::pages::math::switch_on(&app);
        }
        ("math", false) => app.global::<crate::MathPage>().invoke_stop(),
        ("monitor", true) => {
            crate::pages::games::stop_for_other(&app);
            crate::pages::math::stop_for_other(&app);
            music_off(ctx);
            ctx.give_back_all();
            ctx.update_bar(&app);
        }
        ("monitor", false) => {
            crate::pages::games::stop_for_other(&app);
            crate::pages::math::stop_for_other(&app);
            music_off(ctx);
            // Borrow every panel from the monitor and leave it dark
            let status: Status = Arc::new(|app: &AppWindow, text: String| {
                app.global::<App>().set_home_status(
                    if text == "Done." {
                        "The panels are dark. Turn the System monitor back on to bring it back.".into()
                    } else {
                        text
                    }
                    .into(),
                )
            });
            ctx.act(
                // One black/white frame with every LED off.
                |s, p| s.write(&p.device, &[cmd::draw_bits(&|_, _| false)]),
                status,
                Some(ctx.panels()),
                true,
                None,
                false,
            );
        }
        _ => {}
    }
    ctx.update_bar(&app);
}

/// Opens a web page in the default browser.
///
/// Windows: the shell's "open" verb; elsewhere `xdg-open`. Errors are only logged.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        open_path(url);
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Opens a file, folder or web page with whatever Windows opens it with.
///
/// Uses the shell's "open" verb (ShellExecuteW), like double-clicking it in Explorer. False (and a log
/// line) when Windows couldn't open it.
#[cfg(windows)]
pub fn open_path(target: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (op, file) = (wide("open"), wide(target));
    // SAFETY: the strings are null-terminated and live across the call.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW's old-style result: a value above 32 means success, 32 or less is an error code.
    if (r as isize) <= 32 {
        wr_core::warn!("Could not open {target}");
        return false;
    }
    true
}

/// Away from Windows: opens with `xdg-open`; true if it could be started.
#[cfg(not(windows))]
pub fn open_path(target: &str) -> bool {
    std::process::Command::new("xdg-open").arg(target).spawn().is_ok()
}
