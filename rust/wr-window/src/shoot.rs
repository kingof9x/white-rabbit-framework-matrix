//! Pictures of the window without a screen (--shoot OUT.png PAGE [SCALE]), for screenshots away from
//! Windows and for checking a page lays out; and the smoke test CI runs on the real window.
//! Use WHITE_RABBIT_APPEARANCE, WHITE_RABBIT_ACCENT and WHITE_RABBIT_FAKE_PANELS to set the scene.

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use slint::EventLoopError;

use crate::{app, App, AppWindow};

type Queue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

struct Offscreen(Rc<MinimalSoftwareWindow>, Queue);

struct Proxy(Queue);

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), EventLoopError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy(self.1.clone())))
    }
}

/// Runs what other threads sent to the window's thread, and the timers.
fn pump(queue: &Queue) {
    slint::platform::update_timers_and_animations();
    let jobs: Vec<_> = std::mem::take(&mut *queue.lock().unwrap());
    for job in jobs {
        job();
    }
}

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

pub fn save_png(path: &str, w: u32, h: u32, rgb: &[u8]) {
    let image = image::RgbImage::from_raw(w, h, rgb.to_vec()).expect("picture size");
    image.save(path).expect("couldn't save the picture");
}

/// --shoot OUT.png PAGE [SCALE] [WIDTH HEIGHT]
pub fn shoot(args: &[String]) {
    let out = args.first().cloned().unwrap_or_else(|| "shot.png".into());
    let page = args.get(1).cloned().unwrap_or_else(|| "home".into());
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let lw: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1240.0);
    let lh: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(860.0);
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let queue: Queue = Arc::default();
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
        for _ in 0..(after * 50.0) as usize {
            pump(&queue);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let rgb = render(&window, w, h);
    save_png(&out, w, h, &rgb);
    drop(ctx);
}

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
    std::mem::forget(timer);
}
