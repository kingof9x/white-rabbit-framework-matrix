//! Starting up on Windows: one copy only, find the panels, start the drawing threads and the main
//! loop, then show the tray icon until Quit.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wr_core::queue::FrameQueue;
use wr_core::settings::Settings;
use wr_panel::ports::{discover, Pins};

use crate::control::bridge::{pid_alive, Bridge};
use crate::control::commands::Commands;
use crate::control::winserial::WindowsRawPorts;
use crate::monitor::{log_panics, pins_of, LayoutSource, Monitor, Shared};
use crate::sensors::{ScreenBrightness, WindowsReadings};
use crate::{paths, ui};
use wr_core::{info, warn};
use wr_panel::panel::{Claims, Panel, PortSystem};

pub fn run() -> i32 {
    wr_core::log::init(paths::log_file());
    log_panics();
    info!("White Rabbit Rust test build {} starting", env!("CARGO_PKG_VERSION"));

    if !ui::single_instance() {
        info!("White Rabbit is already running");
        ui::show_error(
            "White Rabbit is already running. Quit it from its tray icon first, then start the Rust test build.",
        );
        return 0;
    }

    let source = LayoutSource::locate();
    let layout = source.load();
    // The drawing threads reach the ports through the bridge, which shares them with the window
    let bridge = Arc::new(Bridge::new(Arc::new(WindowsRawPorts)));
    let ports: Arc<dyn PortSystem> = bridge.clone();

    // When started at login, the USB devices may still be appearing
    let mut found = discover(&ports.list(), &pins_of(&layout));
    for _ in 0..15 {
        if !found.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        found = discover(&ports.list(), &pins_of(&layout));
    }
    if found.is_empty() {
        warn!("No LED panels found");
        ui::show_error(
            "No LED Matrix panels were found. Check that the modules are installed, and that no other program \
             (such as the LED Matrix GUI) is using them.",
        );
        return 0;
    }
    for (side, (location, port)) in ["left", "right"].iter().zip(&found) {
        info!("{side} panel: {port} at USB location {}", location.as_deref().unwrap_or("?"));
    }

    let shared = Arc::new(Shared::default());
    *shared.settings.lock().unwrap() = Settings::load(&paths::settings_file());

    let pins: Arc<Mutex<Pins>> = Arc::new(Mutex::new(pins_of(&layout)));
    let claims = Arc::new(Claims::default());
    let queues = [Arc::new(FrameQueue::new()), Arc::new(FrameQueue::new())];
    let mut drawing = Vec::new();
    for (side, queue) in queues.iter().enumerate() {
        let panel = Panel::new(side, ports.clone(), claims.clone(), pins.clone());
        let queue = queue.clone();
        drawing.push(
            std::thread::Builder::new()
                .name(format!("panel-{side}"))
                .spawn(move || panel.run(queue))
                .expect("drawing thread"),
        );
    }

    // The White Rabbit window talks to the panels and changes settings through the control channel
    let commands = Commands {
        shared: shared.clone(),
        bridge: bridge.clone(),
        settings_file: paths::settings_file(),
        ui: Box::new(ui::request),
    };
    crate::control::serve(Arc::new(commands));
    {
        // Give back the panels of a window that closed or crashed without releasing them
        let (bridge, shared) = (bridge.clone(), shared.clone());
        std::thread::Builder::new()
            .name("holders".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                if !bridge.release_orphans(pid_alive).is_empty() {
                    shared.redraw.store(true, Ordering::SeqCst);
                }
            })
            .expect("holders thread");
    }

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

    ui::run(shared.clone());

    // Quit: the main loop darkens the panels and stops the drawing threads
    shared.quit.store(true, Ordering::SeqCst);
    let _ = main_loop.join();
    for t in drawing {
        let _ = t.join();
    }
    info!("Exiting");
    0
}
