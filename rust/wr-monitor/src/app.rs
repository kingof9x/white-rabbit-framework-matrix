//! Starting up on Windows: one copy only, find the panels, start the drawing threads and the main
//! loop, then show the tray icon until Quit.
//!
//! `main.rs` calls [`run`] on the process's first thread, and everything else hangs off it. The order
//! matters: the log comes first so every later step can report problems; the single-instance check
//! comes before any panel is opened so two monitors never fight over the same serial ports; the panels
//! are found before the drawing threads start because each drawing thread is told which side (0 = left,
//! 1 = right) it serves; and the tray window comes last because it blocks this thread until Quit.
//!
//! Besides `run`, this file builds the plug watcher (USB/charger/display events turned into effects)
//! and `WindowsSystem`, the small object through which the control channel asks Windows-only questions.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wr_core::queue::FrameQueue;
use wr_core::settings::Settings;
use wr_panel::ports::{discover, Pins};

use crate::control::bridge::{pid_alive, Bridge};
use crate::control::commands::Commands;
use crate::launch::{self, Start};
use crate::monitor::{log_panics, pins_of, LayoutSource, Monitor, Shared};
use crate::sensors::{ScreenBrightness, WindowsReadings};
use crate::{paths, ui};
use wr_core::effects::EffectsSettings;
use wr_core::{info, warn};
use wr_panel::hub::PortSystem;
use wr_panel::overlay::EffectsOverlay;
use wr_panel::panel::{Panel, Panels};
use wr_panel::winports::WindowsPorts;

/// Starts the whole monitor and returns the process exit code (always 0 today: a missing panel is
/// reported to the user in a message box rather than with an error code).
///
/// Side effects: starts the log, may launch WhiteRabbitWindow.exe, takes the single-instance mutex,
/// spawns the keyboard, drawing, control, holders, plug and main-loop threads, and on return has
/// darkened the panels and joined the main loop and drawing threads.
pub fn run() -> i32 {
    wr_core::log::init(paths::log_file());
    // From here on any panic (on any thread) is written to the log before the program aborts.
    log_panics();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `start` says what this launch is for: only the window, the monitor in the background (login),
    // or the monitor plus the window (the user opened the app).
    let start = launch::start_from(&args);
    if let Start::Window(args) = &start {
        // The window, its layout editor and the reports belong to WhiteRabbitWindow.exe (wr-window):
        // hand the options over to it and stop
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        launch::open_window(&args);
        return 0;
    }

    // A named mutex tells whether another WhiteRabbit.exe is already running (see `ui::single_instance`).
    if !ui::single_instance() {
        // Opening the app again while it runs shows the window
        if start == Start::MonitorAndWindow {
            launch::open_window(&["--window"]);
        }
        return 0;
    }
    info!("White Rabbit monitor starting");
    if start == Start::MonitorAndWindow {
        // The window waits until the monitor answers on the control channel
        launch::open_window(&["--window", "--wait-for-monitor"]);
    }

    // The RGB keyboard and macropad: app-drawn effects and the audio show while the window is closed
    // The last argument is called when the keyboard part wants the app-key hotkeys registered again;
    // hotkeys belong to the tray window's thread, so it posts a request to that window.
    let keyboard = crate::keyboard::KeyboardJob::new(
        paths::keyboard_file(),
        false,
        Box::new(|| ui::request(crate::control::commands::UiRequest::AppKeys)),
    );
    // `pid_alive` lets the keyboard job notice that a window that took over the keyboard has died.
    keyboard.start(pid_alive);
    // A global low-level keyboard hook (on its own `audio-shortcut` thread) that toggles the audio show.
    wr_keyboard::shortcut::watch(|| {
        if let Some(job) = crate::keyboard::job() {
            job.toggle_audio();
        }
    });

    // The layout says which apps go on which panel, and may "pin" a side to a USB location so the left
    // and right panels never swap.
    let source = LayoutSource::locate();
    let layout = source.load();
    let ports: Arc<dyn PortSystem> = Arc::new(WindowsPorts);

    // When started at login, the USB devices may still be appearing
    // `found` is a list of (USB location, COM port) pairs, left panel first.
    let mut found = discover(&ports.list(), &pins_of(&layout));
    // 15 tries 2 s apart: up to 30 s for Windows to finish enumerating USB at login.
    for _ in 0..15 {
        // With a keyboard there is something to do already; the panels are found when they appear
        if !found.is_empty() || wr_keyboard::device::any_present() {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        found = discover(&ports.list(), &pins_of(&layout));
    }
    if found.is_empty() && wr_keyboard::device::any_present() {
        // Keep running for the keyboard; the panels are looked for again in the background
        info!("No LED panels found; running for the keyboard");
    } else if found.is_empty() {
        warn!("No LED panels found");
        keyboard.stop();
        ui::show_error(
            "No LED Matrix panels were found. Check that the modules are installed, and that no other program \
             (such as the LED Matrix GUI) is using them.",
        );
        return 0;
    }
    // `zip` stops at the shorter list, so with one panel only the left one is logged.
    for (side, (location, port)) in ["left", "right"].iter().zip(&found) {
        info!("{side} panel: {port} at USB location {}", location.as_deref().unwrap_or("?"));
    }

    let shared = Arc::new(Shared::default());
    *shared.settings.lock().unwrap() = Settings::load(&paths::settings_file());

    // Shared with the main loop, which updates it when the layout file changes, so that a panel
    // that is found again later goes to the right side.
    let pins: Arc<Mutex<Pins>> = Arc::new(Mutex::new(pins_of(&layout)));
    let panels = Panels::new(ports.clone(), pins.clone());
    // The window's serial/hold/release commands share the drawing threads' open ports
    let bridge = Arc::new(Bridge::new(panels.clone()));
    // The Effects tab's effects, drawn over every frame by the drawing threads
    // Read straight from the JSON (not `Settings`) because the effects block is owned by wr-core's
    // effects module; a missing or broken file just gives the default effects.
    let effects_json = std::fs::read_to_string(paths::settings_file())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| v["effects"].clone())
        .unwrap_or_default();
    let effects = EffectsOverlay::new(EffectsSettings::from_json(&effects_json));
    panels.set_overlay(effects.clone());
    *shared.effects.lock().unwrap() = Some(effects);
    *shared.panels.lock().unwrap() = Some(panels.clone());
    // One single-slot queue per side: the main loop puts frames, that side's drawing thread takes them.
    // Both threads start even with one panel, so a second panel plugged in later just starts showing.
    let queues = [Arc::new(FrameQueue::new()), Arc::new(FrameQueue::new())];
    let mut drawing = Vec::new();
    for (side, queue) in queues.iter().enumerate() {
        let panel = Panel::new(side, panels.clone());
        let queue = queue.clone();
        drawing.push(
            std::thread::Builder::new()
                .name(format!("panel-{side}"))
                .spawn(move || panel.run(queue))
                .expect("drawing thread"),
        );
    }

    // The White Rabbit window talks to the panels and changes settings through the control channel
    // Everything the control thread needs: the shared flags, the bridge to the serial ports, where
    // settings are saved, how to ask the tray window to act, and the Windows-only queries.
    let commands = Commands {
        shared: shared.clone(),
        bridge: bridge.clone(),
        settings_file: paths::settings_file(),
        ui: Box::new(ui::request),
        system: Box::new(WindowsSystem),
    };
    // Spawns the `control` thread that listens on the named pipe; it returns at once.
    crate::control::serve(Arc::new(commands));
    start_plug_watcher(&shared);
    {
        // Give back the panels of a window that closed or crashed without releasing them
        let (bridge, shared) = (bridge.clone(), shared.clone());
        // Every 2 s: if a window that "held" a panel for itself has exited without letting go,
        // `release_orphans` gives the panel back and the main loop is asked to redraw at once.
        std::thread::Builder::new()
            .name("holders".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                if bridge.release_orphans(pid_alive) {
                    shared.redraw.store(true, Ordering::SeqCst);
                }
            })
            .expect("holders thread");
    }

    // Starts the `brightness` thread (WMI is slow); `get` only reads its latest cached value.
    let brightness = ScreenBrightness::start();
    let monitor = Monitor::new(
        shared.clone(),
        queues,
        pins,
        Box::new(WindowsReadings::new()),
        Box::new(move || brightness.get()),
        source,
    );
    let main_loop =
        std::thread::Builder::new().name("main-loop".into()).spawn(move || monitor.run()).expect("main loop");

    // Blocks: runs the tray window's message loop on this thread until the user picks Quit.
    ui::run(shared.clone());

    // Quit: the main loop darkens the panels and stops the drawing threads
    shared.quit.store(true, Ordering::SeqCst);
    // Join errors (a panicked thread) are ignored: we are exiting anyway, and in release builds a panic
    // would already have aborted the process.
    let _ = main_loop.join();
    for t in drawing {
        let _ = t.join();
    }
    keyboard.stop();
    info!("Exiting");
    0
}

/// Plugging things in and out: a ripple from the port and a drain through it, and "Charging" /
/// "On battery" on the text line (Python's `on_plug`).
///
/// Builds two callbacks and hands them to `plugwin::start`, which runs the `plug-events` thread:
/// `learned` returns the user's "learned ports" (which USB port sits on which side of the laptop),
/// and `fire` turns a plug event into an effect, if the Effects settings ask for it.
fn start_plug_watcher(shared: &Arc<Shared>) {
    // Read fresh on every event, so ports learned in the Effects tab take effect without a restart.
    let learned = {
        let shared = shared.clone();
        Box::new(move || {
            shared
                .effects
                .lock()
                .unwrap()
                .as_ref()
                .map(|fx| fx.effects().settings.learned_ports.clone())
                .unwrap_or_default()
        })
    };
    let fire = {
        let shared = shared.clone();
        Box::new(move |event: crate::plug::PlugEvent| {
            let time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            // Kept for the window (it shows the last event, e.g. while learning ports), stamped with
            // Unix time in seconds.
            *shared.last_plug.lock().unwrap() = event.to_json(time);
            // Clone the overlay handle out so the `shared.effects` lock is not held while drawing.
            let Some(fx) = shared.effects.lock().unwrap().clone() else { return };
            let (wanted, say) = {
                let e = fx.effects();
                let s = &e.settings;
                // The kind must be switched on, and so must the side ("left"/"right"/unknown) it was on.
                let wanted = match event.kind {
                    "charger" => s.plug_charger,
                    "headphones" => s.plug_headphones,
                    "display" => s.plug_displays,
                    _ => s.plug_usb,
                } && s.plug_side_on(event.position.as_deref());
                (wanted, event.kind == "charger" && s.text_charger)
            };
            if wanted {
                fx.plug(event.position.as_deref(), event.plugged_in);
            }
            if say {
                fx.effects().say(if event.plugged_in { "Charging" } else { "On battery" });
            }
        })
    };
    crate::plugwin::start(Arc::new(crate::plug::PlugWatcher::new(learned, fire)));
}

/// The control commands' questions about Windows.
///
/// Kept behind the `System` trait so the control commands can be tested on Linux with a fake.
struct WindowsSystem;

impl crate::control::commands::System for WindowsSystem {
    /// Where temperatures come from and whether it works, as JSON for the window's Panels tab and
    /// layout editor; `reload` makes it read again if the chosen source changed since the last reading.
    fn sensor_source(&self, reload: bool) -> serde_json::Value {
        crate::thermal::source_status(reload)
    }

    /// The names of the graphics cards, for the GPU picker in the window.
    fn gpus(&self) -> Vec<String> {
        crate::gpu::names()
    }
}
