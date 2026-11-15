//! Startup animation: what the panels show when the laptop turns on (StartupTab in `tabs.py`). It is
//! saved on the panels themselves, so it needs the White Rabbit firmware.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::{AppWindow, StartupPage};

/// Seconds the panels stay with this window after saving, so the animation can play once
const PREVIEW_SECONDS: f64 = 10.0;
const RANDOM_SET: i32 = 2;

fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<StartupPage>().set_status(text.into());
}

fn show_modules(app: &AppWindow) {
    let ctx = ctx();
    let modules = ctx.modules.lock().unwrap().clone();
    let page = app.global::<StartupPage>();
    let current = modules.values().find_map(|m| m.startup);
    page.set_supported(current.is_some());
    if let Some((mode, animation, set)) = current {
        page.set_mode(mode as i32);
        if (animation as usize) < cmd::STARTUP_ANIMATIONS.len() {
            page.set_animation(animation as i32);
        }
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

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<StartupPage>();
    let names: Vec<SharedString> = cmd::STARTUP_ANIMATIONS.iter().map(|n| SharedString::from(*n)).collect();
    page.set_names(ModelRc::new(VecModel::from(names)));
    page.set_picked(ModelRc::new(VecModel::from(vec![true; cmd::STARTUP_ANIMATIONS.len()])));
    ctx.on_modules.lock().unwrap().push(Box::new(show_modules));
    {
        let ui = ctx.ui.clone();
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
        page.on_save(move || {
            let Some(app) = ctx.ui.upgrade() else { return };
            let page = app.global::<StartupPage>();
            let mask = page.get_picked().iter().enumerate().fold(0u32, |m, (i, on)| if on { m | 1 << i } else { m });
            let mode = page.get_mode();
            if mode == RANDOM_SET && mask == 0 {
                show(&app, "Tick at least one animation.");
                return;
            }
            let animation = page.get_animation().max(0) as u8;
            let modules = ctx.modules.lock().unwrap().clone();
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
            let status: crate::app::Status = Arc::new(|app: &AppWindow, text: String| {
                show(app, if text == "Done." { "Saved. Playing it now.".into() } else { text });
            });
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
