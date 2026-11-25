//! Pictures of the window without a screen (--shoot OUT.png PAGE [SCALE]), for screenshots away from
//! Windows and for checking a page lays out; and the smoke test CI runs on the real window.
//! Use WHITE_RABBIT_APPEARANCE, WHITE_RABBIT_ACCENT and WHITE_RABBIT_FAKE_PANELS to set the scene.

//!
//! How --shoot works without a screen: Slint lets a program supply its own "platform" (the layer that
//! makes windows and runs the event loop). `Offscreen` is a minimal one: its only window is a
//! `MinimalSoftwareWindow` that draws into memory with Slint's software renderer, and it has no real
//! event loop. Instead `shoot` pumps by hand: every 20 ms it runs Slint's timers and any closures
//! worker threads queued for the UI thread (`upgrade_in_event_loop` lands in `Queue`), so the pages
//! fill in as they would on screen. Then the window is drawn into a buffer and saved as a PNG.
//!
//! The smoke test is different: it runs inside the real window and event loop (run_window in
//! main.rs starts it when WHITE_RABBIT_SMOKE_TEST is set) and only logs what it saw.
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use slint::EventLoopError;

use crate::{app, App, AppWindow};

/// Closures sent to the UI thread from other threads, waiting for the next `pump`.
type Queue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

/// The screenless platform: the one in-memory window, and the queue its event-loop proxy fills.
struct Offscreen(Rc<MinimalSoftwareWindow>, Queue);

/// What Slint uses to reach the "event loop" from other threads: it just adds to the queue.
struct Proxy(Queue);

impl EventLoopProxy for Proxy {
    /// There is no real loop to quit; `shoot` ends by itself once the picture is saved.
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        Ok(())
    }

    /// Queues a closure for the UI thread (this is what `upgrade_in_event_loop` calls).
    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), EventLoopError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

impl Platform for Offscreen {
    /// Every window Slint makes is the one in-memory window.
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    /// Gives Slint a proxy so code on worker threads can send work back to the UI thread.
    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy(self.1.clone())))
    }
}

/// Runs what other threads sent to the window's thread, and the timers.
fn pump(queue: &Queue) {
    slint::platform::update_timers_and_animations();
    // Take the whole queue first and run it after letting go of the lock, since a job may queue more.
    let jobs: Vec<_> = std::mem::take(&mut *queue.lock().unwrap());
    for job in jobs {
        job();
    }
}

/// Draws the window into a `w` x `h` RGB buffer (3 bytes per pixel, row by row).
///
/// Drawn four times, running timers and animations between, so the picture shows the page after
/// anything started by the first draw has settled. The colours come out
/// premultiplied by alpha; the window is opaque, so they are used as they are.
pub fn render(window: &MinimalSoftwareWindow, w: u32, h: u32) -> Vec<u8> {
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(&mut buffer, w as usize);
        });
    }
    buffer.iter().flat_map(|p| [p.red, p.green, p.blue]).collect()
}

/// Saves an RGB buffer as a PNG file. Panics (ending the --shoot run) if the size is wrong or the
/// file can't be written.
pub fn save_png(path: &str, w: u32, h: u32, rgb: &[u8]) {
    let image = image::RgbImage::from_raw(w, h, rgb.to_vec()).expect("picture size");
    image.save(path).expect("couldn't save the picture");
}

///
/// Draws one page to a PNG. SCALE is the pixels per point (default 1.5); WIDTH and HEIGHT the window
/// size in points (default 1240 x 860), so the picture is WIDTH*SCALE x HEIGHT*SCALE pixels. Waits
/// WHITE_RABBIT_SHOOT_WAIT seconds (default 1) for the page to fill in, then applies
/// WHITE_RABBIT_SHOOT_ACTION if set (see `shoot_action` in main.rs).
/// --shoot OUT.png PAGE [SCALE] [WIDTH HEIGHT]
pub fn shoot(args: &[String]) {
    let out = args.first().cloned().unwrap_or_else(|| "shot.png".into());
    let page = args.get(1).cloned().unwrap_or_else(|| "home".into());
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let lw: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1240.0);
    let lh: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(860.0);
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let queue: Queue = Arc::default();
    // Must be set before the first Slint window is made, so `build` below gets the in-memory window.
    slint::platform::set_platform(Box::new(Offscreen(window.clone(), queue.clone()))).expect("platform");
    let (ui, ctx) = crate::build(&page);
    let (w, h) = ((lw * scale) as u32, (lh * scale) as u32);
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    window.set_size(slint::PhysicalSize::new(w, h));
    ui.show().unwrap();
    // Let the pages fill in: background work, previews, timers
    let settle: f64 = std::env::var("WHITE_RABBIT_SHOOT_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let end = std::time::Instant::now() + Duration::from_secs_f64(settle);
    while std::time::Instant::now() < end {
        pump(&queue);
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Ok(action) = std::env::var("WHITE_RABBIT_SHOOT_ACTION") {
        crate::shoot_action(&ui, &ctx, &action);
        // WHITE_RABBIT_SHOOT_AFTER: seconds to wait after the action (default half a second)
        let after: f64 = std::env::var("WHITE_RABBIT_SHOOT_AFTER").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        // 50 pumps of 20 ms per second.
        for _ in 0..(after * 50.0) as usize {
            pump(&queue);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let rgb = render(&window, w, h);
    save_png(&out, w, h, &rgb);
    drop(ctx);
}

///
/// Steps every 400 ms on a Slint timer (so on the UI thread, between real events): one page per step,
/// then all looks at once, then it quits the event loop. Each step is logged, and the CI job reads
/// the log to check the window got through every page without crashing.
/// The CI smoke test: open every page, change the look to each choice and back, then close.
pub fn smoke_test(ui: &AppWindow, ctx: &Rc<app::Ctx>) {
    let pages = [
        "home",
        "monitor",
        "games",
        "music",
        "math",
        "draw",
        "effects",
        "messages",
        "transitions",
        "startup",
        "settings",
        "firmware",
        "shortcuts",
        "about",
    ];
    let weak = ui.as_weak();
    let ctx = ctx.clone();
    let step = std::cell::Cell::new(0usize);
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(400), move || {
        let Some(ui) = weak.upgrade() else { return };
        let i = step.get();
        step.set(i + 1);
        if let Some(page) = pages.get(i) {
            ui.global::<App>().set_page((*page).into());
            wr_core::info!("Smoke test: page {page} shown");
        } else if i == pages.len() {
            let saved = crate::theme::load_choice();
            for choice in crate::theme::CHOICES {
                crate::theme::apply(&ui, choice, &crate::theme::read_windows());
                wr_core::info!("Smoke test: look {choice} shows as {}", ui.global::<crate::Theme>().get_mode());
            }
            crate::theme::apply(&ui, &saved, &crate::theme::read_windows());
            wr_core::info!("Smoke test: window {} with {} panels", crate::version(), ctx.panels().len());
        } else {
            wr_core::info!("Smoke test: done");
            let _ = slint::quit_event_loop();
        }
    });
    // The timer must keep running after this function returns, and a dropped Slint timer stops; it is
    // leaked on purpose (the program ends when the test does).
    std::mem::forget(timer);
}
