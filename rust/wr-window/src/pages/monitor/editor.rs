//! The layout editor page (`led_mon/editor/window.py`): placing apps on each panel's pages, the
//! selected app's settings, the live preview, and saving the layout file for the monitor.
//!
//! The page is `EditorPage` in `ui/pages/monitor.slint`. It shows two `PanelEdit` cards (left and right:
//! the page list, the selected page's preview, its slots, duration and port), the list of apps to add,
//! and a settings form for the selected slot. The form is built here as a list of `Field`s; each field
//! has a matching `Act` in `Editor::acts`, so when the Slint side reports "field 3 was toggled" Rust
//! knows which setting that is.
//!
//! How the Slint callbacks map to Rust, all connected in `wire` and run on the UI thread:
//! `app-chosen`, `place`, `place-selected`, `slot-clicked`, `page-chosen`, `duration-edited`,
//! `add-page`, `remove-page`, `move-page`, `clear-slot` and `port-chosen` edit the in-memory layout;
//! `field-toggled`, `field-chosen`, `field-edited` and `field-button` change the selected app's
//! settings; `brightness-changed` and `power-changed` change monitor settings at once; `identify`,
//! `resume`, `send-preview`, `swap` talk to the monitor; `apply` saves and reloads; `revert` reloads the
//! file; `preset` replaces the pages with a ready-made layout; `start-monitor` launches WhiteRabbit.exe.
//!
//! Threads: editing is all in memory on the UI thread. Every request to the monitor (`ask`, `run_action`
//! and the inline ones) goes to a worker thread through `Ctx::in_background`, since the monitor may be
//! slow or not running and each request can wait seconds. Listing microphones and running programs also
//! runs on workers. The preview is drawn on the UI thread from a timer (5 times a second, only while the
//! page is showing) by `readings::Renderer`.
//!
//! What is saved: the layout file (config-local.yaml, or the file named by CONFIG_FILE) when the user
//! presses Apply, Start monitor or Swap, or agrees on closing (`apply_on_close`). Monitor-wide settings
//! in settings.json ("sensor_source", "sensor_refresh_seconds", and brightness and power settings when
//! the monitor isn't running to take them) are saved as soon as they change. Changes to the layout stay
//! in memory, marked "dirty", until saved.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use wr_core::grid::Grid;

use super::catalog::{self, Kind, Size, Target};
use super::model::{self, EditorLayout, Entry, Page, SIDES};
use super::readings::Renderer;
use crate::app::Ctx;
use crate::{
    commands as cmd, dialogs, link, panels, paths, preview, settings, App, AppRow, AppWindow, EditorPage, Field,
    PanelEdit,
};

/// Seconds between preview redraws (5 a second): enough to see apps move, cheap enough for the UI thread.
const FRAME_SECONDS: f64 = 0.2;
/// How long the monitor shows a preview sent with "Show on panels" before going back to the layout.
const PREVIEW_SECONDS: i64 = 30;
/// The GPU drop-down's first entry (saved as ""): let the app pick.
const FIRST_GPU: &str = "First GPU found";
/// The sensor drop-down's first entry (saved as ""): let the app pick.
const AUTOMATIC: &str = "Automatic";
/// The game drop-down's first entry (saved as ""): follow the full-screen game in front.
const AUTO_GAME: &str = "Automatic (the game in front)";

/// Where temperatures and fans are read from: (value saved in settings.json, label in the drop-down).
const SOURCES: [(&str, &str); 4] = [
    ("auto", "Automatic"),
    ("framework_control", "Framework Control"),
    ("libre_hardware_monitor", "Libre Hardware Monitor"),
    ("framework_tool", "framework_tool"),
];
/// The help under the "Read from" drop-down, one per entry of `SOURCES`.
const SOURCE_HELP: [&str; 4] = [
    "Tries Framework Control, then Libre Hardware Monitor, then framework_tool, and uses the first one that gives readings (temperatures and fans can come from different ones).",
    "Needs Framework Control installed; its service runs in the background, so nothing has to be open and no admin rights are needed. On a Framework 16 it gives the CPU, APU, ambient and charger temperatures and both fans.",
    "Needs Libre Hardware Monitor open (it can sit in the tray) and running as admin; White Rabbit reads its sensors through Windows. Which sensors show depends on the laptop: GPU and SSD temperatures are likely, fans may be missing.",
    "Needs framework_tool.exe next to WhiteRabbit.exe or on PATH, and White Rabbit itself running as admin; otherwise Windows refuses to start it.",
];
/// The "Read every" choices, in seconds.
const REFRESH_CHOICES: [f64; 5] = [1.0, 2.0, 5.0, 10.0, 30.0];
/// The help under the "Read every" drop-down.
const REFRESH_HELP: &str = "How often the temperatures and fans are read. Shorter follows changes faster; framework_tool and Libre Hardware Monitor use a little more CPU when read often.";

/// What a line of the settings form changes.
/// One per field in the form, in the same order (`Editor::acts`).
#[derive(Clone, Debug)]
enum Act {
    /// A heading or status line: nothing to do.
    None,
    /// A bool, text, number or list setting (by its index in the app's settings)
    Plain(usize),
    /// A pick from these values
    Choice(usize, Vec<Value>),
    /// One option of a several-of setting
    Multi(usize, Value),
    /// The "Read from" drop-down (saved in settings.json for every app).
    SensorSource,
    /// The "Read every" drop-down (saved in settings.json).
    SensorRefresh,
    /// The "Refresh list" button under the sensor drop-down.
    RefreshSensors,
    /// The "Refresh list" button under the game drop-down.
    RefreshApps,
    /// The button that opens Windows' location privacy settings.
    LocationSettings,
}

/// Everything the editor keeps, on the UI thread only (see `editor()`).
struct Editor {
    /// The layout being edited.
    layout: EditorLayout,
    /// The file the layout was read from; None when the bundled layout was used. Revert reads it again.
    load_path: Option<PathBuf>,
    /// Where Apply writes.
    save_path: PathBuf,
    /// (side, page, "top" | "bottom" | "full")
    selection: Option<(usize, usize, String)>,
    /// The page shown on each panel card.
    selected_page: [usize; 2],
    /// There are changes not yet saved (also shown as `App.layout-dirty`, so other parts of the window know).
    dirty: bool,
    /// Draws the preview pictures.
    renderer: Renderer,
    /// The latest preview picture of each panel's selected page; None until the first draw.
    frames: [Option<Grid>; 2],
    /// Each panel's Port choices as (label, port, USB location), from `model::port_choices`.
    ports: [Vec<(String, Option<String>, Option<String>)>; 2],
    /// What each field of the settings form does, by index.
    acts: Vec<Act>,
    /// The monitor's last status reply
    monitor: Option<Value>,
    /// The monitor's sensor_source reply
    sensors: Option<Value>,
    /// A message to show instead of the sensor status (e.g. "Saved. Checking..."); empty for the status.
    sensor_note: String,
    /// The microphones found, for the audio visualizer's input drop-down.
    microphones: Vec<String>,
    /// Running programs as (exe name, window title), for the FPS apps' game drop-down.
    running_apps: Vec<(String, String)>,
    /// A brightness change waiting to be sent: (follow the screen, level 0-100).
    brightness_pending: Option<(bool, f64)>,
}

// Set once by `wire`. A thread-local so the reply callbacks (which must be `Send`) can find the editor.
thread_local! {
    static EDITOR: RefCell<Option<Rc<RefCell<Editor>>>> = const { RefCell::new(None) };
}

/// The editor state. Panics if called before `wire`, which nothing does. Each caller borrows it for the
/// length of one callback; a second borrow while one is held would panic, so dialogs (which keep timers
/// running) should not be opened while a borrow is held.
fn editor() -> Rc<RefCell<Editor>> {
    EDITOR.with(|e| e.borrow().clone()).expect("layout editor wired")
}

/// Reads the layout from `path`, or the bundled default layout when there is no file yet. A file that
/// can't be read or parsed gives an empty layout and a warning (logged too); the file itself is left as
/// it is until the user saves.
fn read_layout(path: Option<&PathBuf>) -> (EditorLayout, Vec<String>) {
    let loaded = match path {
        Some(p) => model::load_file(p).map_err(|e| (p.display().to_string(), e)),
        None => serde_yaml::from_str::<Value>(paths::BUNDLED_LAYOUT).map_err(|e| ("config.yaml".into(), e.to_string())),
    };
    match loaded {
        Ok(config) => model::from_config(&config),
        Err((file, e)) => {
            wr_core::error!("Could not read {file}: {e}");
            (EditorLayout::default(), vec![format!("Could not read {file} ({e}). Starting with an empty layout.")])
        }
    }
}

/// The CONFIG_FILE environment variable (for testing) or config-local.yaml in the settings folder.
fn save_path() -> PathBuf {
    std::env::var_os("CONFIG_FILE").map(PathBuf::from).unwrap_or_else(|| paths::config_dir().join("config-local.yaml"))
}

// Pictures for the Picture app ------------------------------------------------------------------------

/// The built-in pictures the Picture app can show on each side; these files ship with the monitor.
const BUILT_IN: [(&str, &[&str]); 2] = [
    ("left", &["double-check.json", "the-way-left.json", "zigzag.json"]),
    ("right", &["check.json", "every-third-row.json", "the-way-right.json"]),
];

/// The user's drawings (`<settings folder>\drawings\*.json`), by name, sorted ignoring case. A missing
/// folder gives an empty list.
fn drawing_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(paths::drawings_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(String::from))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// (value, label) for the Picture app: your drawings first, then the built-in patterns.
fn snapshot_options(side: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        drawing_names().into_iter().map(|n| (format!("drawing:{n}"), format!("My drawing: {n}"))).collect();
    if let Some((_, files)) = BUILT_IN.iter().find(|(s, _)| *s == side) {
        out.extend(files.iter().map(|f| (f.to_string(), format!("Built-in: {}", f.trim_end_matches(".json")))));
    }
    out
}

/// The Picture app's default for a new entry: the first drawing, or the first built-in picture.
fn first_snapshot(side: &str) -> Option<String> {
    snapshot_options(side).into_iter().next().map(|(v, _)| v)
}

// Showing the editor ----------------------------------------------------------------------------------

/// Everything one panel card shows, as the Slint `PanelEdit` struct. Also clamps the selected page to
/// the list (after a page was removed). It lists the serial ports (through `port_index`), which is a
/// quick Windows call.
fn panel_edit(e: &mut Editor, s: usize) -> PanelEdit {
    let side = SIDES[s];
    let pages = &e.layout.pages[s];
    // A panel always has at least one page (see `model::from_config`), so this stays a valid index.
    e.selected_page[s] = e.selected_page[s].min(pages.len().saturating_sub(1));
    let page = pages.get(e.selected_page[s]);
    let lines: Vec<SharedString> =
        pages.iter().enumerate().map(|(i, p)| format!("{}. {}", i + 1, p.describe()).into()).collect();
    // The slot to highlight: only if the selection is on this panel's page being shown.
    let selected = match &e.selection {
        Some((ss, pi, slot)) if *ss == s && *pi == e.selected_page[s] => slot.clone(),
        _ => String::new(),
    };
    // 160 is the brightest an app draws (see `readings::FOREGROUND`), shown as full white.
    let levels = e.frames[s].as_ref().map(|g| preview::levels_of(g, 160.0)).unwrap_or_else(preview::dark);
    let labels: Vec<SharedString> = e.ports[s].iter().map(|(l, _, _)| l.as_str().into()).collect();
    let current = port_index(e, s);
    PanelEdit {
        // "Left panel" / "Right panel".
        title: format!("{}{} panel", side[..1].to_uppercase(), &side[1..]).into(),
        ports: ModelRc::new(VecModel::from(labels)),
        port: current as i32,
        pages: ModelRc::new(VecModel::from(lines)),
        page: e.selected_page[s] as i32,
        duration: page.map(|p| p.duration.to_string()).unwrap_or_default().into(),
        // Drawn at twice the size, in the large LED style used on this page.
        image: preview::render_styled(&[levels], 2, preview::LARGE),
        full: page.is_some_and(|p| p.full.is_some()),
        top_empty: page.is_none_or(|p| p.top.is_none()),
        bottom_empty: page.is_none_or(|p| p.bottom.is_none()),
        selected: selected.into(),
    }
}

/// Which Port choice matches what the layout file says for this panel. A port written as a number in
/// the file is read as text here.
fn port_index(e: &Editor, s: usize) -> usize {
    let side = SIDES[s];
    let get = |k: String| {
        e.layout.settings.get(&k).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let ports = connected_ports();
    let (_, current) =
        model::port_choices(&ports, get(format!("{side}-port")).as_deref(), get(format!("{side}-location")).as_deref());
    current
}

/// The LED Matrix panels plugged in now, as (port, USB location without its interface suffix).
fn connected_ports() -> Vec<(String, Option<String>)> {
    panels::list_ports()
        .into_iter()
        // Only ports whose USB ids say they are an LED Matrix module.
        .filter(wr_panel::ports::is_led_matrix_port)
        .map(|p| (p.device, p.location.map(|l| l.split(':').next().unwrap_or("").to_string())))
        .collect()
}

/// Rebuilds both panels' Port choices from what is plugged in now and what the layout says.
fn fill_ports(e: &mut Editor) {
    let ports = connected_ports();
    for (s, side) in SIDES.iter().enumerate() {
        let get = |k: String| e.layout.settings.get(&k).and_then(Value::as_str).map(String::from);
        let (choices, _) = model::port_choices(
            &ports,
            get(format!("{side}-port")).as_deref(),
            get(format!("{side}-location")).as_deref(),
        );
        e.ports[s] = choices;
    }
}

/// Redraws both panel cards and the "unsaved changes" flag.
fn show_panels(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    page.set_left(panel_edit(e, 0));
    page.set_right(panel_edit(e, 1));
    app.global::<App>().set_layout_dirty(e.dirty);
}

/// Only the preview pictures changed.
/// Only the preview pictures changed: updates the image in each card and leaves the rest (so a text box
/// being typed in isn't reset).
fn show_frames(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    for s in 0..2 {
        let levels = e.frames[s].as_ref().map(|g| preview::levels_of(g, 160.0)).unwrap_or_else(preview::dark);
        let image = preview::render_styled(&[levels], 2, preview::LARGE);
        // A Slint struct property is replaced as a whole: copy it, change the image, set it back.
        let mut p = if s == 0 { page.get_left() } else { page.get_right() };
        p.image = image;
        if s == 0 {
            page.set_left(p);
        } else {
            page.set_right(p);
        }
    }
}

/// A blank form field of a kind the Slint side knows: "bool", "text", "choice", "button", "heading" or
/// "status".
fn field(kind: &str, label: impl Into<SharedString>) -> Field {
    Field { kind: kind.into(), label: label.into(), ..Field::default() }
}

/// A drop-down field; `index` is the selected option (out of range shows nothing selected).
fn choice_field(label: &str, labels: Vec<String>, index: usize, help: &str) -> Field {
    let options: Vec<SharedString> = labels.into_iter().map(SharedString::from).collect();
    Field {
        kind: "choice".into(),
        label: label.into(),
        options: ModelRc::new(VecModel::from(options)),
        index: index as i32,
        help: help.into(),
        ..Field::default()
    }
}

/// The setting's current value in the entry, or its default when the entry doesn't have it.
fn get_value(entry: &Entry, setting: &catalog::Setting) -> Value {
    let found = match setting.target {
        Target::App => entry.get(setting.key).cloned(),
        _ => entry.get("args").and_then(|a| a.get(setting.key)).cloned(),
    };
    found.unwrap_or_else(|| setting.default_value())
}

/// A value as text for a text box: lists joined with ", ", strings without quotes, null as empty.
fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// The sensor_source status as one or two lines (`sensor_sources.describe`).
/// `status` is the monitor's sensor_source reply: whether anything is being read, how many temperatures
/// and fans came from which source, and each source's error.
fn describe_sensors(status: Option<&Value>) -> String {
    let Some(status) = status.filter(|s| s.as_object().is_some_and(|m| !m.is_empty())) else {
        return "The monitor isn't running.".into();
    };
    // The monitor only reads sensors while an app that needs them is on a panel.
    if !status.get("reading").and_then(Value::as_bool).unwrap_or(false) {
        return "No panel is showing Temperatures or Fans right now, so nothing is being read.".into();
    }
    let label =
        |k: &str| SOURCES.iter().find(|(key, _)| *key == k).map(|(_, l)| l.to_string()).unwrap_or(k.to_string());
    let mut parts = Vec::new();
    for (what, count, from) in [("Temperatures", "temps", "temps_from"), ("Fans", "fans", "fans_from")] {
        match status.get(from).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(source) => parts.push(format!(
                "{what}: {} from {}",
                status.get(count).and_then(Value::as_i64).unwrap_or(0),
                label(source)
            )),
            None => parts.push(format!("{what}: none")),
        }
    }
    let mut text = format!("Now reading. {}.", parts.join("; "));
    if let Some(errors) = status.get("errors").and_then(Value::as_object).filter(|e| !e.is_empty()) {
        for (source, error) in errors {
            text += &format!("\n{}: {}", label(source), value_text(error));
        }
    }
    text
}

/// Builds the settings form for the selected slot: a field (and an `Act`) per setting the app offers,
/// filled with the entry's values. Choices whose saved value isn't in the list (a GPU or microphone not
/// plugged in, a game not running) get an extra "(not found)" entry so the saved choice isn't lost.
fn show_settings(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    let mut fields: Vec<Field> = Vec::new();
    let mut acts: Vec<Act> = Vec::new();
    // Fields and acts are always added together, so their indexes match.
    let mut push = |f: Field, a: Act| {
        fields.push(f);
        acts.push(a);
    };
    // The selected slot's entry (None for an empty slot), with where it is.
    let entry = e
        .selection
        .as_ref()
        .and_then(|(s, i, slot)| e.layout.pages[*s].get(*i).map(|p| (p.slot(slot).cloned(), *s, *i, slot.clone())));
    match &entry {
        None => {
            // Nothing selected yet (just opened, or after a preset): explain how to start.
            page.set_settings_title("No app selected".into());
            page.set_settings_help(
                "Drag an app from the list on the left onto a panel, or select a slot and double-click an app.".into(),
            );
        }
        // An empty slot is selected: say where it is and how to fill it.
        Some((None, s, i, slot)) => {
            page.set_settings_title("Empty slot".into());
            page.set_settings_help(
                format!("{}. Double-click an app on the left, or drag it here.", where_text(*s, *i, slot)).into(),
            );
        }
        Some((Some(entry), s, i, slot)) => {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
            let info = catalog::get(name);
            let side = SIDES[*s];
            // An app from a plugin has no catalog entry; its own name is the best title.
            page.set_settings_title(if info.key == "plugin" { name.into() } else { info.label.into() });
            page.set_settings_help(format!("{}\n{}", info.help, where_text(*s, *i, slot)).into());
            let shown: Vec<(usize, &catalog::Setting)> = info
                .settings
                .iter()
                .enumerate()
                .filter(|(_, st)| {
                    // The sensor and location sources only work on Windows; away from it they are
                    // shown only with fake panels (for screenshots).
                    cfg!(windows)
                        || !matches!(st.kind, Kind::SensorSource | Kind::LocationSource)
                        || std::env::var_os("WHITE_RABBIT_FAKE_PANELS").is_some()
                })
                .collect();
            // A single status line so the form isn't just blank.
            if shown.is_empty() {
                let mut f = field("status", "");
                f.text = "This app has no settings.".into();
                push(f, Act::None);
            }
            for (n, st) in shown {
                // One or more fields per setting, by kind. `n` (the setting's index in the catalog)
                // goes into the `Act`, so a change later finds the same setting.
                let value = get_value(entry, st);
                match st.kind {
                    // A switch; the stored value may be any JSON, read with Python's truthiness.
                    Kind::Bool => {
                        let mut f = field("bool", st.label);
                        f.checked = model::is_true(Some(&value));
                        f.help = st.help.into();
                        push(f, Act::Plain(n));
                    }
                    // A text box; numbers and lists are shown as text and parsed back in
                    // `field-edited`.
                    Kind::Text | Kind::Int | Kind::List => {
                        let mut f = field("text", st.label);
                        f.text = value_text(&value).into();
                        f.help = st.help.into();
                        push(f, Act::Plain(n));
                    }
                    Kind::Choice | Kind::Snapshot | Kind::Gpu | Kind::Microphone | Kind::LocationSource => {
                        // The drop-down's (saved value, label) pairs. Pictures, GPUs and microphones come
                        // from this computer or the monitor; other choices from the catalog.
                        let pairs: Vec<(Value, String)> = match st.kind {
                            Kind::Snapshot => {
                                snapshot_options(side).into_iter().map(|(v, l)| (Value::String(v), l)).collect()
                            }
                            Kind::Gpu => {
                                let names: Vec<String> = e
                                    .monitor
                                    .as_ref()
                                    .and_then(|m| m.get("gpus"))
                                    .and_then(Value::as_array)
                                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                                    .unwrap_or_default();
                                std::iter::once((json!(""), FIRST_GPU.to_string()))
                                    .chain(names.into_iter().map(|n| (json!(n), n)))
                                    .collect()
                            }
                            Kind::Microphone => {
                                std::iter::once((json!(""), wr_music::capture::DEFAULT_MICROPHONE.to_string()))
                                    .chain(e.microphones.iter().map(|n| (json!(n), n.clone())))
                                    .collect()
                            }
                            _ => st
                                .option_values()
                                .into_iter()
                                .enumerate()
                                .map(|(i, v)| (v, st.option_label(i)))
                                .collect(),
                        };
                        // Find the saved value in the list. Empty means "automatic / not chosen".
                        let (mut values, mut labels): (Vec<Value>, Vec<String>) = pairs.into_iter().unzip();
                        let empty = value.is_null() || value == json!("");
                        let mut index = values.iter().position(|v| *v == value);
                        if index.is_none() && matches!(st.kind, Kind::Gpu | Kind::Microphone) {
                            if empty {
                                index = Some(0);
                            } else {
                                // Picked on this computer before, but not found now
                                values.push(value.clone());
                                labels.push(format!("{} (not found)", value_text(&value)));
                                index = Some(values.len() - 1);
                            }
                        }
                        // Any other value not in the list (a hand-edited file) is kept as an entry too.
                        if index.is_none() && !empty {
                            values.push(value.clone());
                            labels.push(value_text(&value));
                            index = Some(values.len() - 1);
                        }
                        let mut help = st.help.to_string();
                        if st.kind == Kind::LocationSource {
                            help = if value == json!("windows") {
                                "Follows the laptop as it moves, using Windows' own location (Wi-Fi, GPS). If Windows can't give a location, the zip code or coordinates below are used.".into()
                            } else {
                                "The zip code or coordinates below. If both are blank, your location is worked out from your internet address.".into()
                            };
                        }
                        if st.kind == Kind::Microphone && e.microphones.is_empty() {
                            help = "No microphones were found.".into();
                        }
                        // With one real GPU (or none) the choice is pointless; say why.
                        if st.kind == Kind::Gpu && values.len() <= 2 {
                            help = if e.monitor.is_none() {
                                "The list of GPUs comes from the monitor; start it to see them.".into()
                            } else if values.len() == 2 {
                                "Only one GPU was found.".into()
                            } else {
                                "No GPU usage could be read on this computer.".into()
                            };
                        }
                        push(
                            // usize::MAX: an empty value with no matching option shows nothing selected.
                            choice_field(st.label, labels, index.unwrap_or(usize::MAX), &help),
                            Act::Choice(n, values),
                        );
                        if st.kind == Kind::LocationSource && value == json!("windows") {
                            push(field("button", "Windows location settings"), Act::LocationSettings);
                        }
                    }
                    // A temperature sensor by name, from the monitor's last sensor reply, each with
                    // its reading now so the user can tell them apart.
                    Kind::Sensor => {
                        // The monitor's sensor reply lists ["name", degrees C] pairs it is reading now.
                        let named: Vec<(String, f64)> = e
                            .sensors
                            .as_ref()
                            .and_then(|s| s.get("named"))
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_f64()?)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut values = vec![json!("")];
                        let mut labels = vec![AUTOMATIC.to_string()];
                        for (name, c) in &named {
                            values.push(json!(name));
                            labels.push(format!("{name} ({c:.0} °C now)"));
                        }
                        // A sensor named in the file but not read now is kept as an extra entry.
                        let current = value_text(&value);
                        if !current.is_empty() && !named.iter().any(|(n, _)| *n == current) {
                            values.push(json!(current));
                            labels.push(format!("{current} (no reading now)"));
                        }
                        let index = if current.is_empty() {
                            0
                        } else {
                            values.iter().position(|v| *v == json!(current)).unwrap_or(0)
                        };
                        let mut help = st.help.to_string();
                        if st.key == "sensor_1" && named.is_empty() {
                            help = "No sensors read yet. Check where temperatures are read from below, then press Refresh list.".into();
                        }
                        push(choice_field(st.label, labels, index, &help), Act::Choice(n, values));
                        // The refresh button goes once, under the first sensor drop-down.
                        if st.key == "sensor_1" {
                            push(field("button", "Refresh list"), Act::RefreshSensors);
                        }
                    }
                    // The FPS apps' game: "automatic" or one of the programs running now (by exe
                    // name, compared ignoring case); a saved game that isn't running is kept.
                    Kind::Process => {
                        let mut values = vec![json!("")];
                        let mut labels = vec![AUTO_GAME.to_string()];
                        for (exe, title) in &e.running_apps {
                            values.push(json!(exe));
                            labels.push(title.clone());
                        }
                        let current = value_text(&value);
                        let found =
                            values.iter().position(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(&current)));
                        let index = if current.is_empty() {
                            0
                        } else if let Some(i) = found {
                            i
                        } else {
                            values.push(json!(current));
                            labels.push(format!("{current} (not open now)"));
                            values.len() - 1
                        };
                        push(choice_field(st.label, labels, index, st.help), Act::Choice(n, values));
                        push(field("button", "Refresh list"), Act::RefreshApps);
                    }
                    // Several-of: a heading, then one switch per option.
                    Kind::Multi => {
                        push(field("heading", st.label), Act::None);
                        let current: Vec<Value> = value.as_array().cloned().unwrap_or_default();
                        for (i, option) in st.option_values().into_iter().enumerate() {
                            let mut f = field("bool", st.option_label(i).replace('_', " "));
                            f.checked = current.contains(&option);
                            push(f, Act::Multi(n, option));
                        }
                    }
                    // Not part of the app entry: these lines change settings.json for every app that
                    // reads temperatures or fans, which is why they sit under their own heading.
                    Kind::SensorSource => {
                        // These two are monitor-wide settings, read from settings.json rather than the entry.
                        let saved = settings::load();
                        let choice = saved.get("sensor_source").and_then(Value::as_str).unwrap_or("auto");
                        let ci = SOURCES.iter().position(|(k, _)| *k == choice).unwrap_or(0);
                        push(field("heading", "Temperatures and fans (all panels)"), Act::None);
                        push(
                            choice_field(
                                "Read from",
                                SOURCES.iter().map(|(_, l)| l.to_string()).collect(),
                                ci,
                                SOURCE_HELP[ci],
                            ),
                            Act::SensorSource,
                        );
                        let refresh = saved.get("sensor_refresh_seconds").and_then(Value::as_f64).unwrap_or(2.0);
                        let mut labels: Vec<String> = REFRESH_CHOICES
                            .iter()
                            .map(|n| format!("{n} second{}", if *n == 1.0 { "" } else { "s" }))
                            .collect();
                        // A hand-edited value that isn't one of the choices is added as its own entry.
                        let ri = REFRESH_CHOICES.iter().position(|n| *n == refresh).unwrap_or_else(|| {
                            labels.push(format!("{refresh} seconds"));
                            labels.len() - 1
                        });
                        push(choice_field("Read every", labels, ri, REFRESH_HELP), Act::SensorRefresh);
                        let mut f = field("status", "");
                        f.text = if e.sensor_note.is_empty() {
                            describe_sensors(e.sensors.as_ref())
                        } else {
                            e.sensor_note.clone()
                        }
                        .into();
                        push(f, Act::None);
                    }
                }
            }
        }
    }
    // Keep the acts for the callbacks, then hand the fields to the page.
    e.acts = acts;
    page.set_fields(ModelRc::new(VecModel::from(fields)));
}

/// "Left panel, page 2, top half", for the form's help line.
fn where_text(s: usize, i: usize, slot: &str) -> String {
    let side = SIDES[s];
    let place = match slot {
        "full" => "whole panel",
        "top" => "top half",
        _ => "bottom half",
    };
    format!("{}{} panel, page {}, {place}", side[..1].to_uppercase(), &side[1..], i + 1)
}

/// Sets the status line at the bottom of the editor.
fn status(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<EditorPage>().set_status(text.into());
}

// Changing the layout ---------------------------------------------------------------------------------

/// Selects a slot (`slot` is "full", "top" or "bottom") of a page, redraws, and fetches the lists the
/// app's form needs (sensors from the monitor, running programs) in the background; the form is redrawn
/// again when they arrive.
fn select(app: &AppWindow, e: &mut Editor, s: usize, index: usize, slot: &str) {
    e.selection = Some((s, index, slot.to_string()));
    e.selected_page[s] = index;
    show_panels(app, e);
    show_settings(app, e);
    // The Temperatures app's sensor list comes from the monitor
    // Only the apps whose form needs them trigger these background lookups.
    let wants_sensors = e.layout.pages[s]
        .get(index)
        .and_then(|p| p.slot(slot))
        .and_then(|en| en.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|n| {
            catalog::get(n).settings.iter().any(|st| matches!(st.kind, Kind::Sensor | Kind::SensorSource))
        });
    if wants_sensors {
        ask_sensors(false);
    }
    let wants_apps = e.layout.pages[s]
        .get(index)
        .and_then(|p| p.slot(slot))
        .and_then(|en| en.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|n| catalog::get(n).settings.iter().any(|st| st.kind == Kind::Process));
    if wants_apps {
        list_running_apps();
    }
}

/// Marks the layout changed and redraws the cards (and the form when `refresh_settings`).
fn changed(app: &AppWindow, e: &mut Editor, refresh_settings: bool) {
    e.dirty = true;
    show_panels(app, e);
    if refresh_settings {
        show_settings(app, e);
    }
}

/// Puts a new copy of app `key`, with default settings, on the selected page of panel `s`. A whole-panel
/// app replaces both halves; a half app dropped on "full" goes in the top half and removes a whole-panel
/// app. The new app is selected.
fn place(app: &AppWindow, e: &mut Editor, s: usize, key: &str, slot: &str) {
    let side = SIDES[s];
    let index = e.selected_page[s];
    // `index` is the page shown on that panel's card, which is where the app was dropped.
    let Some(page) = e.layout.pages[s].get_mut(index) else { return };
    let entry = catalog::new_app(key, side, first_snapshot(side));
    // Note the page keeps its duration; only the slots change.
    let slot = if catalog::get(key).size == Size::Full {
        page.full = Some(entry);
        page.top = None;
        page.bottom = None;
        "full"
    } else {
        page.full = None;
        let slot = if slot == "full" { "top" } else { slot };
        *page.slot_mut(slot) = Some(entry);
        slot
    };
    select(app, e, s, index, slot);
    changed(app, e, true);
}

/// Changes one setting of the selected app.
/// `n` is the setting's index in the app's catalog entry. Empty values of kinds where "missing" means
/// "automatic" remove the key instead, so the file stays tidy. `refresh` redraws the form (needed when a
/// choice shows or hides other fields).
fn set_setting(app: &AppWindow, e: &mut Editor, n: usize, value: Value, refresh: bool) {
    let Some((s, i, slot)) = e.selection.clone() else { return };
    let Some(entry) = e.layout.pages[s].get_mut(i).and_then(|p| p.slot_mut(&slot).as_mut()) else { return };
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(st) = catalog::get(&name).settings.get(n) else { return };
    // Most settings live under "args"; a few (`Target::App`) on the entry itself. A broken "args" that
    // isn't an object is replaced.
    let target: &mut Map<String, Value> = match st.target {
        Target::App => entry,
        _ => {
            let args = entry.entry("args").or_insert_with(|| json!({}));
            if !args.is_object() {
                *args = json!({});
            }
            args.as_object_mut().unwrap()
        }
    };
    // The same rule as `catalog::new_app` uses for new entries.
    let empty = value.is_null() || value == json!("") || value == json!([]);
    if empty && matches!(st.kind, Kind::Text | Kind::Process | Kind::List | Kind::Gpu | Kind::Sensor | Kind::Microphone)
    {
        target.remove(st.key);
    } else {
        target.insert(st.key.into(), value);
    }
    changed(app, e, refresh);
}

/// An item of a comma-separated list: a decimal number when it has a point and parses, text otherwise.
/// Whole numbers stay text, as the Python editor kept them.
fn number_or_text(text: &str) -> Value {
    if text.contains('.') {
        if let Ok(f) = text.parse::<f64>() {
            return json!(f);
        }
    }
    json!(text)
}

// Talking to the monitor ------------------------------------------------------------------------------

/// Sends one request to the monitor on a worker thread; `done` gets the reply (None: no monitor or no
/// answer within `timeout` seconds) on the UI thread.
fn ask(message: Value, timeout: f64, done: impl FnOnce(&AppWindow, Option<Value>) + Send + 'static) {
    crate::app::ctx().in_background(move || link::request(message, Duration::from_secs_f64(timeout)), done);
}

/// The reply only if it says "ok": true.
fn ok_reply(reply: Option<Value>) -> Option<Value> {
    reply.filter(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false))
}

/// Asks the monitor which sensors it reads (up to 8 seconds: with `reload` it re-reads its sources first,
/// which can take a while), then redraws the form. With `reload` (after a source change) the note says
/// whether the change was taken up.
fn ask_sensors(reload: bool) {
    ask(json!({"cmd": "sensor_source", "reload": reload}), 8.0, move |app, reply| {
        let ed = editor();
        let mut e = ed.borrow_mut();
        // Runs on the UI thread when the reply (or the timeout) comes.
        e.sensors = ok_reply(reply).and_then(|r| r.get("sensor_source").cloned());
        e.sensor_note = if reload {
            match &e.sensors {
                None => "Saved. The monitor isn't running; it will use this when it starts.".into(),
                Some(s) if !s.get("reading").and_then(Value::as_bool).unwrap_or(false) => {
                    format!("Saved. {}", describe_sensors(Some(s)))
                }
                Some(s) => describe_sensors(Some(s)),
            }
        } else {
            String::new()
        };
        show_settings(app, &mut e);
    });
}

/// Lists the programs with windows on a worker thread (`winapps.rs`), then redraws the form.
fn list_running_apps() {
    crate::app::ctx().in_background(crate::winapps::running_apps, |app, apps| {
        let ed = editor();
        let mut e = ed.borrow_mut();
        e.running_apps = apps;
        show_settings(app, &mut e);
    });
}

/// Asks the monitor for its status (1.5 s timeout, it should answer at once) and shows "Monitor is
/// running (ports)" or "isn't running". The line is only replaced when it held a monitor message or the
/// running state changed, so it doesn't overwrite another message. Also shows the monitor's brightness,
/// unless the user is moving the slider right now.
fn check_monitor() {
    ask(json!({"cmd": "status"}), 1.5, |app, reply| {
        let ed = editor();
        let mut e = ed.borrow_mut();
        let status = ok_reply(reply);
        // Kept for the GPU list and the status line; None while the monitor isn't running.
        let was_running = e.monitor.is_some();
        e.monitor = status.clone();
        // What the status line says now, to decide below whether this check may replace it.
        let current = app.global::<EditorPage>().get_status().to_string();
        let text = match &status {
            Some(s) => {
                let ports: Vec<String> = s
                    .get("ports")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default();
                format!(
                    "Monitor is running ({}).",
                    if ports.is_empty() { "no panels".into() } else { ports.join(", ") }
                )
            }
            None => "The monitor isn't running. Apply saves the layout; Start monitor shows it on the panels.".into(),
        };
        if current.is_empty()
            || was_running != status.is_some()
            || current.starts_with("Monitor is")
            || current.starts_with("The monitor isn't")
        {
            status_text(app, text);
        }
        if let Some(b) = status.as_ref().and_then(|s| s.get("brightness")) {
            if e.brightness_pending.is_none() {
                show_brightness(app, b);
            }
        }
    });
}

/// Sets the status line (the same as `status`, for a `String`).
fn status_text(app: &AppWindow, text: String) {
    status(app, text);
}

/// The monitor's brightness settings: follow the screen's brightness, or a fixed level (0-100).
fn show_brightness(app: &AppWindow, b: &Value) {
    let page = app.global::<EditorPage>();
    page.set_follow_screen(b.get("follow_screen").and_then(Value::as_bool).unwrap_or(true));
    page.set_brightness(b.get("level").and_then(Value::as_f64).unwrap_or(70.0) as f32);
}

/// Runs a step on a worker thread and shows what it says in the status line.
/// `busy` is shown at once (if not empty); the action's result replaces it when done.
fn run_action(app: &AppWindow, busy: &str, action: impl FnOnce() -> String + Send + 'static) {
    if !busy.is_empty() {
        status(app, busy);
    }
    crate::app::ctx().in_background(action, status);
}

/// Writes the layout file. On failure shows a dialog: the layout's problems if it doesn't pass
/// `validate`, otherwise the file error. Returns true when saved.
fn save(app: &AppWindow, e: &mut Editor) -> bool {
    if let Err(problems) = model::save(&e.layout, &e.save_path) {
        let text = if model::validate(&e.layout).is_empty() {
            format!("Could not write {}:\n{problems}", e.save_path.display())
        } else {
            problems
        };
        dialogs::info("Can't save the layout", &text);
        return false;
    }
    // From now on Revert reads the file just written.
    e.load_path = Some(e.save_path.clone());
    e.dirty = false;
    app.global::<App>().set_layout_dirty(false);
    true
}

/// Tells the monitor to read the layout file again. False when no monitor answered within 3 seconds.
fn reload_monitor() -> bool {
    link::request(json!({"cmd": "reload"}), Duration::from_secs(3)).is_some()
}

/// Asked when the window closes: apply the changes (true) or drop them.
pub fn apply_on_close() {
    let ed = editor();
    // Copied out first, so no borrow is held while the question is open (timers still run then)
    // (The save and reload here run on the UI thread: the window is closing, so there is nothing to
    // keep responsive, and the monitor must have the file before the window goes.)
    let (dirty, layout, path) = {
        let e = ed.borrow();
        (e.dirty, e.layout.clone(), e.save_path.clone())
    };
    if dirty
        && dialogs::confirm("System monitor", "Apply your layout changes before closing?")
        && model::save(&layout, &path).is_ok()
    {
        reload_monitor();
    }
}

/// Starts WhiteRabbit.exe (next to this exe) with --background, so it starts without opening this window
/// again. It runs as its own process; nothing waits for it.
fn start_monitor() -> Result<(), String> {
    let exe = paths::app_dir().join(if cfg!(windows) { "WhiteRabbit.exe" } else { "WhiteRabbit" });
    std::process::Command::new(&exe)
        .arg("--background")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", exe.display()))
}

// Wiring ----------------------------------------------------------------------------------------------

/// Reads the layout, sets up the editor state, fills the page, asks the monitor for its power settings
/// and lists the microphones in the background, connects every callback (see the file header) and starts
/// the preview and status timers. Runs once at start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let load_path = paths::layout_file();
    let (layout, warnings) = read_layout(load_path.as_ref());
    let e = Rc::new(RefCell::new(Editor {
        layout,
        load_path,
        save_path: save_path(),
        selection: None,
        selected_page: [0, 0],
        dirty: false,
        renderer: Renderer::new(),
        frames: [None, None],
        ports: [Vec::new(), Vec::new()],
        acts: Vec::new(),
        monitor: None,
        sensors: None,
        sensor_note: String::new(),
        microphones: Vec::new(),
        running_apps: Vec::new(),
        brightness_pending: None,
    }));
    // From here on `editor()` works, so callbacks and replies can find the state.
    EDITOR.with(|x| *x.borrow_mut() = Some(e.clone()));
    let page = app.global::<EditorPage>();
    // The "add an app" list.
    let rows: Vec<AppRow> = catalog::offered()
        .map(|a| AppRow {
            key: a.key.into(),
            label: a.label.into(),
            size: if a.size == Size::Full { "Whole panel" } else { "Half panel" }.into(),
        })
        .collect();
    page.set_apps(ModelRc::new(VecModel::from(rows)));
    // The power settings use Windows features; elsewhere they show only for screenshots and tests.
    page.set_show_power(
        cfg!(windows)
            || std::env::var_os("WHITE_RABBIT_SHOW_POWER_SETTINGS").is_some()
            || std::env::var_os("WHITE_RABBIT_FAKE_PANELS").is_some(),
    );
    {
        let mut ed = e.borrow_mut();
        fill_ports(&mut ed);
        show_panels(app, &mut ed);
        show_settings(app, &mut ed);
    }
    // Repairs `from_config` had to make (lists paired up, durations evened out) are shown once.
    if !warnings.is_empty() {
        status(app, warnings.join(" "));
    }
    // Brightness and turning off, as saved (the running monitor's when it answers)
    // Show the saved values at once; the monitor's live values replace them when it answers.
    let saved = settings::load();
    page.set_follow_screen(saved.get("follow_screen_brightness").and_then(Value::as_bool).unwrap_or(true));
    page.set_brightness(saved.get("brightness_level").and_then(Value::as_f64).unwrap_or(70.0) as f32);
    page.set_off_with_lid(saved.get("turn_off_with_lid").and_then(Value::as_bool).unwrap_or(true));
    page.set_off_when_unused(saved.get("turn_off_when_screen_unused").and_then(Value::as_bool).unwrap_or(true));
    // The running monitor's own values win over the saved ones, if it answers.
    ask(json!({"cmd": "power"}), 1.5, |app, reply| {
        if let Some(r) = ok_reply(reply) {
            let page = app.global::<EditorPage>();
            if let Some(v) = r.get("turn_off_with_lid").and_then(Value::as_bool) {
                page.set_off_with_lid(v);
            }
            if let Some(v) = r.get("turn_off_when_screen_unused").and_then(Value::as_bool) {
                page.set_off_when_unused(v);
            }
        }
    });
    // Microphones for the audio visualizer's input drop-down; listing them can take a moment.
    ctx.in_background(wr_music::capture::list_microphones, |_, names| editor().borrow_mut().microphones = names);

    // `app-chosen(i)`: an app in the list was clicked; show what it is and where it fits.
    page.on_app_chosen(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        if let Some(info) = catalog::offered().nth(i as usize) {
            let size = if info.size == Size::Full { "whole panel" } else { "top or bottom half" };
            app.global::<EditorPage>().set_app_help(format!("{}\nGoes in: {size}.", info.help).into());
        }
    });
    // `place(i, s, slot)`: app `i` of the list was dropped on panel `s`'s slot.
    page.on_place(|i, s, slot| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        // (`i` counts in the "add an app" list, which is `catalog::offered()` in order.)
        let Some(info) = catalog::offered().nth(i as usize) else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        place(&app, &mut e, s as usize, info.key, &slot);
    });
    // `place-selected(i)`: app `i` was double-clicked: it goes in the selected slot (or the left panel's
    // top half when nothing is selected).
    page.on_place_selected(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some(info) = catalog::offered().nth(i as usize) else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let (s, slot) = match &e.selection {
            // The selected slot's panel and slot; the page is the one shown on that panel.
            Some((s, _, slot)) => (*s, slot.clone()),
            None => (0, "top".to_string()),
        };
        place(&app, &mut e, s, info.key, &slot);
    });
    // `slot-clicked(s, slot)`: select a slot of the page shown on panel `s`.
    page.on_slot_clicked(|s, slot| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let index = e.selected_page[s as usize];
        select(&app, &mut e, s as usize, index, &slot);
    });
    // `page-chosen(s, i)`: show page `i` of panel `s` and select its first slot.
    page.on_page_chosen(|s, i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let Some(p) = e.layout.pages[s].get(i as usize) else { return };
        let slot = if p.full.is_some() { "full" } else { "top" };
        select(&app, &mut e, s, i as usize, slot);
    });
    // `duration-edited(s, text)`: typed into the duration box. Only whole numbers of 1 or more are
    // taken; anything else is ignored until it becomes one.
    page.on_duration_edited(|s, text| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let index = e.selected_page[s];
        let Ok(value) = text.trim().parse::<i64>() else { return };
        if let Some(p) = e.layout.pages[s].get_mut(index) {
            if value >= 1 && value != p.duration {
                p.duration = value;
                e.dirty = true;
                // Not show_panels: that would reset the box being typed in
                // Only the page list's label changes (it shows the duration); the box keeps exactly
                // what the user typed.
                let mut edit = panel_edit(&mut e, s);
                edit.duration = text;
                let page = app.global::<EditorPage>();
                if s == 0 {
                    page.set_left(edit);
                } else {
                    page.set_right(edit);
                }
                app.global::<App>().set_layout_dirty(true);
            }
        }
    });
    // `add-page(s)`: an empty page after the selected one, with the same duration.
    page.on_add_page(|s| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let i = e.selected_page[s];
        let duration = e.layout.pages[s].get(i).map(|p| p.duration).unwrap_or(model::DEFAULT_DURATION);
        // Inserted after the selected page (at the end if the selection is somehow past it).
        let at = (i + 1).min(e.layout.pages[s].len());
        e.layout.pages[s].insert(at, Page::with_duration(duration));
        select(&app, &mut e, s, at, "top");
        changed(&app, &mut e, true);
    });
    // `remove-page(s)`: remove the selected page; the last page is emptied instead (a panel needs one).
    page.on_remove_page(|s| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let i = e.selected_page[s];
        if e.layout.pages[s].len() <= 1 {
            let d = e.layout.pages[s][0].duration;
            e.layout.pages[s][0] = Page::with_duration(d);
        } else if i < e.layout.pages[s].len() {
            e.layout.pages[s].remove(i);
        }
        // Select the page that took the removed one's place (or the new last page).
        let i = i.min(e.layout.pages[s].len() - 1);
        select(&app, &mut e, s, i, "top");
        changed(&app, &mut e, true);
    });
    // `move-page(s, step)`: move the selected page up (-1) or down (+1); off either end does nothing.
    page.on_move_page(|s, step| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let i = e.selected_page[s];
        let j = i as i64 + step as i64;
        if j < 0 || j as usize >= e.layout.pages[s].len() {
            return;
        }
        // The selection moves with the page.
        e.layout.pages[s].swap(i, j as usize);
        let slot = match &e.selection {
            Some((ss, _, slot)) if *ss == s => slot.clone(),
            _ => "top".into(),
        };
        select(&app, &mut e, s, j as usize, &slot);
        changed(&app, &mut e, true);
    });
    // `clear-slot(s)`: empty the selected slot on panel `s`.
    page.on_clear_slot(|s| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let i = e.selected_page[s];
        let Some(p) = e.layout.pages[s].get(i) else { return };
        let slot = match &e.selection {
            Some((ss, _, slot)) if *ss == s => slot.clone(),
            _ => {
                if p.full.is_some() {
                    "full".into()
                } else {
                    "top".into()
                }
            }
        };
        // Emptying "full" or a half leaves an empty slot; the top slot is selected afterwards.
        if let Some(p) = e.layout.pages[s].get_mut(i) {
            *p.slot_mut(&slot) = None;
        }
        select(&app, &mut e, s, i, "top");
        changed(&app, &mut e, true);
    });
    // `port-chosen(s, index)`: pin this panel to a port. A known USB location is saved rather than the
    // COM port name, because Windows can renumber COM ports; "Automatic" removes both.
    page.on_port_chosen(|s, index| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let side = SIDES[s];
        // The chosen entry of `fill_ports`' list: "Automatic" has neither port nor location.
        let Some((_, port, location)) = e.ports[s].get(index as usize).cloned() else { return };
        // Only one of the two is ever saved for a side.
        e.layout.settings.remove(&format!("{side}-port"));
        e.layout.settings.remove(&format!("{side}-location"));
        if let Some(l) = location {
            e.layout.settings.insert(format!("{side}-location"), json!(l));
        } else if let Some(p) = port {
            e.layout.settings.insert(format!("{side}-port"), json!(p));
        }
        changed(&app, &mut e, false);
        // (The monitor opens its ports only when it starts.)
        status(&app, "Port changes take effect the next time the monitor starts.");
    });

    // The settings form
    // `field-toggled(i, on)`: a switch in the form; either a true/false setting or one option of a
    // several-of setting.
    page.on_field_toggled(|i, on| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        match e.acts.get(i as usize).cloned() {
            Some(Act::Plain(n)) => set_setting(&app, &mut e, n, json!(on), false),
            Some(Act::Multi(n, option)) => {
                let Some((s, p, slot)) = e.selection.clone() else { return };
                let Some(entry) = e.layout.pages[s].get(p).and_then(|pg| pg.slot(&slot)) else { return };
                let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let Some(st) = catalog::get(&name).settings.get(n) else { return };
                // Add or remove this option from the list saved now.
                let mut chosen: Vec<Value> = get_value(entry, st).as_array().cloned().unwrap_or_default();
                if on && !chosen.contains(&option) {
                    chosen.push(option);
                } else if !on {
                    chosen.retain(|v| *v != option);
                }
                // Keep the order of the options
                // (A saved value that isn't one of the options is dropped here.)
                let ordered: Vec<Value> = st.option_values().into_iter().filter(|o| chosen.contains(o)).collect();
                set_setting(&app, &mut e, n, Value::Array(ordered), false);
            }
            _ => {}
        }
    });
    // `field-chosen(i, index)`: a drop-down in the form.
    page.on_field_chosen(|i, index| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        // -1 (nothing selected) is treated as the first option.
        let index = index.max(0) as usize;
        match e.acts.get(i as usize).cloned() {
            Some(Act::Choice(n, values)) => {
                if let Some(v) = values.get(index) {
                    // Switching the weather's location source to or from "windows" shows or hides the
                    // location settings button, so the form is rebuilt then.
                    let refresh = matches!(v.as_str(), Some("windows"))
                        || e.acts.iter().any(|a| matches!(a, Act::LocationSettings));
                    set_setting(&app, &mut e, n, v.clone(), refresh);
                }
            }
            // Monitor-wide: saved in settings.json at once, then the monitor is asked to reload its
            // sensor sources and the result is shown under the drop-down.
            Some(Act::SensorSource) => {
                let key = SOURCES.get(index).map(|(k, _)| *k).unwrap_or("auto");
                settings::set("sensor_source", json!(key));
                e.sensor_note = "Saved. Checking...".into();
                show_settings(&app, &mut e);
                ask_sensors(true);
            }
            // Also monitor-wide: how often temperatures and fans are read.
            Some(Act::SensorRefresh) => {
                if let Some(seconds) = REFRESH_CHOICES.get(index) {
                    settings::set("sensor_refresh_seconds", json!(seconds));
                    e.sensor_note = "Saved. Checking...".into();
                    show_settings(&app, &mut e);
                    ask_sensors(true);
                }
            }
            _ => {}
        }
    });
    // `field-edited(i, text)`: a text box in the form. Whole-number settings ignore text that isn't a
    // whole number (yet); lists are split on commas.
    page.on_field_edited(|i, text| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        // Only plain settings have text boxes; any other index is a stale event and ignored.
        let Some(Act::Plain(n)) = e.acts.get(i as usize).cloned() else { return };
        let Some((s, p, slot)) = e.selection.clone() else { return };
        let name = e.layout.pages[s]
            .get(p)
            .and_then(|pg| pg.slot(&slot))
            .and_then(|en| en.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(st) = catalog::get(&name).settings.get(n) else { return };
        let raw = text.trim();
        // Turn the text into the setting's JSON type.
        let value = match st.kind {
            Kind::Int => match raw.parse::<i64>() {
                Ok(v) => json!(v),
                Err(_) => return,
            },
            Kind::List => {
                Value::Array(raw.split(',').map(str::trim).filter(|p| !p.is_empty()).map(number_or_text).collect())
            }
            _ => json!(raw),
        };
        set_setting(&app, &mut e, n, value, false);
    });
    // `field-button(i)`: a button in the form.
    page.on_field_button(|i| {
        let ed = editor();
        let act = ed.borrow().acts.get(i as usize).cloned();
        match act {
            Some(Act::RefreshSensors) => ask_sensors(false),
            Some(Act::RefreshApps) => list_running_apps(),
            Some(Act::LocationSettings) => {
                // A Windows settings link: opens Settings > Privacy > Location.
                crate::app::open_path("ms-settings:privacy-location");
            }
            _ => {}
        }
    });

    // Brightness: the running monitor changes it straight away, otherwise it is saved for next start
    // `brightness-changed(follow, level)`: follow the screen's brightness, or a fixed level 0-100.
    // Without a running monitor (no "ok" reply) the values are saved to settings.json for its next start.
    page.on_brightness_changed(|follow, level| {
        let ed = editor();
        // No change waiting means no timer running yet: this change starts one.
        let first = ed.borrow().brightness_pending.is_none();
        ed.borrow_mut().brightness_pending = Some((follow, level as f64));
        if first {
            // The slider sends many changes while dragging. The first change starts a 150 ms timer and
            // later ones only replace the pending value, so at most one request goes out per 150 ms and
            // the last value always gets sent.
            slint::Timer::single_shot(Duration::from_millis(150), || {
                let Some((follow, level)) = editor().borrow_mut().brightness_pending.take() else { return };
                crate::app::ctx().in_background(
                    move || {
                        let level = level.round() as i64;
                        let reply = link::request(
                            json!({"cmd": "brightness", "follow_screen": follow, "level": level}),
                            Duration::from_secs(3),
                        );
                        // No monitor took it: save it for its next start instead.
                        if ok_reply(reply).is_none() {
                            let mut s = settings::load();
                            s.insert("follow_screen_brightness".into(), json!(follow));
                            s.insert("brightness_level".into(), json!(level));
                            let _ = settings::save(&s);
                        }
                    },
                    |_, _| {},
                );
            });
        }
    });
    // `power-changed(key, on)`: turn off with the lid / when the screen is unused. The running monitor
    // takes it (and saves it); without one it is saved to settings.json for the next start.
    page.on_power_changed(|key, on| {
        let key = key.to_string();
        crate::app::ctx().in_background(
            move || {
                // `key` is the settings.json key ("turn_off_with_lid" or "turn_off_when_screen_unused").
                let reply = link::request(json!({"cmd": "power", key.clone(): on}), Duration::from_secs(3));
                if ok_reply(reply).is_none() {
                    settings::set(&key, json!(on));
                }
            },
            |_, _| {},
        );
    });

    // The buttons
    // `identify()`: each panel shows its name and app names for a few seconds (monitor only).
    page.on_identify(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        run_action(&app, "", || {
            // On a worker thread: any answer means the monitor did it.
            if link::request(json!({"cmd": "identify"}), Duration::from_secs(3)).is_some() {
                "The panels show their names (if you gave them any) and app names for a few seconds.".into()
            } else {
                "Identify needs the monitor to be running.".into()
            }
        });
    });
    // `resume()`: end a preview or anything else and go back to the layout (monitor only).
    page.on_resume(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        run_action(&app, "", || {
            if link::request(json!({"cmd": "resume"}), Duration::from_secs(3)).is_some() {
                "The monitor is showing your layout again.".into()
            } else {
                "The monitor isn't running.".into()
            }
        });
    });
    // `send-preview()`: show the preview pictures on the real panels. `send-target` picks both (0),
    // left (1) or right (2). Through the monitor for `PREVIEW_SECONDS`; without one, drawn directly.
    page.on_send_preview(|| {
        let ctx = crate::app::ctx();
        let Some(app) = ctx.ui.upgrade() else { return };
        let ed = editor();
        let e = ed.borrow();
        let sides: Vec<usize> = match app.global::<EditorPage>().get_send_target() {
            1 => vec![0],
            2 => vec![1],
            _ => vec![0, 1],
        };
        // The latest preview pictures; the timer draws them several times a second.
        let frames: Vec<(usize, Grid)> = sides.into_iter().filter_map(|s| e.frames[s].clone().map(|g| (s, g))).collect();
        if frames.is_empty() {
            status(&app, "The preview isn't ready yet.");
            return;
        }
        let sender = ctx.sender.clone();
        let panels = ctx.panels();
        run_action(&app, "Sending the preview...", move || {
            // Each picture goes as a list of 9 columns of 34 brightness values.
            let mut payload = Map::new();
            for (s, g) in &frames {
                payload.insert(SIDES[*s].into(), json!(g.v.iter().map(|c| c.to_vec()).collect::<Vec<_>>()));
            }
            // The monitor shows the pictures and goes back to the layout after `PREVIEW_SECONDS`. A reply
            // without "ok" means it refused (the error says why); no reply means no monitor.
            if let Some(reply) = link::request(json!({"cmd": "preview", "frames": payload, "seconds": PREVIEW_SECONDS}), Duration::from_secs(5)) {
                if !reply.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    return format!(
                        "That didn't work: {}",
                        reply.get("error").and_then(Value::as_str).unwrap_or("the monitor refused the preview")
                    );
                }
                return format!("Showing the preview for {PREVIEW_SECONDS} seconds (sent through the running monitor).");
            }
            // No monitor: draw on the panels directly
            // `ctx.panels()` was read when the window opened.
            if panels.is_empty() {
                return "That didn't work: no LED Matrix panels were found. Check that no other program is using them.".into();
            }
            let mut sent = Vec::new();
            for (s, g) in &frames {
                // `ctx.panels()` lists the left panel first, then the right.
                let Some(panel) = panels.get(*s) else { continue };
                // The grid holds whole numbers; the panel wants bytes 0-255, column by column.
                let mut bytes = [[0u8; wr_core::grid::HEIGHT]; wr_core::grid::WIDTH];
                for (x, col) in g.v.iter().enumerate() {
                    for (y, v) in col.iter().enumerate() {
                        bytes[x][y] = (*v).clamp(0, 255) as u8;
                    }
                }
                // Stop any firmware scrolling, then draw the picture with the greyscale commands.
                let mut writes = vec![cmd::animate(false)];
                writes.extend(cmd::greyscale(&bytes));
                match sender.write(&panel.device, &writes) {
                    Ok(()) => sent.push(format!("{} ({})", SIDES[*s], panel.device)),
                    Err(e) => return format!("That didn't work: {e}"),
                }
            }
            // Free the ports again so the monitor can open them when it starts.
            sender.close_direct();
            if sent.is_empty() {
                "That didn't work: that panel isn't connected.".into()
            } else {
                format!(
                    "Sent the preview to the {} panel. The monitor isn't running, so it stays until something else draws.",
                    sent.join(" and ")
                )
            }
        });
    });
    // `apply()`: save the layout file, then tell the monitor to reload it (on a worker thread).
    page.on_apply(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        if !save(&app, &mut e) {
            show_panels(&app, &mut e);
            return;
        }
        show_panels(&app, &mut e);
        // Saved (on failure `save` showed what went wrong in a dialog and we returned above).
        let path = e.save_path.display().to_string();
        run_action(&app, "Saving...", move || {
            if reload_monitor() {
                "Saved. The panels switch to the new layout in a moment.".into()
            } else {
                format!("Saved to {path}. The monitor isn't running; click Start monitor to show it.")
            }
        });
    });
    // `start-monitor()`: save unsaved changes first, start the monitor, and check on it 3 seconds later.
    page.on_start_monitor(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        // Unsaved changes are saved first, so the monitor starts with what the user sees.
        if e.dirty && !save(&app, &mut e) {
            return;
        }
        show_panels(&app, &mut e);
        // The monitor takes a moment to start and open the panels, hence the check 3 seconds later.
        match start_monitor() {
            Ok(()) => {
                status(&app, "Starting the monitor...");
                crate::app::ctx().after(3.0, |_| check_monitor());
            }
            Err(err) => status(&app, format!("Could not start the monitor: {err}")),
        }
    });
    // `preset(i)`: replace both panels' pages with a ready-made layout, after asking. Not saved yet.
    page.on_preset(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some((name, _)) = model::PRESETS.get(i as usize) else { return };
        if !dialogs::confirm(
            "Ready-made layout",
            &format!("Replace both panels' pages with the {name} layout?\n\nNothing is saved until you press Apply to panels."),
        ) {
            return;
        }
        let ed = editor();
        let mut e = ed.borrow_mut();
        if let Some(pages) = model::preset_pages(name, &first_snapshot) {
            e.layout.pages = pages;
            e.selected_page = [0, 0];
            e.selection = None;
            changed(&app, &mut e, true);
            status(&app, format!("Using the {name} layout. Press Apply to panels to show it."));
        }
    });
    // `revert()`: throw away the changes (after asking) and read the file again.
    page.on_revert(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        if e.dirty && !dialogs::confirm("Revert", "Discard your changes and reload the saved layout?") {
            return;
        }
        // The warnings were shown when the file was first read; not again.
        let (layout, _) = read_layout(e.load_path.as_ref());
        e.layout = layout;
        e.selection = None;
        e.dirty = false;
        fill_ports(&mut e);
        show_panels(&app, &mut e);
        show_settings(&app, &mut e);
        status(&app, "Reloaded the saved layout.");
    });
    // `swap()`: the panels are the wrong way round. Works out which port is which now, writes the pins the
    // other way round, saves at once, and asks the monitor to reload and identify so the user can check.
    page.on_swap(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        // The pins the layout file has now.
        let get = |k: &str| e.layout.settings.get(k).and_then(Value::as_str).map(String::from);
        let pins = wr_panel::ports::Pins {
            left_port: get("left-port"),
            right_port: get("right-port"),
            left_location: get("left-location"),
            right_location: get("right-location"),
        };
        // `discover` orders the panels left, right using the current pins, as the monitor would; the
        // swap then writes them the other way round.
        let found = wr_panel::ports::discover(&panels::list_ports(), &pins);
        let Some(swapped) = model::swapped_pins(&found) else {
            status(&app, "Swap left and right needs both panels connected.");
            return;
        };
        // Replace every pin with the swapped ones (ports or locations, see `model::swapped_pins`).
        for k in ["left-port", "right-port", "left-location", "right-location"] {
            e.layout.settings.remove(k);
        }
        e.layout.settings.extend(swapped);
        fill_ports(&mut e);
        changed(&app, &mut e, false);
        if !save(&app, &mut e) {
            return;
        }
        show_panels(&app, &mut e);
        run_action(&app, "Swapping...", || {
            if reload_monitor() {
                // The monitor reopens the panels with the new pins, then shows each panel's name.
                let _ = link::request(json!({"cmd": "identify"}), Duration::from_secs(3));
                "Swapped. Each panel shows its name for a few seconds so you can check.".into()
            } else {
                "Swapped and saved. Start the monitor to see it.".into()
            }
        });
    });

    // The live preview while the page shows, and the monitor's state every 5 seconds
    {
        let ui = ctx.ui.clone();
        ctx.every(FRAME_SECONDS, move || {
            let Some(app) = ui.upgrade() else { return };
            // (Skipped while another page is showing, as nobody would see it.)
            if app.global::<App>().get_page() != "monitor" {
                return;
            }
            let ed = editor();
            let mut e = ed.borrow_mut();
            // Draw each panel's selected page with the editor's own copy of the monitor's drawing code.
            for s in 0..2 {
                let page = e.layout.pages[s].get(e.selected_page[s]).cloned();
                let grid = e.renderer.render(page.as_ref(), SIDES[s]);
                e.frames[s] = Some(grid);
            }
            show_frames(&app, &mut e);
        });
    }
    {
        let ui = ctx.ui.clone();
        // The monitor check runs every 5 seconds while the page is showing, and at once when the user
        // comes back to it (the count restarts on leaving the page).
        let mut ticks = 0u32;
        ctx.every(1.0, move || {
            let Some(app) = ui.upgrade() else { return };
            if app.global::<App>().get_page() != "monitor" {
                ticks = 0;
                return;
            }
            if ticks.is_multiple_of(5) {
                check_monitor();
            }
            ticks += 1;
        });
    }
    // Each time the page is shown, list the ports again: a panel may have been plugged in or moved.
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "monitor" {
            let ed = editor();
            let mut e = ed.borrow_mut();
            fill_ports(&mut e);
            show_panels(app, &mut e);
        }
    }));
}
