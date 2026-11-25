//! Startup animation: what the panels show when the laptop turns on (StartupTab in `tabs.py`). It is
//! saved on the panels themselves, so it needs the White Rabbit firmware.
//!
//! The page (`ui/pages/startup.slint`) has four choices, stored in `StartupPage.mode`: 0 = random (any
//! animation), 1 = always the one picked in the drop-down (`animation`), 2 = random out of the ticked
//! ones (`picked`, one tick box per entry of `cmd::STARTUP_ANIMATIONS`), 3 = none. Nothing is saved on
//! the computer: Save sends one "startup animation" command (id 0x21) to each ticked panel that has the
//! White Rabbit firmware, and the panel keeps the choice in its own flash.
//!
//! What the page shows comes from `Ctx::modules`, the per-panel firmware information that
//! `Ctx::refresh_modules` reads on a worker thread (it asks each panel for its version and current
//! startup setting). When that information changes, `show_modules` is called through `ctx.on_modules`.
//! Saving runs through `Ctx::act`, which borrows the panels from the monitor and writes on a worker
//! thread, because a serial write can take a while and must not freeze the window.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::{AppWindow, StartupPage};

/// Seconds the panels stay with this window after saving, so the animation can play once
/// How long `Ctx::act` keeps the panels borrowed after the write before giving them back to the monitor.
/// The firmware plays the new animation straight away when it gets the command, so for these seconds
/// the user can watch it without the monitor drawing over it.
const PREVIEW_SECONDS: f64 = 10.0;
/// The `mode` value for "random out of these" (see the file header). Only this mode uses the tick boxes,
/// so only it needs at least one ticked.
const RANDOM_SET: i32 = 2;

/// Sets the status line under the Save button.
fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<StartupPage>().set_status(text.into());
}

/// Updates the page from what the panels reported (registered in `ctx.on_modules`, so it runs on the UI
/// thread every time the firmware information is refreshed). Shows the setting of the first panel that
/// answered; the panels normally share one setting because Save writes to all ticked panels at once.
/// When no panel answered the question, the status line explains why: another program holds the port,
/// there are no panels, or the firmware is too old to know the command.
fn show_modules(app: &AppWindow) {
    let ctx = ctx();
    let modules = ctx.modules.lock().unwrap().clone();
    let page = app.global::<StartupPage>();
    // `startup` is (mode, animation, set) from `cmd::parse_startup`; None for firmware that doesn't
    // understand the question. HashMap order is arbitrary, so with two panels that disagree this may
    // show either one.
    let current = modules.values().find_map(|m| m.startup);
    page.set_supported(current.is_some());
    if let Some((mode, animation, set)) = current {
        page.set_mode(mode as i32);
        // Newer firmware may know animations this window doesn't; leave the drop-down alone then.
        if (animation as usize) < cmd::STARTUP_ANIMATIONS.len() {
            page.set_animation(animation as i32);
        }
        // `set` is a bit mask: bit i on means animation i is in the "random out of these" set.
        let picked: Vec<bool> = (0..cmd::STARTUP_ANIMATIONS.len()).map(|i| set & (1 << i) != 0).collect();
        page.set_picked(ModelRc::new(VecModel::from(picked)));
        show(app, "");
    } else if modules.values().any(|m| m.in_use) {
        show(app, "Another program is using the panels. Close it, then click Check again on the Firmware page.");
    } else if modules.is_empty() {
        show(app, "No LED Matrix panels found.");
    } else {
        show(app, "The panels need the White Rabbit firmware for this (Settings > Firmware).");
    }
}

/// Connects the Startup page: fills the animation names, starts with every tick box on, registers
/// `show_modules` for firmware updates, and connects `picked-changed` and `save`. Runs once at
/// start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<StartupPage>();
    let names: Vec<SharedString> = cmd::STARTUP_ANIMATIONS.iter().map(|n| SharedString::from(*n)).collect();
    page.set_names(ModelRc::new(VecModel::from(names)));
    page.set_picked(ModelRc::new(VecModel::from(vec![true; cmd::STARTUP_ANIMATIONS.len()])));
    ctx.on_modules.lock().unwrap().push(Box::new(show_modules));
    {
        let ui = ctx.ui.clone();
        // `picked-changed(i, on)`: a tick box was clicked. The Slint side hands the change to Rust,
        // which stores it in the `picked` model so that Save (below) reads the up-to-date ticks.
        page.on_picked_changed(move |i, on| {
            let Some(app) = ui.upgrade() else { return };
            let picked = app.global::<StartupPage>().get_picked();
            if (i as usize) < picked.row_count() {
                picked.set_row_data(i as usize, on);
            }
        });
    }
    {
        let ctx = ctx.clone();
        // `save()`: build the command from the page and send it to the ticked panels that support it.
        page.on_save(move || {
            let Some(app) = ctx.ui.upgrade() else { return };
            let page = app.global::<StartupPage>();
            // Turn the tick boxes back into the bit mask the firmware expects (bit i = animation i).
            let mask = page.get_picked().iter().enumerate().fold(0u64, |m, (i, on)| if on { m | 1 << i } else { m });
            let mode = page.get_mode();
            if mode == RANDOM_SET && mask == 0 {
                show(&app, "Tick at least one animation.");
                return;
            }
            // The drop-down reports -1 when nothing is selected; send animation 0 then.
            let animation = page.get_animation().max(0) as u8;
            let modules = ctx.modules.lock().unwrap().clone();
            // Only panels whose firmware answered the startup question can store it; others would ignore or
            // misread the command.
            let supported: Vec<_> = ctx
                .selected_panels()
                .into_iter()
                .filter(|p| modules.get(&p.device).is_some_and(|m| m.startup.is_some()))
                .collect();
            if supported.is_empty() {
                show(&app, "None of the ticked panels has the White Rabbit firmware.");
                return;
            }
            show(&app, "Saving...");
            // `Ctx::act` reports "Done." on success; reword that for this page, pass errors through as they are.
            let status: crate::app::Status = Arc::new(|app: &AppWindow, text: String| {
                show(app, if text == "Done." { "Saved. Playing it now.".into() } else { text });
            });
            // Build the bytes once here; the closure below runs on a worker thread for each panel. The
            // arguments to `act`: these panels, borrow them from the monitor (true), give them back after
            // PREVIEW_SECONDS, and don't insist on exactly one panel (false).
            let command = cmd::set_startup_animation(mode as u8, animation, mask);
            ctx.act(
                move |s, p| s.write(&p.device, std::slice::from_ref(&command)).map(|_| ()),
                status,
                Some(supported),
                true,
                Some(PREVIEW_SECONDS),
                false,
            );
        });
    }
}
