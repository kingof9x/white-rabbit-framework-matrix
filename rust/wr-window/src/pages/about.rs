//! About White Rabbit: the version and the projects it comes from (`white_rabbit/info_tab.py`).
//! Checking for updates moved to the Settings page (`updates.rs`).

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::Ctx;
use crate::{AboutPage, AppWindow, Credit};

/// The projects White Rabbit is based on or inspired by: (name, url, what came from it)
pub const CREDITS: [(&str, &str, &str); 8] = [
    ("led-matrix by MidnightJava", "https://github.com/MidnightJava/led-matrix",
     "The system monitor: apps on the panels, plugins and the layout file."),
    ("LED System Monitor by Jeremy", "https://code.karsttech.com/jeremy/FW_LED_System_Monitor",
     "The original system monitor that led-matrix grew from."),
    ("inputmodule-rs by Framework", "https://github.com/FrameworkComputer/inputmodule-rs",
     "The LED Matrix firmware and the LED Matrix Control app: patterns, games and firmware updates."),
    ("Framework-LED-Matrix by mariobx", "https://github.com/mariobx/Framework-LED-Matrix",
     "The ideas for the Math page (written from scratch, no code copied)."),
    ("pixeled by patate (slowpaste)", "https://github.com/slowpaste/pixeled",
     "The ideas for the Effects page, the sand battery and now playing apps (written from scratch)."),
    ("Framework16 LED Matrix Manager by Deo", "https://github.com/Deovandski/Deo-s-Framework16-LED-Matrix-Manager",
     "The ideas for the animations, the old-TV effect, mirror mode, the keyboard gap, panel names and the Draw page (our own code)."),
    ("Slint", "https://slint.dev",
     "The toolkit this window is built with (royalty-free licence)."),
    ("Fluent UI System Icons by Microsoft", "https://github.com/microsoft/fluentui-system-icons",
     "The icons in this window (MIT licence)."),
];

pub fn wire(_ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<AboutPage>();
    page.set_version(crate::version().into());
    let credits: Vec<Credit> =
        CREDITS.iter().map(|(n, u, w)| Credit { name: (*n).into(), url: (*u).into(), what: (*w).into() }).collect();
    page.set_credits(ModelRc::new(VecModel::from(credits)));
}
