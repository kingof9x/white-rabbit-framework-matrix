//! The pages of the window, one module each: what each page shows and what its buttons do.
//!
//! Each page module has a `wire` function that connects the page's Slint callbacks (button presses,
//! slider moves) to Rust code, and fills the page's properties from the saved settings. `wire` runs
//! once at start-up, on the UI thread, before the window is shown. Work that would block the UI
//! (serial writes, network, file scans) is passed to a worker thread by the pages themselves (see
//! `app.rs` for `Ctx` and the job helpers).

pub mod about;
pub mod draw;
pub mod effects;
pub mod firmware;
pub mod games;
pub mod math;
pub mod monitor;
pub mod music;
pub mod shortcuts;
pub mod startup;
pub mod updates;

use std::rc::Rc;

use crate::app::Ctx;
use crate::AppWindow;

/// Connects every page to the window. Called once from `main.rs` after the window is created.
/// The order doesn't matter for behaviour: each page only touches its own callbacks and properties.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    firmware::wire(ctx, app);
    startup::wire(ctx, app);
    effects::wire(ctx, app);
    music::wire(ctx, app);
    games::wire(ctx, app);
    math::wire(ctx, app);
    draw::wire(ctx, app);
    monitor::editor::wire(ctx, app);
    shortcuts::wire(ctx, app);
    about::wire(ctx, app);
    updates::wire(ctx, app);
}
