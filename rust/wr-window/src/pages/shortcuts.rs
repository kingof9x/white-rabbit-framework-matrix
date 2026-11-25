//! Keyboard shortcuts: saved as "shortcuts": {letter: true/false} in settings.json (a shortcut not
//! listed is on); the monitor reads them again when told ("shortcuts" command).
//!
//! The shortcuts themselves (Ctrl+Alt+Shift+letter) are registered and handled by the monitor process
//! (WhiteRabbit.exe), not by this window. This page only switches each one on or off:
//! `ui/pages/shortcuts.slint` shows one row per shortcut with a switch, and its `toggled(index, on)`
//! callback lands in `wire` below. That handler runs on the UI thread, writes the whole on/off map to
//! settings.json straight away (a small local file, fast enough for the UI thread), and then sends the
//! monitor a "shortcuts" control message on a worker thread so it re-reads the file. The message goes
//! to a worker because the monitor might be busy or not running, and `link::request` can wait up to
//! the 5 second timeout before giving up; the window must not freeze meanwhile.

use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::Ctx;
use crate::{settings, AppWindow, ShortcutRow, ShortcutsPage};

/// (letter, name, what it does)
/// The shortcuts the page lists: (letter, name, what it does). The letter is both the key pressed with
/// Ctrl+Alt+Shift and the key used in settings.json, so it must match what the monitor registers.
/// The order here is the order of the rows on the page and of the `Vec<bool>` from `load`.
pub const SHORTCUTS: [(&str, &str, &str); 5] = [
    ("i", "Show app names", "Shows the name of each app on the panels for a few seconds."),
    ("n", "Next app", "Skips to the next app on both panels."),
    ("f", "Freeze app switching", "Keeps the current apps on the panels instead of rotating."),
    ("u", "Unfreeze app switching", "Goes back to rotating through the apps."),
    ("m", "Audio reactivity", "Turns audio reactivity on or off: the audio visualizer on both panels."),
];

/// The key combination shown on a row, e.g. "Ctrl+Alt+Shift+I" for "i".
pub fn keys_text(letter: &str) -> String {
    format!("Ctrl+Alt+Shift+{}", letter.to_uppercase())
}

/// Reads which shortcuts are on from settings.json, in the order of `SHORTCUTS`. A missing
/// "shortcuts" entry, a letter that isn't listed, or a value that isn't true/false all count as on,
/// so a fresh install (or a hand-edited file with a typo) has every shortcut working.
pub fn load() -> Vec<bool> {
    let saved = match settings::get("shortcuts") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    SHORTCUTS.iter().map(|(l, _, _)| saved.get(*l).map(|v| v.as_bool().unwrap_or(true)).unwrap_or(true)).collect()
}

/// Builds the shortcut rows from `SHORTCUTS` and the saved settings, and connects the page's
/// `toggled` callback. Runs once at start-up on the UI thread.
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
    // `toggled(i, on)`: the user flipped the switch on row `i`. Runs on the UI thread. Saves the new
    // state, then tells the monitor in the background and shows a status line when it answers.
    page.on_toggled(move |i, on| {
        let Some(app) = ctx.ui.upgrade() else { return };
        let page = app.global::<ShortcutsPage>();
        let rows = page.get_rows();
        // Copy the switch position into the model first, so the saved map below includes this change.
        // An out-of-range index (shouldn't happen) is simply ignored.
        if let Some(mut row) = rows.row_data(i as usize) {
            row.on = on;
            rows.set_row_data(i as usize, row);
        }
        // Save every letter explicitly (not just the changed one), so the file always lists all of them.
        let enabled: Map<String, Value> = rows.iter().map(|r| (r.letter.to_string(), Value::Bool(r.on))).collect();
        if !settings::set("shortcuts", Value::Object(enabled)) {
            page.set_status("Couldn't save the change (see the log).".into());
            return;
        }
        // A short summary of what is on now, for the status line after the monitor answers.
        let on: Vec<String> = rows.iter().filter(|r| r.on).map(|r| r.keys.to_string()).collect();
        let summary =
            if on.is_empty() { "All shortcuts are off.".to_string() } else { format!("On: {}", on.join(", ")) };
        // The monitor re-reads settings.json when it gets {"cmd": "shortcuts"}. No answer within 5 seconds
        // (or no monitor at all) is not an error: the setting is already saved and the monitor reads it at
        // start-up, so we only add a note saying so.
        ctx.in_background(
            || crate::link::request(json!({"cmd": "shortcuts"}), Duration::from_secs(5)),
            move |app, reply| {
                let note =
                    if reply.is_some() { "" } else { " The monitor isn't running; it will use this when it starts." };
                app.global::<ShortcutsPage>().set_status(format!("Saved. {summary}{note}").into());
            },
        );
    });
}
