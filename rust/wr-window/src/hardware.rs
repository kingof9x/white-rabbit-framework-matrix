//! Home's "Your hardware": the LED panels, keyboard and macropad or numpad that are plugged in, numbered
//! in the order White Rabbit found them since the window opened. Something unplugged leaves the list;
//! plugged in again, it goes to the end.

//!
//! Also draws the two small LED panel pictures on Home that copy what the real panels show right now:
//! every 150 ms a worker thread asks the monitor for its "frames" (the last frame it sent to each
//! port, as hex), and the UI thread turns any frame that changed into a picture (`preview.rs`).
//!
//! Everything here runs on the UI thread from one Slint timer, except the "frames" request, which
//! blocks for up to a second and so runs on its own short-lived thread. Its state lives in
//! thread-locals because Slint objects can't leave the UI thread; the one flag shared with the worker
//! (`ASKING`) is an atomic. The list is built from what other parts of the window already show (the
//! panel cards from `app.rs`, the keyboard page's device list), so this file talks to no hardware.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{App, AppWindow, Hardware, HwRow, Kb};

thread_local! {
    /// What was found, in the order it was found ("left", "right", "Keyboard", "Macropad", "Numpad").
    static ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The timer that drives the refreshes; kept here so it lives as long as the UI thread.
    static TIMER: slint::Timer = slint::Timer::default();
}

/// One thing that is plugged in: its key in ORDER, then the row without its number.
struct Found {
    /// "left", "right", or the keyboard page's device name ("Keyboard", "Macropad", "Numpad")
    key: String,
    /// The row as shown on Home, with an empty number until `refresh` numbers it
    row: HwRow,
}

/// "left" or "right" from a panel card's label (e.g. "Left panel (COM4)" or "Desk (left, COM4)").
/// Anything that doesn't mention "right" counts as left.
fn side_of(label: &str) -> &'static str {
    let l = label.to_lowercase();
    if l.contains("right") {
        "right"
    } else {
        "left"
    }
}

/// Everything plugged in right now: the panels from the menu's panel list, then the keyboard and
/// macropad / numpad the keyboard page has found (ones marked "Not found" are left out).
fn found(app: &AppWindow) -> Vec<Found> {
    let g = app.global::<App>();
    let kb = app.global::<Kb>();
    let mut out = vec![];
    for p in g.get_panels().iter() {
        let side = side_of(&p.label);
        // "Showing the system monitor.  Firmware: 0.2.0" -> "COM4, firmware 0.2.0"
        let firmware = p.doing.split("Firmware: ").nth(1).map(|f| format!(", firmware {}", f.trim_end_matches('.')));
        out.push(Found {
            key: side.to_string(),
            row: HwRow {
                order: "".into(),
                name: format!("LED Matrix, {side}").into(),
                detail: format!("{}{}", p.device, firmware.unwrap_or_default()).into(),
                status: "Connected".into(),
                ok: true,
                kind: "panel".into(),
            },
        });
    }
    for d in kb.get_devices().iter() {
        if d.status == "Not found" {
            continue;
        }
        let name = d.name.to_string();
        // The keyboard's detail is its kind (the keyboard page's type choice: 0 clear RGB, 1 black RGB,
        // 2 white backlight); a macropad or numpad's detail is which side of the keyboard it is on.
        let detail = match name.as_str() {
            "Keyboard" => match kb.get_kb_type() {
                1 => "Black RGB keys",
                2 => "Black keys, white backlight",
                _ => "Clear RGB keys",
            }
            .to_string(),
            _ => (if kb.get_module_left() { "On the left" } else { "On the right" }).to_string(),
        };
        let kind = if name == "Keyboard" { "keyboard" } else { "macropad" };
        let name = match name.as_str() {
            "Macropad" => "RGB Macropad".to_string(),
            _ => name,
        };
        out.push(Found {
            key: d.name.to_string(),
            row: HwRow {
                order: "".into(),
                name: name.into(),
                detail: detail.into(),
                status: d.status.clone(),
                ok: d.ok,
                kind: kind.into(),
            },
        });
    }
    out
}

/// Looks at what is plugged in and updates the list and the picture.
pub fn refresh(app: &AppWindow) {
    let now = found(app);
    let order = ORDER.with(|o| {
        let mut o = o.borrow_mut();
        // Drop what was unplugged, then add anything new at the end; what stays keeps its place, so the
        // numbers on Home don't jump around when something else is plugged in.
        o.retain(|k| now.iter().any(|f| &f.key == k));
        for f in &now {
            if !o.contains(&f.key) {
                o.push(f.key.clone());
            }
        }
        o.clone()
    });
    let number = |key: &str| order.iter().position(|k| k == key).map(|i| (i + 1).to_string()).unwrap_or_default();
    let mut rows: Vec<HwRow> = now.into_iter().map(|f| HwRow { order: number(&f.key).into(), ..f.row }).collect();
    rows.sort_by_key(|r| r.order.parse::<usize>().unwrap_or(0));
    let h = app.global::<Hardware>();
    h.set_left_panel(number("left").into());
    h.set_right_panel(number("right").into());
    h.set_keyboard(number("Keyboard").into());
    let module = number("Macropad");
    // The picture has one spot for the module beside the keyboard: the macropad, else the numpad.
    h.set_module(if module.is_empty() { number("Numpad") } else { module }.into());
    // Replace the list only when it changed: a new model makes Slint rebuild the rows, which would
    // flicker twice a second otherwise.
    if !h.get_rows().iter().eq(rows.iter().cloned()) {
        h.set_rows(ModelRc::new(VecModel::from(rows)));
    }
}

///
/// In points: 6-point LEDs 8 apart, a 3-point margin, one panel per picture (so no gap).
/// How the panels are drawn on Home: the keyboard picture's LED size and spacing.
const STYLE: crate::preview::Style = crate::preview::Style { dot: 6.0, step: 8, margin: 3, gap: 0 };

/// One panel's picture at 2 pixels per point.
fn panel_image(levels: &crate::preview::Levels) -> slint::Image {
    crate::preview::render_styled(std::slice::from_ref(levels), 2, STYLE)
}

/// A frame from the monitor (column by column, as hex) as levels. The brightest LED is drawn at full
/// brightness, so the picture reads the same when the panels are dimmed for a dark room.
fn levels_of_hex(hex: &str) -> Option<crate::preview::Levels> {
    let bytes = crate::link::unhex(hex);
    if bytes.len() != wr_core::WIDTH * wr_core::HEIGHT {
        return None;
    }
    // At least 32, so a nearly dark frame (faint leftovers) isn't blown up to full brightness. The bytes
    // are column by column: column x is bytes x*34 .. x*34 + 34.
    let full = bytes.iter().copied().max().unwrap_or(0).max(32) as f64;
    let mut out = crate::preview::dark();
    for (x, col) in out.iter_mut().enumerate() {
        for (y, v) in col.iter_mut().enumerate() {
            *v = bytes[x * wr_core::HEIGHT + y] as f64 / full;
        }
    }
    Some(out)
}

thread_local! {
    /// The frames last drawn on Home, [left, right], so an unchanged frame isn't drawn again.
    static SHOWN: RefCell<[Option<String>; 2]> = const { RefCell::new([None, None]) };
}

///
/// True while a request is on its way, so a slow monitor never has more than one request waiting:
/// the timer's next ticks skip until the answer comes.
/// Asking the monitor for the panels' frames (one question at a time).
static ASKING: AtomicBool = AtomicBool::new(false);

///
/// On the UI thread. A panel with no frame in the reply (the monitor isn't running, or doesn't draw
/// on it) is shown dark. Frames are compared as hex text with what is shown, and only a changed one
/// is turned into a new picture.
/// Draws what each panel shows now, from the monitor's "frames" reply ({port: hex}).
fn show_frames(app: &AppWindow, frames: &serde_json::Map<String, serde_json::Value>) {
    let h = app.global::<Hardware>();
    let mut wanted: [Option<String>; 2] = [None, None];
    for p in app.global::<App>().get_panels().iter() {
        // 0 for the left panel, 1 for the right; matched to the reply by port name, ignoring case (COM4/com4).
        let side = usize::from(side_of(&p.label) == "right");
        wanted[side] = frames
            .iter()
            .find(|(port, _)| port.eq_ignore_ascii_case(&p.device))
            .and_then(|(_, hex)| hex.as_str())
            .map(str::to_string);
    }
    SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        for side in 0..2 {
            if shown[side] == wanted[side] {
                continue;
            }
            let levels = wanted[side].as_deref().and_then(levels_of_hex).unwrap_or_else(crate::preview::dark);
            let image = panel_image(&levels);
            if side == 0 {
                h.set_left_image(image);
            } else {
                h.set_right_image(image);
            }
            shown[side] = wanted[side].clone();
        }
    });
}

/// While Home is open, asks the monitor what the panels show (in the background: the window never waits).
fn ask_for_frames(app: &AppWindow) {
    if app.global::<App>().get_page() != "home" || app.global::<App>().get_panels().row_count() == 0 {
        return;
    }
    // Set the flag and see whether it was already set, in one step, so two ticks can't both start one.
    if ASKING.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = app.as_weak();
    std::thread::spawn(move || {
        // One second at most; with no monitor, `request` fails quickly (no pipe) and the panels show dark.
        let reply = crate::link::request(serde_json::json!({"cmd": "frames"}), Duration::from_secs(1));
        ASKING.store(false, Ordering::Release);
        let frames = reply.and_then(|r| r.get("frames").and_then(|f| f.as_object().cloned())).unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |app| show_frames(&app, &frames));
    });
}

///
/// Called once by `build` in main.rs. One timer ticks every 150 ms: each tick asks for the frames
/// (about 6.7 times a second, only while Home is shown) and every third tick (450 ms) refreshes the list.
/// The LED panel pictures, a look at what is plugged in twice a second, and the panels' frames about
/// six times a second while Home is open.
pub fn wire(app: &AppWindow) {
    let dark = panel_image(&crate::preview::dark());
    app.global::<Hardware>().set_left_image(dark.clone());
    app.global::<Hardware>().set_right_image(dark);
    refresh(app);
    let weak = app.as_weak();
    let mut ticks = 0u32;
    TIMER.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_millis(150), move || {
            if let Some(app) = weak.upgrade() {
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(3) {
                    refresh(&app);
                }
                ask_for_frames(&app);
            }
        })
    });
}

/// Tests for turning the monitor's frames into Home's pictures.
#[cfg(test)]
mod tests {
    use super::*;

    /// A frame from dimmed panels is scaled so its brightest LED is full brightness, and a frame of the
    /// wrong length is refused.
    #[test]
    fn a_dimmed_frame_is_drawn_at_full_brightness() {
        let mut bytes = vec![0u8; wr_core::WIDTH * wr_core::HEIGHT];
        // Column 2, row 5 lit at 80 (the panels dimmed), column 0 row 0 at half that
        bytes[2 * wr_core::HEIGHT + 5] = 80;
        bytes[0] = 40;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let levels = levels_of_hex(&hex).unwrap();
        assert_eq!(levels[2][5], 1.0);
        assert_eq!(levels[0][0], 0.5);
        assert_eq!(levels[1][1], 0.0);
        assert!(levels_of_hex("00ff").is_none());
    }
}
