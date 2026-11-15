//! The layout editor page (`led_mon/editor/window.py`): placing apps on each panel's pages, the
//! selected app's settings, the live preview, and saving the layout file for the monitor.

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

const FRAME_SECONDS: f64 = 0.2;
const PREVIEW_SECONDS: i64 = 30;
const FIRST_GPU: &str = "First GPU found";
const AUTOMATIC: &str = "Automatic";
const AUTO_GAME: &str = "Automatic (the game in front)";

const SOURCES: [(&str, &str); 4] = [
    ("auto", "Automatic"),
    ("framework_control", "Framework Control"),
    ("libre_hardware_monitor", "Libre Hardware Monitor"),
    ("framework_tool", "framework_tool"),
];
const SOURCE_HELP: [&str; 4] = [
    "Tries Framework Control, then Libre Hardware Monitor, then framework_tool, and uses the first one that gives readings (temperatures and fans can come from different ones).",
    "Needs Framework Control installed; its service runs in the background, so nothing has to be open and no admin rights are needed. On a Framework 16 it gives the CPU, APU, ambient and charger temperatures and both fans.",
    "Needs Libre Hardware Monitor open (it can sit in the tray) and running as admin; White Rabbit reads its sensors through Windows. Which sensors show depends on the laptop: GPU and SSD temperatures are likely, fans may be missing.",
    "Needs framework_tool.exe next to WhiteRabbit.exe or on PATH, and White Rabbit itself running as admin; otherwise Windows refuses to start it.",
];
const REFRESH_CHOICES: [f64; 5] = [1.0, 2.0, 5.0, 10.0, 30.0];
const REFRESH_HELP: &str = "How often the temperatures and fans are read. Shorter follows changes faster; framework_tool and Libre Hardware Monitor use a little more CPU when read often.";

/// What a line of the settings form changes.
#[derive(Clone, Debug)]
enum Act {
    None,
    /// A bool, text, number or list setting (by its index in the app's settings)
    Plain(usize),
    /// A pick from these values
    Choice(usize, Vec<Value>),
    /// One option of a several-of setting
    Multi(usize, Value),
    SensorSource,
    SensorRefresh,
    RefreshSensors,
    RefreshApps,
    LocationSettings,
}

struct Editor {
    layout: EditorLayout,
    load_path: Option<PathBuf>,
    save_path: PathBuf,
    /// (side, page, "top" | "bottom" | "full")
    selection: Option<(usize, usize, String)>,
    selected_page: [usize; 2],
    dirty: bool,
    renderer: Renderer,
    frames: [Option<Grid>; 2],
    ports: [Vec<(String, Option<String>, Option<String>)>; 2],
    acts: Vec<Act>,
    /// The monitor's last status reply
    monitor: Option<Value>,
    /// The monitor's sensor_source reply
    sensors: Option<Value>,
    sensor_note: String,
    microphones: Vec<String>,
    running_apps: Vec<(String, String)>,
    brightness_pending: Option<(bool, f64)>,
}

thread_local! {
    static EDITOR: RefCell<Option<Rc<RefCell<Editor>>>> = const { RefCell::new(None) };
}

fn editor() -> Rc<RefCell<Editor>> {
    EDITOR.with(|e| e.borrow().clone()).expect("layout editor wired")
}

/// Whether the layout has changes not applied yet (asked when the window closes).
pub fn is_dirty() -> bool {
    EDITOR.with(|e| e.borrow().as_ref().map(|e| e.borrow().dirty)).unwrap_or(false)
}

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

fn save_path() -> PathBuf {
    std::env::var_os("CONFIG_FILE").map(PathBuf::from).unwrap_or_else(|| paths::config_dir().join("config-local.yaml"))
}

// Pictures for the Picture app ------------------------------------------------------------------------

const BUILT_IN: [(&str, &[&str]); 2] = [
    ("left", &["double-check.json", "the-way-left.json", "zigzag.json"]),
    ("right", &["check.json", "every-third-row.json", "the-way-right.json"]),
];

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

fn first_snapshot(side: &str) -> Option<String> {
    snapshot_options(side).into_iter().next().map(|(v, _)| v)
}

// Showing the editor ----------------------------------------------------------------------------------

fn panel_edit(e: &mut Editor, s: usize) -> PanelEdit {
    let side = SIDES[s];
    let pages = &e.layout.pages[s];
    e.selected_page[s] = e.selected_page[s].min(pages.len().saturating_sub(1));
    let page = pages.get(e.selected_page[s]);
    let lines: Vec<SharedString> =
        pages.iter().enumerate().map(|(i, p)| format!("{}. {}", i + 1, p.describe()).into()).collect();
    let selected = match &e.selection {
        Some((ss, pi, slot)) if *ss == s && *pi == e.selected_page[s] => slot.clone(),
        _ => String::new(),
    };
    let levels = e.frames[s].as_ref().map(|g| preview::levels_of(g, 160.0)).unwrap_or_else(preview::dark);
    let labels: Vec<SharedString> = e.ports[s].iter().map(|(l, _, _)| l.as_str().into()).collect();
    let current = port_index(e, s);
    PanelEdit {
        title: format!("{}{} panel", side[..1].to_uppercase(), &side[1..]).into(),
        ports: ModelRc::new(VecModel::from(labels)),
        port: current as i32,
        pages: ModelRc::new(VecModel::from(lines)),
        page: e.selected_page[s] as i32,
        duration: page.map(|p| p.duration.to_string()).unwrap_or_default().into(),
        image: preview::render_styled(&[levels], 2, preview::LARGE),
        full: page.is_some_and(|p| p.full.is_some()),
        top_empty: page.is_none_or(|p| p.top.is_none()),
        bottom_empty: page.is_none_or(|p| p.bottom.is_none()),
        selected: selected.into(),
    }
}

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

fn connected_ports() -> Vec<(String, Option<String>)> {
    panels::list_ports()
        .into_iter()
        .filter(wr_panel::ports::is_led_matrix_port)
        .map(|p| (p.device, p.location.map(|l| l.split(':').next().unwrap_or("").to_string())))
        .collect()
}

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

fn show_panels(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    page.set_left(panel_edit(e, 0));
    page.set_right(panel_edit(e, 1));
    app.global::<App>().set_layout_dirty(e.dirty);
}

/// Only the preview pictures changed.
fn show_frames(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    for s in 0..2 {
        let levels = e.frames[s].as_ref().map(|g| preview::levels_of(g, 160.0)).unwrap_or_else(preview::dark);
        let image = preview::render_styled(&[levels], 2, preview::LARGE);
        let mut p = if s == 0 { page.get_left() } else { page.get_right() };
        p.image = image;
        if s == 0 {
            page.set_left(p);
        } else {
            page.set_right(p);
        }
    }
}

fn field(kind: &str, label: impl Into<SharedString>) -> Field {
    Field { kind: kind.into(), label: label.into(), ..Field::default() }
}

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

fn get_value(entry: &Entry, setting: &catalog::Setting) -> Value {
    let found = match setting.target {
        Target::App => entry.get(setting.key).cloned(),
        _ => entry.get("args").and_then(|a| a.get(setting.key)).cloned(),
    };
    found.unwrap_or_else(|| setting.default_value())
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// The sensor_source status as one or two lines (`sensor_sources.describe`).
fn describe_sensors(status: Option<&Value>) -> String {
    let Some(status) = status.filter(|s| s.as_object().is_some_and(|m| !m.is_empty())) else {
        return "The monitor isn't running.".into();
    };
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

fn show_settings(app: &AppWindow, e: &mut Editor) {
    let page = app.global::<EditorPage>();
    let mut fields: Vec<Field> = Vec::new();
    let mut acts: Vec<Act> = Vec::new();
    let mut push = |f: Field, a: Act| {
        fields.push(f);
        acts.push(a);
    };
    let entry = e
        .selection
        .as_ref()
        .and_then(|(s, i, slot)| e.layout.pages[*s].get(*i).map(|p| (p.slot(slot).cloned(), *s, *i, slot.clone())));
    match &entry {
        None => {
            page.set_settings_title("No app selected".into());
            page.set_settings_help(
                "Drag an app from the list on the left onto a panel, or select a slot and double-click an app.".into(),
            );
        }
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
            page.set_settings_title(if info.key == "plugin" { name.into() } else { info.label.into() });
            page.set_settings_help(format!("{}\n{}", info.help, where_text(*s, *i, slot)).into());
            let shown: Vec<(usize, &catalog::Setting)> = info
                .settings
                .iter()
                .enumerate()
                .filter(|(_, st)| {
                    cfg!(windows)
                        || !matches!(st.kind, Kind::SensorSource | Kind::LocationSource)
                        || std::env::var_os("WHITE_RABBIT_FAKE_PANELS").is_some()
                })
                .collect();
            if shown.is_empty() {
                let mut f = field("status", "");
                f.text = "This app has no settings.".into();
                push(f, Act::None);
            }
            for (n, st) in shown {
                let value = get_value(entry, st);
                match st.kind {
                    Kind::Bool => {
                        let mut f = field("bool", st.label);
                        f.checked = model::is_true(Some(&value));
                        f.help = st.help.into();
                        push(f, Act::Plain(n));
                    }
                    Kind::Text | Kind::Int | Kind::List => {
                        let mut f = field("text", st.label);
                        f.text = value_text(&value).into();
                        f.help = st.help.into();
                        push(f, Act::Plain(n));
                    }
                    Kind::Choice | Kind::Snapshot | Kind::Gpu | Kind::Microphone | Kind::LocationSource => {
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
                            choice_field(st.label, labels, index.unwrap_or(usize::MAX), &help),
                            Act::Choice(n, values),
                        );
                        if st.kind == Kind::LocationSource && value == json!("windows") {
                            push(field("button", "Windows location settings"), Act::LocationSettings);
                        }
                    }
                    Kind::Sensor => {
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
                        if st.key == "sensor_1" {
                            push(field("button", "Refresh list"), Act::RefreshSensors);
                        }
                    }
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
                    Kind::Multi => {
                        push(field("heading", st.label), Act::None);
                        let current: Vec<Value> = value.as_array().cloned().unwrap_or_default();
                        for (i, option) in st.option_values().into_iter().enumerate() {
                            let mut f = field("bool", st.option_label(i).replace('_', " "));
                            f.checked = current.contains(&option);
                            push(f, Act::Multi(n, option));
                        }
                    }
                    Kind::SensorSource => {
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
    e.acts = acts;
    page.set_fields(ModelRc::new(VecModel::from(fields)));
}

fn where_text(s: usize, i: usize, slot: &str) -> String {
    let side = SIDES[s];
    let place = match slot {
        "full" => "whole panel",
        "top" => "top half",
        _ => "bottom half",
    };
    format!("{}{} panel, page {}, {place}", side[..1].to_uppercase(), &side[1..], i + 1)
}

fn status(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<EditorPage>().set_status(text.into());
}

// Changing the layout ---------------------------------------------------------------------------------

fn select(app: &AppWindow, e: &mut Editor, s: usize, index: usize, slot: &str) {
    e.selection = Some((s, index, slot.to_string()));
    e.selected_page[s] = index;
    show_panels(app, e);
    show_settings(app, e);
    // The Temperatures app's sensor list comes from the monitor
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

fn changed(app: &AppWindow, e: &mut Editor, refresh_settings: bool) {
    e.dirty = true;
    show_panels(app, e);
    if refresh_settings {
        show_settings(app, e);
    }
}

fn place(app: &AppWindow, e: &mut Editor, s: usize, key: &str, slot: &str) {
    let side = SIDES[s];
    let index = e.selected_page[s];
    let Some(page) = e.layout.pages[s].get_mut(index) else { return };
    let entry = catalog::new_app(key, side, first_snapshot(side));
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
fn set_setting(app: &AppWindow, e: &mut Editor, n: usize, value: Value, refresh: bool) {
    let Some((s, i, slot)) = e.selection.clone() else { return };
    let Some(entry) = e.layout.pages[s].get_mut(i).and_then(|p| p.slot_mut(&slot).as_mut()) else { return };
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(st) = catalog::get(&name).settings.get(n) else { return };
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
    let empty = value.is_null() || value == json!("") || value == json!([]);
    if empty && matches!(st.kind, Kind::Text | Kind::Process | Kind::List | Kind::Gpu | Kind::Sensor | Kind::Microphone)
    {
        target.remove(st.key);
    } else {
        target.insert(st.key.into(), value);
    }
    changed(app, e, refresh);
}

fn number_or_text(text: &str) -> Value {
    if text.contains('.') {
        if let Ok(f) = text.parse::<f64>() {
            return json!(f);
        }
    }
    json!(text)
}

// Talking to the monitor ------------------------------------------------------------------------------

fn ask(message: Value, timeout: f64, done: impl FnOnce(&AppWindow, Option<Value>) + Send + 'static) {
    crate::app::ctx().in_background(move || link::request(message, Duration::from_secs_f64(timeout)), done);
}

fn ok_reply(reply: Option<Value>) -> Option<Value> {
    reply.filter(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false))
}

fn ask_sensors(reload: bool) {
    ask(json!({"cmd": "sensor_source", "reload": reload}), 8.0, move |app, reply| {
        let ed = editor();
        let mut e = ed.borrow_mut();
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

fn list_running_apps() {
    crate::app::ctx().in_background(crate::winapps::running_apps, |app, apps| {
        let ed = editor();
        let mut e = ed.borrow_mut();
        e.running_apps = apps;
        show_settings(app, &mut e);
    });
}

fn check_monitor() {
    ask(json!({"cmd": "status"}), 1.5, |app, reply| {
        let ed = editor();
        let mut e = ed.borrow_mut();
        let status = ok_reply(reply);
        let was_running = e.monitor.is_some();
        e.monitor = status.clone();
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

fn status_text(app: &AppWindow, text: String) {
    status(app, text);
}

fn show_brightness(app: &AppWindow, b: &Value) {
    let page = app.global::<EditorPage>();
    page.set_follow_screen(b.get("follow_screen").and_then(Value::as_bool).unwrap_or(true));
    page.set_brightness(b.get("level").and_then(Value::as_f64).unwrap_or(70.0) as f32);
}

/// Runs a step on a worker thread and shows what it says in the status line.
fn run_action(app: &AppWindow, busy: &str, action: impl FnOnce() -> String + Send + 'static) {
    if !busy.is_empty() {
        status(app, busy);
    }
    crate::app::ctx().in_background(action, |app, text| status(app, text));
}

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
    e.load_path = Some(e.save_path.clone());
    e.dirty = false;
    app.global::<App>().set_layout_dirty(false);
    true
}

fn reload_monitor() -> bool {
    link::request(json!({"cmd": "reload"}), Duration::from_secs(3)).is_some()
}

/// Asked when the window closes: apply the changes (true) or drop them.
pub fn apply_on_close() {
    let ed = editor();
    let e = ed.borrow_mut();
    if e.dirty && dialogs::confirm("System monitor", "Apply your layout changes before closing?") {
        if model::save(&e.layout, &e.save_path).is_ok() {
            reload_monitor();
        }
    }
}

fn start_monitor() -> Result<(), String> {
    let exe = paths::app_dir().join(if cfg!(windows) { "WhiteRabbit.exe" } else { "WhiteRabbit" });
    std::process::Command::new(&exe)
        .arg("--background")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", exe.display()))
}

// Wiring ----------------------------------------------------------------------------------------------

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
    EDITOR.with(|x| *x.borrow_mut() = Some(e.clone()));
    let page = app.global::<EditorPage>();
    let rows: Vec<AppRow> = catalog::offered()
        .map(|a| AppRow {
            key: a.key.into(),
            label: a.label.into(),
            size: if a.size == Size::Full { "Whole panel" } else { "Half panel" }.into(),
        })
        .collect();
    page.set_apps(ModelRc::new(VecModel::from(rows)));
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
    if !warnings.is_empty() {
        status(app, warnings.join(" "));
    }
    // Brightness and turning off, as saved (the running monitor's when it answers)
    let saved = settings::load();
    page.set_follow_screen(saved.get("follow_screen_brightness").and_then(Value::as_bool).unwrap_or(true));
    page.set_brightness(saved.get("brightness_level").and_then(Value::as_f64).unwrap_or(70.0) as f32);
    page.set_off_with_lid(saved.get("turn_off_with_lid").and_then(Value::as_bool).unwrap_or(true));
    page.set_off_when_unused(saved.get("turn_off_when_screen_unused").and_then(Value::as_bool).unwrap_or(true));
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
    ctx.in_background(wr_music::capture::list_microphones, |_, names| editor().borrow_mut().microphones = names);

    page.on_app_chosen(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        if let Some(info) = catalog::offered().nth(i as usize) {
            let size = if info.size == Size::Full { "whole panel" } else { "top or bottom half" };
            app.global::<EditorPage>().set_app_help(format!("{}\nGoes in: {size}.", info.help).into());
        }
    });
    page.on_place(|i, s, slot| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some(info) = catalog::offered().nth(i as usize) else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        place(&app, &mut e, s as usize, info.key, &slot);
    });
    page.on_place_selected(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let Some(info) = catalog::offered().nth(i as usize) else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let (s, slot) = match &e.selection {
            Some((s, _, slot)) => (*s, slot.clone()),
            None => (0, "top".to_string()),
        };
        place(&app, &mut e, s, info.key, &slot);
    });
    page.on_slot_clicked(|s, slot| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let index = e.selected_page[s as usize];
        select(&app, &mut e, s as usize, index, &slot);
    });
    page.on_page_chosen(|s, i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let Some(p) = e.layout.pages[s].get(i as usize) else { return };
        let slot = if p.full.is_some() { "full" } else { "top" };
        select(&app, &mut e, s, i as usize, slot);
    });
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
    page.on_add_page(|s| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let i = e.selected_page[s];
        let duration = e.layout.pages[s].get(i).map(|p| p.duration).unwrap_or(model::DEFAULT_DURATION);
        let at = (i + 1).min(e.layout.pages[s].len());
        e.layout.pages[s].insert(at, Page::with_duration(duration));
        select(&app, &mut e, s, at, "top");
        changed(&app, &mut e, true);
    });
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
        let i = i.min(e.layout.pages[s].len() - 1);
        select(&app, &mut e, s, i, "top");
        changed(&app, &mut e, true);
    });
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
        e.layout.pages[s].swap(i, j as usize);
        let slot = match &e.selection {
            Some((ss, _, slot)) if *ss == s => slot.clone(),
            _ => "top".into(),
        };
        select(&app, &mut e, s, j as usize, &slot);
        changed(&app, &mut e, true);
    });
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
        if let Some(p) = e.layout.pages[s].get_mut(i) {
            *p.slot_mut(&slot) = None;
        }
        select(&app, &mut e, s, i, "top");
        changed(&app, &mut e, true);
    });
    page.on_port_chosen(|s, index| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let s = s as usize;
        let side = SIDES[s];
        let Some((_, port, location)) = e.ports[s].get(index as usize).cloned() else { return };
        e.layout.settings.remove(&format!("{side}-port"));
        e.layout.settings.remove(&format!("{side}-location"));
        if let Some(l) = location {
            e.layout.settings.insert(format!("{side}-location"), json!(l));
        } else if let Some(p) = port {
            e.layout.settings.insert(format!("{side}-port"), json!(p));
        }
        changed(&app, &mut e, false);
        status(&app, "Port changes take effect the next time the monitor starts.");
    });

    // The settings form
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
                let mut chosen: Vec<Value> = get_value(entry, st).as_array().cloned().unwrap_or_default();
                if on && !chosen.contains(&option) {
                    chosen.push(option);
                } else if !on {
                    chosen.retain(|v| *v != option);
                }
                // Keep the order of the options
                let ordered: Vec<Value> = st.option_values().into_iter().filter(|o| chosen.contains(o)).collect();
                set_setting(&app, &mut e, n, Value::Array(ordered), false);
            }
            _ => {}
        }
    });
    page.on_field_chosen(|i, index| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let index = index.max(0) as usize;
        match e.acts.get(i as usize).cloned() {
            Some(Act::Choice(n, values)) => {
                if let Some(v) = values.get(index) {
                    let refresh = matches!(v.as_str(), Some("windows"))
                        || e.acts.iter().any(|a| matches!(a, Act::LocationSettings));
                    set_setting(&app, &mut e, n, v.clone(), refresh);
                }
            }
            Some(Act::SensorSource) => {
                let key = SOURCES.get(index).map(|(k, _)| *k).unwrap_or("auto");
                settings::set("sensor_source", json!(key));
                e.sensor_note = "Saved. Checking...".into();
                show_settings(&app, &mut e);
                ask_sensors(true);
            }
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
    page.on_field_edited(|i, text| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
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
    page.on_field_button(|i| {
        let ed = editor();
        let act = ed.borrow().acts.get(i as usize).cloned();
        match act {
            Some(Act::RefreshSensors) => ask_sensors(false),
            Some(Act::RefreshApps) => list_running_apps(),
            Some(Act::LocationSettings) => {
                crate::app::open_path("ms-settings:privacy-location");
            }
            _ => {}
        }
    });

    // Brightness: the running monitor changes it straight away, otherwise it is saved for next start
    page.on_brightness_changed(|follow, level| {
        let ed = editor();
        let first = ed.borrow().brightness_pending.is_none();
        ed.borrow_mut().brightness_pending = Some((follow, level as f64));
        if first {
            // The slider sends many changes while dragging; send the latest one a moment after it stops
            slint::Timer::single_shot(Duration::from_millis(150), || {
                let Some((follow, level)) = editor().borrow_mut().brightness_pending.take() else { return };
                crate::app::ctx().in_background(
                    move || {
                        let level = level.round() as i64;
                        let reply = link::request(
                            json!({"cmd": "brightness", "follow_screen": follow, "level": level}),
                            Duration::from_secs(3),
                        );
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
    page.on_power_changed(|key, on| {
        let key = key.to_string();
        crate::app::ctx().in_background(
            move || {
                let reply = link::request(json!({"cmd": "power", key.clone(): on}), Duration::from_secs(3));
                if ok_reply(reply).is_none() {
                    settings::set(&key, json!(on));
                }
            },
            |_, _| {},
        );
    });

    // The buttons
    page.on_identify(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        run_action(&app, "", || {
            if link::request(json!({"cmd": "identify"}), Duration::from_secs(3)).is_some() {
                "The panels show their names (if you gave them any) and app names for a few seconds.".into()
            } else {
                "Identify needs the monitor to be running.".into()
            }
        });
    });
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
        let frames: Vec<(usize, Grid)> = sides.into_iter().filter_map(|s| e.frames[s].clone().map(|g| (s, g))).collect();
        if frames.is_empty() {
            status(&app, "The preview isn't ready yet.");
            return;
        }
        let sender = ctx.sender.clone();
        let panels = ctx.panels();
        run_action(&app, "Sending the preview...", move || {
            let mut payload = Map::new();
            for (s, g) in &frames {
                payload.insert(SIDES[*s].into(), json!(g.v.iter().map(|c| c.to_vec()).collect::<Vec<_>>()));
            }
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
            if panels.is_empty() {
                return "That didn't work: no LED Matrix panels were found. Check that no other program is using them.".into();
            }
            let mut sent = Vec::new();
            for (s, g) in &frames {
                let Some(panel) = panels.get(*s) else { continue };
                let mut bytes = [[0u8; wr_core::grid::HEIGHT]; wr_core::grid::WIDTH];
                for (x, col) in g.v.iter().enumerate() {
                    for (y, v) in col.iter().enumerate() {
                        bytes[x][y] = (*v).clamp(0, 255) as u8;
                    }
                }
                let mut writes = vec![cmd::animate(false)];
                writes.extend(cmd::greyscale(&bytes));
                match sender.write(&panel.device, &writes) {
                    Ok(()) => sent.push(format!("{} ({})", SIDES[*s], panel.device)),
                    Err(e) => return format!("That didn't work: {e}"),
                }
            }
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
    page.on_apply(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        if !save(&app, &mut e) {
            show_panels(&app, &mut e);
            return;
        }
        show_panels(&app, &mut e);
        let path = e.save_path.display().to_string();
        run_action(&app, "Saving...", move || {
            if reload_monitor() {
                "Saved. The panels switch to the new layout in a moment.".into()
            } else {
                format!("Saved to {path}. The monitor isn't running; click Start monitor to show it.")
            }
        });
    });
    page.on_start_monitor(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        if e.dirty && !save(&app, &mut e) {
            return;
        }
        show_panels(&app, &mut e);
        match start_monitor() {
            Ok(()) => {
                status(&app, "Starting the monitor...");
                crate::app::ctx().after(3.0, |_| check_monitor());
            }
            Err(err) => status(&app, format!("Could not start the monitor: {err}")),
        }
    });
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
    page.on_revert(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        if e.dirty && !dialogs::confirm("Revert", "Discard your changes and reload the saved layout?") {
            return;
        }
        let (layout, _) = read_layout(e.load_path.as_ref());
        e.layout = layout;
        e.selection = None;
        e.dirty = false;
        fill_ports(&mut e);
        show_panels(&app, &mut e);
        show_settings(&app, &mut e);
        status(&app, "Reloaded the saved layout.");
    });
    page.on_swap(|| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let ed = editor();
        let mut e = ed.borrow_mut();
        let get = |k: &str| e.layout.settings.get(k).and_then(Value::as_str).map(String::from);
        let pins = wr_panel::ports::Pins {
            left_port: get("left-port"),
            right_port: get("right-port"),
            left_location: get("left-location"),
            right_location: get("right-location"),
        };
        let found = wr_panel::ports::discover(&panels::list_ports(), &pins);
        let Some(swapped) = model::swapped_pins(&found) else {
            status(&app, "Swap left and right needs both panels connected.");
            return;
        };
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
            if app.global::<App>().get_page() != "monitor" {
                return;
            }
            let ed = editor();
            let mut e = ed.borrow_mut();
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
        let mut ticks = 0u32;
        ctx.every(1.0, move || {
            let Some(app) = ui.upgrade() else { return };
            if app.global::<App>().get_page() != "monitor" {
                ticks = 0;
                return;
            }
            if ticks % 5 == 0 {
                check_monitor();
            }
            ticks += 1;
        });
    }
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "monitor" {
            let ed = editor();
            let mut e = ed.borrow_mut();
            fill_ports(&mut e);
            show_panels(app, &mut e);
        }
    }));
}
