//! The layout editor's model, and its conversion to and from the layout file (`led_mon/editor/layout.py`).
//!
//! The file gives each of the four quarters its own rotating list of apps. The editor shows each panel
//! as a list of pages instead: one app on the whole panel, or a top app and a bottom app, shown together
//! for the page's duration. Writing a page puts its top app (or the whole-panel app, with scope: panel)
//! in the top quarter's list and its bottom app (or an empty display: false slot) in the bottom quarter's
//! list, with the same duration, so the two lists stay in step.
//!
//! The file is YAML (config-local.yaml in the settings folder); it is handled here as a JSON value
//! (`serde_json::Value`), which `serde_yaml` reads and writes, so the editor and the monitor's parser
//! (`wr_core::layout`) see the same data. Everything here is plain data and functions, called by
//! `editor.rs` on the UI thread; `save` and `load_file` touch the disk (small files, quick).

use std::path::Path;

use serde_json::{json, Map, Value};

use super::catalog;

/// The two panels, in the order used by `EditorLayout::pages` and everywhere else here.
pub const SIDES: [&str; 2] = ["left", "right"];
/// Seconds a page shows before the next, when nothing else says.
pub const DEFAULT_DURATION: i64 = 30;
/// Written at the top of the saved file, so someone opening it by hand knows where it came from.
const HEADER: &str = "# Written by the LED Matrix layout editor. You can still edit this file by hand;\n\
# see config-README.md. The editor keeps a copy of the previous version in config-local.yaml.bak\n\n";

/// One app as the editor holds it: "name", any flags, and "args" (see `catalog.rs`).
pub type Entry = Map<String, Value>;

/// One page of a panel's rotation: either `full`, or `top` and/or `bottom` (an empty half is None).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// Seconds the page shows.
    pub duration: i64,
    /// One app on the whole panel
    pub full: Option<Entry>,
    /// The app in the top half (rows 0-16), when there is no whole-panel app.
    pub top: Option<Entry>,
    /// The app in the bottom half (rows 17-33), when there is no whole-panel app.
    pub bottom: Option<Entry>,
}

/// An empty page of the default duration.
impl Default for Page {
    fn default() -> Self {
        Page { duration: DEFAULT_DURATION, full: None, top: None, bottom: None }
    }
}

impl Page {
    /// An empty page of the given duration.
    pub fn with_duration(duration: i64) -> Page {
        Page { duration, ..Page::default() }
    }

    /// The app in a slot named as the Slint side names them: "full", "top", anything else is "bottom".
    pub fn slot(&self, slot: &str) -> Option<&Entry> {
        match slot {
            "full" => self.full.as_ref(),
            "top" => self.top.as_ref(),
            _ => self.bottom.as_ref(),
        }
    }

    /// A slot to change, named as in `slot`.
    pub fn slot_mut(&mut self, slot: &str) -> &mut Option<Entry> {
        match slot {
            "full" => &mut self.full,
            "top" => &mut self.top,
            _ => &mut self.bottom,
        }
    }

    /// "1. CPU cores / Memory and battery  (30 s)" without the number
    pub fn describe(&self) -> String {
        // "CPU cores / Memory and battery" for two halves, one name for a whole-panel app.
        let what = if self.full.is_some() {
            catalog::label(self.full.as_ref())
        } else if self.top.is_none() && self.bottom.is_none() {
            "(empty)".into()
        } else {
            format!("{} / {}", catalog::label(self.top.as_ref()), catalog::label(self.bottom.as_ref()))
        };
        format!("{what}  ({} s)", self.duration)
    }
}

/// The whole layout as the editor edits it.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorLayout {
    /// left, right
    pub pages: [Vec<Page>; 2],
    /// Everything else in the file (duration, left-port, right-port, ...), kept as it was
    pub settings: Map<String, Value>,
}

/// One empty page per panel and a 10-second default duration: what a missing or unreadable file becomes.
impl Default for EditorLayout {
    fn default() -> Self {
        let mut settings = Map::new();
        settings.insert("duration".into(), json!(10));
        EditorLayout { pages: [vec![Page::default()], vec![Page::default()]], settings }
    }
}

/// The ready-made layouts: (name, [(top, bottom)] per side, one page per pair).
/// Only one preset for now; the editor's preset list shows these names.
pub const PRESETS: [(&str, [&[(&str, &str)]; 2]); 1] =
    [("Classic monitor", [&[("cpu", "mem-bat")], &[("disk", "net")]])];

/// The pages of the preset called `name` (None for an unknown name), with each app's default settings.
/// `first_snapshot(side)` gives the Picture app its first drawing, if a preset uses it.
pub fn preset_pages(name: &str, first_snapshot: &dyn Fn(&str) -> Option<String>) -> Option<[Vec<Page>; 2]> {
    let (_, sides) = PRESETS.iter().find(|(n, _)| *n == name)?;
    // One page per (top, bottom) pair listed for side `i`.
    let make = |i: usize| -> Vec<Page> {
        let side = SIDES[i];
        sides[i]
            .iter()
            .map(|(top, bottom)| Page {
                duration: DEFAULT_DURATION,
                full: None,
                top: Some(catalog::new_app(top, side, first_snapshot(side))),
                bottom: Some(catalog::new_app(bottom, side, first_snapshot(side))),
            })
            .collect()
    };
    Some([make(0), make(1)])
}

/// Python's idea of "true" for a JSON value: false, 0, "", [], {} and null are false. The layout files
/// were written by the Python editor, which used this rule.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Whether a slot shows an app: it exists and isn't marked "display: false" (an empty placeholder).
fn displays(app: Option<&Entry>) -> bool {
    app.is_some_and(|a| a.get("display").map(|d| *d != Value::Bool(false)).unwrap_or(true))
}

/// A whole number from a hand-written file, which may hold 30, 30.0, "30" or true.
fn int_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// An app entry as the editor holds it: without the YAML list marker and placement keys.
fn clean(app: &Entry) -> Entry {
    let mut app = app.clone();
    app.remove("app");
    app.remove("scope");
    app.remove("duration");
    app
}

/// A layout from a parsed file, and warnings about what had to change.
/// Turns the four quarter lists into pages, pairing the top and bottom lists item by item. A whole-panel
/// app (scope: panel) in either list fills the page. Problems that can be repaired (lists of different
/// lengths, different durations on one page) become warnings for the editor to show; nothing fails, and a
/// panel with no apps gets one empty page.
pub fn from_config(config: &Value) -> (EditorLayout, Vec<String>) {
    let mut config = config.as_object().cloned().unwrap_or_default();
    // The four lists are taken out; everything left in `config` is kept as the layout's settings.
    let quads = config.remove("quadrants").and_then(|q| q.as_object().cloned()).unwrap_or_default();
    // The file's own default duration; 0 or missing means the editor's default.
    let default_duration = int_of(config.get("duration")).filter(|d| *d != 0).unwrap_or(DEFAULT_DURATION);
    let mut warnings = Vec::new();
    let mut pages: [Vec<Page>; 2] = [Vec::new(), Vec::new()];
    for (s, side) in SIDES.iter().enumerate() {
        // A quarter's list; an item that isn't an object becomes an empty entry (which won't display).
        let list = |q: String| -> Vec<Entry> {
            quads
                .get(&q)
                .and_then(Value::as_array)
                .map(|l| l.iter().map(|e| e.as_object().cloned().unwrap_or_default()).collect())
                .unwrap_or_default()
        };
        let top = list(format!("top-{side}"));
        let bottom = list(format!("bottom-{side}"));
        if !top.is_empty() && !bottom.is_empty() && top.len() != bottom.len() {
            warnings.push(format!(
                "The {side} panel's top and bottom lists have different lengths, so they rotated independently. The editor has paired them up page by page."
            ));
        }
        for i in 0..top.len().max(bottom.len()) {
            let t = top.get(i);
            let b = bottom.get(i);
            let durations: Vec<i64> =
                [t, b].iter().flatten().map(|a| int_of(a.get("duration")).unwrap_or(default_duration)).collect();
            if durations.len() == 2 && durations[0] != durations[1] {
                warnings.push(format!(
                    "Page {} of the {side} panel had different top and bottom durations; the editor uses {} seconds for both.",
                    i + 1,
                    durations[0]
                ));
            }
            // The top item's duration wins when the two disagree (the warning above says so).
            let mut page = Page::with_duration(durations.first().copied().unwrap_or(default_duration));
            let scope_panel =
                |a: Option<&Entry>| a.and_then(|a| a.get("scope")).and_then(Value::as_str) == Some("panel");
            if displays(t) && scope_panel(t) {
                page.full = t.map(clean);
            // A whole-panel app written in the bottom list (by hand) counts only if the top slot is empty.
            } else if displays(b) && scope_panel(b) && !displays(t) {
                page.full = b.map(clean);
            } else {
                page.top = if displays(t) { t.map(clean) } else { None };
                page.bottom = if displays(b) { b.map(clean) } else { None };
            }
            pages[s].push(page);
        }
        if pages[s].is_empty() {
            pages[s].push(Page::with_duration(default_duration));
        }
    }
    (EditorLayout { pages, settings: config }, warnings)
}

/// Problems that stop the layout being saved.
/// Two rules, both from the Python editor: an app can appear only once in each quarter's list (a
/// whole-panel app counts as a top app), and every page lasts at least one second.
pub fn validate(layout: &EditorLayout) -> Vec<String> {
    let mut problems = Vec::new();
    for (s, side) in SIDES.iter().enumerate() {
        for position in ["top", "bottom"] {
            // (app name, first page it is on) for this quarter.
            let mut seen: Vec<(String, usize)> = Vec::new();
            for (i, page) in layout.pages[s].iter().enumerate() {
                // A whole-panel app is written to the top quarter's list
                let app = if position == "top" {
                    page.full.as_ref().or(page.top.as_ref())
                } else if page.full.is_some() {
                    None
                } else {
                    page.bottom.as_ref()
                };
                let Some(app) = app else { continue };
                let name = app.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                if let Some((_, first)) = seen.iter().find(|(n, _)| *n == name) {
                    let place = if position == "top" { "top half (or whole panel)" } else { "bottom half" };
                    problems.push(format!(
                        "{} is used twice in the {side} panel's {place} (pages {first} and {}). Each app can be used once there.",
                        catalog::label(Some(app)),
                        i + 1
                    ));
                } else {
                    seen.push((name, i + 1));
                }
            }
        }
    }
    for (s, side) in SIDES.iter().enumerate() {
        for (i, page) in layout.pages[s].iter().enumerate() {
            if page.duration < 1 {
                problems.push(format!("Page {} of the {side} panel needs a duration of at least 1 second.", i + 1));
            }
        }
    }
    problems
}

/// An empty placeholder slot. The monitor needs every quarter list to have an item for each page, so an
/// empty half is written as an app that doesn't display; its name is unique so it never counts as a repeat.
fn blank(side: &str, position: &str, index: usize, duration: i64) -> Value {
    json!({"app": null, "name": format!("empty-{position}-{side}-{index}"), "duration": duration, "display": false})
}

/// One app as written to the file: "app: null" (the YAML list marker the Python layout used), the name,
/// the duration, "scope: panel" for a whole-panel app, then everything `catalog::finalize` gives it.
fn entry(app: &Entry, side: &str, duration: i64, panel_scope: bool) -> Value {
    let mut finalized = catalog::finalize(app.clone(), side);
    let name = finalized.remove("name").unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("app".into(), Value::Null);
    out.insert("name".into(), name);
    out.insert("duration".into(), json!(duration));
    if panel_scope {
        out.insert("scope".into(), json!("panel"));
    }
    for (k, v) in finalized {
        out.insert(k, v);
    }
    Value::Object(out)
}

/// The file contents for a layout.
/// The opposite of `from_config`: builds the four quarter lists from the pages. Every other key the file
/// had (ports, locations...) is kept as it was; a "duration" is added if missing, as the monitor needs one.
pub fn to_config(layout: &EditorLayout) -> Value {
    let mut config = layout.settings.clone();
    config.entry("duration").or_insert(json!(DEFAULT_DURATION));
    let mut quadrants = Map::new();
    for (s, side) in SIDES.iter().enumerate() {
        let (mut top, mut bottom) = (Vec::new(), Vec::new());
        for (i, page) in layout.pages[s].iter().enumerate() {
            // n: the page number used in blank slots' names; d: the page's duration in both lists.
            let (n, d) = (i + 1, page.duration);
            if let Some(full) = &page.full {
                top.push(entry(full, side, d, true));
                bottom.push(blank(side, "bottom", n, d));
            } else {
                top.push(
                    page.top.as_ref().map(|a| entry(a, side, d, false)).unwrap_or_else(|| blank(side, "top", n, d)),
                );
                bottom.push(
                    page.bottom
                        .as_ref()
                        .map(|a| entry(a, side, d, false))
                        .unwrap_or_else(|| blank(side, "bottom", n, d)),
                );
            }
        }
        quadrants.insert(format!("top-{side}"), Value::Array(top));
        quadrants.insert(format!("bottom-{side}"), Value::Array(bottom));
    }
    config.insert("quadrants".into(), Value::Object(quadrants));
    Value::Object(config)
}

/// The file text: the header comment, then the YAML. A value that can't be written as YAML (it always can)
/// gives just the header.
pub fn dump(config: &Value) -> String {
    HEADER.to_string() + &serde_yaml::to_string(config).unwrap_or_default()
}

/// Reads and parses a layout file. An empty file counts as an empty object; a read or YAML error comes back
/// as text.
pub fn load_file(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(if value.is_null() { json!({}) } else { value })
}

/// Writes the layout (config-local.yaml), keeping the previous file as .bak.
/// Refuses with every problem from `validate`, joined by blank lines. Otherwise writes to a .tmp file and
/// renames it over the real one, so the monitor (which reloads the file when it changes) never reads a
/// half-written file. The previous version is copied to .bak first.
pub fn save(layout: &EditorLayout, path: &Path) -> Result<(), String> {
    let problems = validate(layout);
    if !problems.is_empty() {
        return Err(problems.join("\n\n"));
    }
    let text = dump(&to_config(layout));
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        // config-local.yaml.bak, overwritten each time.
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, &bak).map_err(|e| e.to_string())?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// Ports ----------------------------------------------------------------------------------------------

/// The first Port choice: let the monitor find the panel itself.
pub const AUTO_PORT: &str = "Automatic";

/// The Port box's entries for one panel, as [(label, port, location)], and the index to show now.
/// Picking a panel whose USB location is known saves the location (left-location / right-location),
/// which stays the same when Windows renumbers the COM ports; otherwise the port name is saved.
pub fn port_choices(
    ports: &[(String, Option<String>)],
    port_setting: Option<&str>,
    location_setting: Option<&str>,
) -> (Vec<(String, Option<String>, Option<String>)>, usize) {
    let port_setting = port_setting.filter(|s| !s.is_empty());
    let location_setting = location_setting.filter(|s| !s.is_empty());
    // `current` is the index of the choice matching what the file says; 0 (Automatic) if none does.
    let mut choices: Vec<(String, Option<String>, Option<String>)> = vec![(AUTO_PORT.into(), None, None)];
    let mut current = 0;
    for (port, location) in ports {
        // `location` is the USB location Windows reports (e.g. "1-3.2"), when known.
        let label = match location {
            Some(l) => format!("{port} (USB {l})"),
            None => port.clone(),
        };
        if port_setting.is_some_and(|p| p.to_lowercase() == port.to_lowercase()) {
            // Keep what the file says (the port), so choosing it again changes nothing
            choices.push((label, Some(port.clone()), None));
            current = choices.len() - 1;
        } else {
            choices.push((label, Some(port.clone()), location.clone()));
            if port_setting.is_none()
                // A saved location matches a port whose location starts with it (the saved one may be
                // shorter, without the interface part).
                && location_setting.is_some_and(|ls| location.as_deref().is_some_and(|l| l.starts_with(ls)))
            {
                current = choices.len() - 1;
            }
        }
    }
    // The saved port or location isn't plugged in now: add it as a choice anyway, so the box shows
    // what the file says and saving doesn't silently switch to Automatic.
    if current == 0 {
        if let Some(p) = port_setting {
            choices.push((p.to_string(), Some(p.to_string()), None));
            current = choices.len() - 1;
        } else if let Some(l) = location_setting {
            choices.push((format!("USB {l} (not connected)"), None, Some(l.to_string())));
            current = choices.len() - 1;
        }
    }
    (choices, current)
}

/// Settings that put the two panels found now ([(location, port)], left first) the other way round.
/// Uses USB locations when both panels have one (they survive COM port renumbering), port names otherwise.
/// None with fewer than two panels.
pub fn swapped_pins(found: &[(Option<String>, String)]) -> Option<Map<String, Value>> {
    if found.len() < 2 {
        return None;
    }
    let ((left_location, left_port), (right_location, right_port)) = (&found[0], &found[1]);
    let mut pins = Map::new();
    // Locations may carry an interface suffix after ':' ("1-3.2:x.0"); only the part before it is saved.
    let short = |l: &str| l.split(':').next().unwrap_or(l).to_string();
    if let (Some(l), Some(r)) = (left_location, right_location) {
        pins.insert("left-location".into(), json!(short(r)));
        pins.insert("right-location".into(), json!(short(l)));
    } else {
        pins.insert("left-port".into(), json!(right_port));
        pins.insert("right-port".into(), json!(left_port));
    }
    Some(pins)
}

/// Whether a setting is present and true by Python's rules (see `truthy`).
pub fn is_true(v: Option<&Value>) -> bool {
    v.is_some_and(truthy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout that ships with the app, parsed.
    fn bundled() -> Value {
        serde_yaml::from_str(crate::paths::BUNDLED_LAYOUT).unwrap()
    }

    /// The bundled layout loads without warnings, is valid, survives write -> read unchanged, and what the
    /// editor writes can be read by the monitor's own parser.
    #[test]
    fn round_trips_the_bundled_layout() {
        let (layout, warnings) = from_config(&bundled());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(validate(&layout).is_empty());
        let written = to_config(&layout);
        let (again, _) = from_config(&serde_yaml::from_str(&dump(&written)).unwrap());
        assert_eq!(again, layout);
        // The monitor can read what the editor writes
        wr_core::layout::Layout::parse(&dump(&written)).unwrap();
    }

    /// Whole-panel apps and display: false blanks are read as pages and written back the same way, and lists
    /// of different lengths give a warning.
    #[test]
    fn whole_panel_apps_and_blanks() {
        let config = json!({"duration": 10, "quadrants": {
            "top-left": [{"app": null, "name": "equalizer", "scope": "panel", "duration": 20}],
            "bottom-left": [{"app": null, "name": "empty", "display": false, "duration": 20}],
            "top-right": [{"name": "cpu"}, {"name": "time", "duration": 5}],
            "bottom-right": [{"name": "mem-bat"}],
        }});
        let (layout, warnings) = from_config(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(layout.pages[0][0].full.as_ref().unwrap()["name"], "equalizer");
        assert_eq!(layout.pages[0][0].duration, 20);
        assert_eq!(layout.pages[1].len(), 2);
        assert!(layout.pages[1][1].bottom.is_none());
        let out = to_config(&layout);
        assert_eq!(out["quadrants"]["bottom-right"][1]["display"], json!(false));
        assert_eq!(out["quadrants"]["top-left"][0]["scope"], json!("panel"));
        assert_eq!(out["quadrants"]["top-left"][0]["args"]["side"], json!("left"));
    }

    /// The same app on two pages of one half is reported once, naming both pages.
    #[test]
    fn the_same_app_twice_in_one_half_is_refused() {
        let mut layout = EditorLayout::default();
        let cpu = catalog::new_app("cpu", "left", None);
        layout.pages[0] =
            vec![Page { top: Some(cpu.clone()), ..Page::default() }, Page { top: Some(cpu), ..Page::default() }];
        let problems = validate(&layout);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("pages 1 and 2"));
    }

    /// Port choices match a saved location or port (ignoring case), show a missing location as not connected,
    /// and swapping two panels swaps their short USB locations.
    #[test]
    fn ports_and_swapping() {
        let ports = vec![("COM4".to_string(), Some("1-3.2".to_string())), ("COM5".to_string(), None)];
        let (choices, current) = port_choices(&ports, None, Some("1-3"));
        assert_eq!(choices[current].0, "COM4 (USB 1-3.2)");
        let (choices, current) = port_choices(&ports, Some("com5"), None);
        assert_eq!(choices[current], ("COM5".to_string(), Some("COM5".to_string()), None));
        let (choices, current) = port_choices(&[], None, Some("1-9"));
        assert_eq!(choices[current].0, "USB 1-9 (not connected)");
        let found = vec![(Some("1-3.2:x.0".into()), "COM4".into()), (Some("1-4.2:x.0".into()), "COM5".into())];
        let pins = swapped_pins(&found).unwrap();
        assert_eq!(pins["left-location"], "1-4.2");
        assert_eq!(pins["right-location"], "1-3.2");
    }
}
