//! Home's "Your hardware": the LED panels, keyboard and macropad or numpad that are plugged in, numbered
//! in the order White Rabbit found them since the window opened. Something unplugged leaves the list;
//! plugged in again, it goes to the end.

use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{App, AppWindow, Hardware, HwRow, Kb};

thread_local! {
    /// What was found, in the order it was found ("left", "right", "Keyboard", "Macropad", "Numpad").
    static ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static TIMER: slint::Timer = slint::Timer::default();
}

/// One thing that is plugged in: its key in ORDER, then the row without its number.
struct Found {
    key: String,
    row: HwRow,
}

fn side_of(label: &str) -> &'static str {
    let l = label.to_lowercase();
    if l.contains("right") {
        "right"
    } else {
        "left"
    }
}

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
    h.set_module(if module.is_empty() { number("Numpad") } else { module }.into());
    if !h.get_rows().iter().eq(rows.iter().cloned()) {
        h.set_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// The LED panel picture, and a look at what is plugged in twice a second.
pub fn wire(app: &AppWindow) {
    let style = crate::preview::Style { dot: 6.0, step: 8, margin: 3, gap: 0 };
    app.global::<Hardware>().set_panel(crate::preview::render_styled(&[crate::preview::dark()], 2, style));
    refresh(app);
    let weak = app.as_weak();
    TIMER.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            if let Some(app) = weak.upgrade() {
                refresh(&app);
            }
        })
    });
}
