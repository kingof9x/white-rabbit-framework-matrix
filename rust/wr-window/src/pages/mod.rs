//! The pages of the window, one module each: what each page shows and what its buttons do.

pub mod about;
pub mod effects;
pub mod monitor;
pub mod music;
pub mod firmware;
pub mod shortcuts;
pub mod startup;

use std::rc::Rc;

use crate::app::Ctx;
use crate::AppWindow;

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    firmware::wire(ctx, app);
    startup::wire(ctx, app);
    effects::wire(ctx, app);
    music::wire(ctx, app);
    shortcuts::wire(ctx, app);
    about::wire(ctx, app);
}
