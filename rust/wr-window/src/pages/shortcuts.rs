//! Keyboard shortcuts: saved as "shortcuts": {letter: true/false} in settings.json (a shortcut not
//! listed is on); the monitor reads them again when told ("shortcuts" command).

use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::Ctx;
use crate::{settings, AppWindow, ShortcutRow, ShortcutsPage};

/// (letter, name, what it does)
pub const SHORTCUTS: [(&str, &str, &str); 5] = [
    ("i", "Show app names", "Shows the name of each app on the panels for a few seconds."),
    ("n", "Next app", "Skips to the next app on both panels."),
    ("f", "Freeze app switching", "Keeps the current apps on the panels instead of rotating."),
    ("u", "Unfreeze app switching", "Goes back to rotating through the apps."),
    ("m", "Music mode", "Turns music mode on or off: the audio visualizer on both panels."),
];

pub fn keys_text(letter: &str) -> String {
    format!("Ctrl+Alt+Shift+{}", letter.to_uppercase())
}

pub fn load() -> Vec<bool> {
    let saved = match settings::get("shortcuts") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    SHORTCUTS.iter().map(|(l, _, _)| saved.get(*l).map(|v| v.as_bool().unwrap_or(true)).unwrap_or(true)).collect()
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<ShortcutsPage>();
    let on = load();
    let rows: Vec<ShortcutRow> = SHORTCUTS
        .iter()
        .zip(on)
        .map(|((letter, name, what), on)| ShortcutRow {
            letter: (*letter).into(),
            keys: keys_text(letter).into(),
            name: (*name).into(),
            what: (*what).into(),
            on,
        })
        .collect();
    page.set_rows(ModelRc::new(VecModel::from(rows)));
    let ctx = ctx.clone();
    page.on_toggled(move |i, on| {
        let Some(app) = ctx.ui.upgrade() else { return };
        let page = app.global::<ShortcutsPage>();
        let rows = page.get_rows();
        if let Some(mut row) = rows.row_data(i as usize) {
            row.on = on;
            rows.set_row_data(i as usize, row);
        }
        let enabled: Map<String, Value> = rows.iter().map(|r| (r.letter.to_string(), Value::Bool(r.on))).collect();
        if !settings::set("shortcuts", Value::Object(enabled)) {
            page.set_status("Couldn't save the change (see the log).".into());
            return;
        }
        let on: Vec<String> = rows.iter().filter(|r| r.on).map(|r| r.keys.to_string()).collect();
        let summary = if on.is_empty() { "All shortcuts are off.".to_string() } else { format!("On: {}", on.join(", ")) };
        ctx.in_background(
            || crate::link::request(json!({"cmd": "shortcuts"}), Duration::from_secs(5)),
            move |app, reply| {
                let note = if reply.is_some() { "" } else { " The monitor isn't running; it will use this when it starts." };
                app.global::<ShortcutsPage>().set_status(format!("Saved. {summary}{note}").into());
            },
        );
    });
}
