//! WhiteRabbitWindow.exe: White Rabbit's window, in Rust. WhiteRabbit.exe (the monitor and tray icon)
//! starts it with --window, and hands it these too:
//!
//!   --window [--wait-for-monitor] [--tab PAGE]   open the window (on PAGE)
//!   --edit-layout                                open the window on the System monitor page
//!   --open-lhm / --find-lhm                      open Libre Hardware Monitor (asking where it is)
//!   --open-framework-control                     open Framework Control
//!   --diagnose / --list-apps                     print a report and exit
//!
//! For screenshots and tests away from Windows: --shoot OUT.png PAGE [SCALE] draws a page into a picture.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Style lints we choose not to follow: callback types are spelled out where they are used, and some
// loops index several arrays at once.
#![allow(clippy::type_complexity, clippy::needless_range_loop)]

slint::include_modules!();

mod app;
mod commands;
mod dialogs;
mod drawings;
mod external;
mod font;
mod formula;
mod games;
mod hardware;
mod jobs;
mod keyboard;
mod lead_in;
mod link;
mod maths;
mod pages;
mod panels;
mod paths;
mod preview;
mod report;
mod settings;
mod shoot;
mod span;
mod theme;
mod winapps;

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

/// --tab names (older names too) -> pages
pub fn start_page(tab: &str) -> &'static str {
    match tab {
        "panels" | "monitor" => "monitor",
        "startup" => "startup",
        "play" | "games" => "games",
        "math" => "math",
        "draw" => "draw",
        "effects" => "effects",
        "messages" | "text" => "messages",
        "transitions" => "transitions",
        "audio" | "music" => "music",
        "firmware" => "firmware",
        "shortcuts" => "shortcuts",
        "info" | "about" => "about",
        "settings" => "settings",
        _ => "home",
    }
}

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// The app's full name, as Windows shows it.
pub const APP_TITLE: &str = "White Rabbit - Framework 16 LED Control";

/// "White Rabbit - Framework 16 LED Control 1.5": the title bar shows which version is running.
pub fn window_title() -> String {
    format!("{APP_TITLE} {}", version())
}

/// This copy's version: from the VERSION file next to the exe, else the release version the build was
/// given (WHITE_RABBIT_VERSION), else the test-build version in pyproject.toml.
pub fn version() -> String {
    if let Ok(v) = std::fs::read_to_string(paths::app_dir().join("VERSION")) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(v) = option_env!("WHITE_RABBIT_VERSION").filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    const PYPROJECT: &str = include_str!("../../../pyproject.toml");
    PYPROJECT
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("version")
                .map(|r| r.trim().trim_start_matches('=').trim().trim_matches('"').to_string())
        })
        .unwrap_or_default()
}

/// Let a windowless exe print to the console it was started from (for --diagnose and --list-apps).
fn attach_console() {
    #[cfg(windows)]
    // SAFETY: plain call; failing just means there is no console.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
}

/// If the window is already open, bring it to the front. True if it was.
#[cfg(windows)]
fn focus_open_window() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let name = wide(paths::WINDOW_MUTEX);
    // SAFETY: the name is null-terminated. The handle is kept (never closed) while this window runs.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: plain call, right after CreateMutexW.
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        let _ = handle;
        return false;
    }
    let title = wide(&window_title());
    // SAFETY: the title is null-terminated; the window handle is checked before use.
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
    true
}

#[cfg(not(windows))]
fn focus_open_window() -> bool {
    false
}

/// Started together with the monitor: let it open the panels first, so the two don't collide.
fn wait_for_monitor(seconds: u64) {
    let link = link::Link::default();
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if link.running(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Builds the window and every page.
pub fn build(start: &str) -> (AppWindow, Rc<app::Ctx>) {
    let ui = AppWindow::new().expect("the window couldn't be made");
    let g = ui.global::<App>();
    g.set_title(window_title().into());
    let choice = theme::load_choice();
    g.set_theme_choice(theme::CHOICES.iter().position(|c| *c == choice).unwrap_or(0) as i32);
    theme::apply(&ui, &choice, &theme::read_windows());
    let ctx = app::Ctx::new(&ui);
    app::wire(&ctx, &ui);
    pages::wire(&ctx, &ui);
    ctx.update_bar(&ui);
    g.set_page(start.into());
    ctx.refresh_modules();
    keyboard::wire(&ui);
    hardware::wire(&ui);
    (ui, ctx)
}

/// Draws the window with the graphics card (OpenGL, through FemtoVG). The software renderer used before
/// redrew only the parts it thought had changed, and on Windows that left old pixels behind (a closed
/// drop-down list still showing, parts of a page not updating); it also drew every pixel on the CPU, so
/// a maximised window lagged. If OpenGL can't start, Slint falls back to the software renderer by
/// itself. WHITE_RABBIT_RENDERER=software (or femtovg) picks one by hand, for testing.
fn pick_renderer() {
    let wanted = std::env::var("WHITE_RABBIT_RENDERER").ok().filter(|r| !r.trim().is_empty());
    let name = wanted.as_deref().map(str::trim).unwrap_or("femtovg").to_string();
    match slint::BackendSelector::new().renderer_name(name.clone()).select() {
        Ok(()) => wr_core::info!("Window renderer: {name}"),
        Err(e) => {
            wr_core::warn!("Couldn't use the {name} renderer ({e}); using the software renderer");
            if let Err(e) = slint::BackendSelector::new().renderer_name("software".into()).select() {
                wr_core::warn!("Couldn't pick the software renderer either: {e}");
            }
        }
    }
}

fn run_window(args: &[String]) {
    if focus_open_window() {
        return;
    }
    if args.iter().any(|a| a == "--wait-for-monitor") {
        wait_for_monitor(10);
    }
    let start = if args.iter().any(|a| a == "--edit-layout") {
        "monitor"
    } else {
        start_page(&arg_after(args, "--tab").unwrap_or_default())
    };
    pick_renderer();
    let (ui, ctx) = build(start);
    if std::env::var_os("WHITE_RABBIT_SMOKE_TEST").is_some() {
        shoot::smoke_test(&ui, &ctx);
    }
    if let Err(e) = ui.show() {
        // Say why in the log (for example, the graphics driver refused), instead of a bare crash
        wr_core::error!("The window couldn't open: {e}. Setting WHITE_RABBIT_RENDERER=software may help.");
        return;
    }
    theme::title_bar(&ui, ui.global::<Theme>().get_dark());
    let close_ctx = ctx.clone();
    ui.window().on_close_requested(move || {
        jobs::stop_all();
        keyboard::stop();
        pages::monitor::editor::apply_on_close();
        let borrowed: Vec<_> =
            close_ctx.panels().into_iter().filter(|p| close_ctx.lender.is_borrowed(&p.device)).collect();
        if !borrowed.is_empty() {
            // Give the panels back before closing; the monitor also notices a closed window by itself
            close_ctx.lender.give_back(&close_ctx.sender, &borrowed, None);
        }
        slint::CloseRequestResponse::HideWindow
    });
    if let Err(e) = slint::run_event_loop() {
        wr_core::error!("The window stopped with an error: {e}");
    }
    drop(ctx);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    wr_core::log::init(paths::log_file());
    if args.first().map(String::as_str) == Some("--shoot") {
        shoot::shoot(&args[1..]);
        return;
    }
    std::panic::set_hook(Box::new(|info| wr_core::error!("The window stopped: {info}")));
    wr_core::info!("White Rabbit window {} starting: {}", version(), args.join(" "));
    if args.iter().any(|a| a == "--diagnose" || a == "--list-apps") {
        attach_console();
        report::run(&args);
        return;
    }
    // The tray's Libre Hardware Monitor and Framework Control items
    for (flag, which) in [("--find-lhm", "find-lhm"), ("--open-lhm", "lhm"), ("--open-framework-control", "fc")] {
        if args.iter().any(|a| a == flag) {
            external::open_from_tray(which);
            return;
        }
    }
    run_window(&args);
}

/// Screenshots: put the window in a state first (WHITE_RABBIT_SHOOT_ACTION).
pub fn shoot_action(ui: &AppWindow, _ctx: &Rc<app::Ctx>, action: &str) {
    let g = ui.global::<App>();
    match action {
        "name-panels" => g.invoke_name_panels(),
        "music-on" => g.set_music_mode(true),
        // hearing:TEXT shows TEXT under the Audio page's sound check
        a if a.starts_with("hearing:") => ui.global::<MusicPage>().set_hearing(a[8..].into()),
        // edit:SIDE:PAGE:SLOT selects a slot in the layout editor, e.g. edit:0:2:top
        a if a.starts_with("edit:") => {
            let parts: Vec<&str> = a.split(':').collect();
            if let [_, side, page, slot] = parts[..] {
                let e = ui.global::<EditorPage>();
                e.invoke_page_chosen(side.parse().unwrap_or(0), page.parse().unwrap_or(0));
                e.invoke_slot_clicked(side.parse().unwrap_or(0), slot.into());
            }
        }
        // teach: the Effects page's port teacher
        "teach" => ui.global::<EffectsPage>().invoke_teach(),
        // math-effect:N starts one of the Math page's "More maths"
        a if a.starts_with("math-effect:") => {
            let m = ui.global::<MathPage>();
            m.set_effect(a[12..].parse().unwrap_or(0));
            m.invoke_start_effect();
        }
        // draw-open:N opens a saved drawing on the Draw page
        a if a.starts_with("draw-open:") => {
            let d = ui.global::<DrawPage>();
            d.set_chosen(a[10..].parse().unwrap_or(0));
            d.invoke_open();
        }
        "math-graph" => ui.global::<MathPage>().invoke_show_graph(),
        // kb-check-firmware: the keyboard Firmware page's Check for updates (sample answer with --demo)
        "kb-check-firmware" => ui.global::<crate::Kb>().invoke_check_firmware(),
        // updates-demo:CASE fills Settings > Updates with sample answers (no panels or internet needed)
        a if a.starts_with("updates-demo:") => pages::updates::demo(ui, _ctx, &a[13..]),
        // game:N starts a game on the Games & patterns page
        a if a.starts_with("game:") => ui.global::<GamesPage>().invoke_start(a[5..].parse().unwrap_or(0)),
        _ => wr_core::warn!("Unknown screenshot action {action}"),
    }
}
