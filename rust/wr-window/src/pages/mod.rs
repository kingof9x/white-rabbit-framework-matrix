//! The pages of the window, one module each: what each page shows and what its buttons do.

pub mod about;
pub mod monitor;
pub mod firmware;
pub mod shortcuts;

use std::rc::Rc;

use crate::app::Ctx;
use crate::AppWindow;

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    firmware::wire(ctx, app);
    shortcuts::wire(ctx, app);
    about::wire(ctx, app);
}
