//! The layout file (`config-local.yaml`, or `config.yaml`) and which app each quarter of the panels
//! shows when: a port of the rotation in `led_mon/led_system_monitor.py`.
//!
//! Each quarter (top-left, bottom-left, top-right, bottom-right) has a list of apps that take turns,
//! each for its own `duration` in seconds. An app with `scope: panel` takes the whole panel, and the
//! other half of that panel keeps rotating unseen so the two stay in step.
//!
//! Where it sits: `wr-monitor/src/monitor.rs` reads the file (falling back to the bundled layout when
//! it is missing or broken), builds one `Rotation`, and calls `Rotation::step` and `Rotation::panel` on
//! every pass of its main loop (about ten times a second). When the window saves a new layout the monitor
//! notices the file changed and calls `set_layout`. Nothing here reads files or clocks itself: the caller
//! passes the text and the time (seconds since the monitor started), which keeps it easy to test.
//!
//! The file format, as the Python version wrote it:
//!
//! ```yaml
//! duration: 10            # default seconds per app
//! left-location: 1-4.2    # optional: USB position of the left panel (also right-, and -port: COM4)
//! quadrants:
//!   top-left:
//!   - app:                # the "app:" key itself is ignored; the keys below belong to the same item
//!     name: time
//!     duration: 5
//!     scope: panel        # whole panel
//!     display: true       # false: keeps its turn but shows nothing
//!     animate: false      # the panel scrolls the picture itself
//!     args: {border: false}
//! ```

use serde_yaml::{Mapping, Value};
use std::collections::HashMap;

/// The four quarter names as they appear in the file; all four must be present.
pub const QUADRANTS: [&str; 4] = ["top-left", "bottom-left", "top-right", "bottom-right"];

/// One entry in a quarter's list: which app to draw, for how long, and its options.
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    /// The app's id, for example "cpu", "time" or "weather_current"; `render.rs` picks the drawing by it.
    pub name: String,
    /// Seconds before the next app in this quarter; the layout's `duration` when missing.
    pub duration: i64,
    /// `scope: panel` takes the whole panel.
    pub whole_panel: bool,
    /// `display: false` keeps the place in the rotation but shows nothing.
    pub display: bool,
    /// The panel scrolls the picture down by itself.
    pub animate: bool,
    /// Draws to the panel on its own (the audio visualizer).
    pub persistent_draw: bool,
    /// The app's own options (`args:` in the file), read with `arg`, `arg_bool` and `arg_str`. Empty when
    /// missing or not a mapping.
    pub args: Mapping,
}

impl App {
    /// The raw value of option `key`, or `None` when it is not set.
    pub fn arg(&self, key: &str) -> Option<&Value> {
        self.args.get(Value::String(key.to_string()))
    }

    /// Option `key` read the Python way (`0`, `""`, `false`, `null` are false); `false` when missing.
    pub fn arg_bool(&self, key: &str) -> bool {
        self.arg(key).map(yaml_truthy).unwrap_or(false)
    }

    /// Option `key` as text; numbers are turned into text, other kinds of value give `None`.
    pub fn arg_str(&self, key: &str) -> Option<String> {
        match self.arg(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// `border: false` turns the app's border off.
    /// On by default; any false-ish value turns it off.
    pub fn border(&self) -> bool {
        self.arg("border").map(yaml_truthy).unwrap_or(true)
    }
}

/// The whole layout file, checked and read into Rust types by `Layout::parse`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Default seconds per app, for apps without their own `duration`.
    pub duration: i64,
    /// Each of the four `QUADRANTS` names to its list of apps; `parse` guarantees all four are present and
    /// none is empty.
    pub quadrants: HashMap<String, Vec<App>>,
    /// Optional `left-port` / `right-port` (e.g. COM4) and `left-location` / `right-location` (USB position).
    pub left_port: Option<String>,
    pub right_port: Option<String>,
    pub left_location: Option<String>,
    pub right_location: Option<String>,
}

/// Why a layout file could not be used, as a sentence for the log.
#[derive(Debug)]
pub struct LayoutError(pub String);

impl std::fmt::Display for LayoutError {
    /// Shows just the message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LayoutError {}

/// Python's `bool(value)` for YAML values, so the file means the same as it did to the Python app.
/// A tagged value (`!tag x`) is judged by its inner value.
fn yaml_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Sequence(s) => !s.is_empty(),
        Value::Mapping(m) => !m.is_empty(),
        Value::Tagged(t) => yaml_truthy(&t.value),
    }
}

/// A whole number from an integer, a float (truncated: `7.9` -> 7) or numeric text; otherwise `None`.
fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// The text of an optional top-level key such as `left-port`; empty text counts as missing, and a
/// number is accepted as text.
fn opt_string(map: &Mapping, key: &str) -> Option<String> {
    match map.get(Value::String(key.to_string()))? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Layout {
    /// Reads the layout from the YAML text of the file.
    ///
    /// Errors (with a message for the log) when the text is not YAML, `duration` is missing, any of the four
    /// quarters is missing or empty, or an app has no `name`. Unknown keys are ignored. Per app, missing
    /// keys get their defaults: the layout's duration, half panel, displayed, not animated.
    pub fn parse(text: &str) -> Result<Layout, LayoutError> {
        let root: Value = serde_yaml::from_str(text).map_err(|e| LayoutError(format!("Can't read the layout: {e}")))?;
        let map = root.as_mapping().ok_or_else(|| LayoutError("The layout file is empty".into()))?;
        let duration = map
            .get(Value::String("duration".into()))
            .and_then(as_int)
            .ok_or_else(|| LayoutError("The layout has no duration".into()))?;
        let quads = map
            .get(Value::String("quadrants".into()))
            .and_then(Value::as_mapping)
            .ok_or_else(|| LayoutError("The layout has no quadrants".into()))?;
        let mut quadrants = HashMap::new();
        for q in QUADRANTS {
            let list = quads
                .get(Value::String(q.into()))
                .and_then(Value::as_sequence)
                .ok_or_else(|| LayoutError(format!("The layout has no {q} quadrant")))?;
            let mut apps = Vec::new();
            // Each list item is a mapping such as {app: null, name: time, duration: 5}.
            for entry in list {
                let m =
                    entry.as_mapping().ok_or_else(|| LayoutError(format!("An app in {q} isn't a list of settings")))?;
                let get = |k: &str| m.get(Value::String(k.into()));
                let name = match get("name") {
                    Some(Value::String(s)) => s.clone(),
                    _ => return Err(LayoutError(format!("An app in {q} has no name"))),
                };
                apps.push(App {
                    name,
                    duration: get("duration").and_then(as_int).unwrap_or(duration),
                    whole_panel: matches!(get("scope"), Some(Value::String(s)) if s == "panel"),
                    display: get("display").map(yaml_truthy).unwrap_or(true),
                    animate: get("animate").map(yaml_truthy).unwrap_or(false),
                    persistent_draw: get("persistent-draw").map(yaml_truthy).unwrap_or(false),
                    args: get("args").and_then(Value::as_mapping).cloned().unwrap_or_default(),
                });
            }
            if apps.is_empty() {
                return Err(LayoutError(format!("The {q} quadrant has no apps")));
            }
            quadrants.insert(q.to_string(), apps);
        }
        Ok(Layout {
            duration,
            quadrants,
            left_port: opt_string(map, "left-port"),
            right_port: opt_string(map, "right-port"),
            left_location: opt_string(map, "left-location"),
            right_location: opt_string(map, "right-location"),
        })
    }

    /// The apps of quarter `quadrant` (one of `QUADRANTS`). Panics on any other name.
    pub fn apps(&self, quadrant: &str) -> &[App] {
        &self.quadrants[quadrant]
    }
}

/// What one panel shows: one app on the whole panel, or a top and a bottom app.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelApps {
    /// One app drawn over the whole 9 x 34 panel.
    Whole(App),
    /// The top-half app and the bottom-half app.
    Halves(App, App),
}

impl PanelApps {
    /// The apps shown, top first.
    pub fn apps(&self) -> Vec<&App> {
        match self {
            PanelApps::Whole(a) => vec![a],
            PanelApps::Halves(t, b) => vec![t, b],
        }
    }

    /// Whether any shown app wants the panel's own scrolling.
    pub fn animate(&self) -> bool {
        self.apps().iter().any(|a| a.animate)
    }
}

/// Which app each quarter shows now, and when to move on.
///
/// Owned by the monitor's main loop (not shared between threads); the hotkeys reach it through flags the
/// loop copies in (`frozen`, and `next_app` passed to `step`).
pub struct Rotation {
    /// The layout being shown.
    pub layout: Layout,
    /// For each quarter, the position in its app list of the app showing now.
    index: HashMap<String, usize>,
    /// When each (quarter, app name) was last switched to. Missing = now, as Python's defaultdict.
    since: HashMap<(String, String), f64>,
    /// While true nothing moves on, not even "next app" (Ctrl+Alt+Shift+F / U).
    pub frozen: bool,
}

/// What changed in one step: whether the left / right panel moved to another app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Changed {
    /// The left panel now shows a different app (or apps).
    pub left: bool,
    /// The right panel now shows a different app (or apps).
    pub right: bool,
}

/// Which half of a panel owns the whole panel: `Some(0)` the top, `Some(1)` the bottom, `None` neither
/// (both halves show). An app that is hidden (`display: false`) cannot take the panel. If both halves
/// want the whole panel, the top one wins.
fn owner(top: &App, bottom: &App) -> Option<usize> {
    let t = top.display && top.whole_panel;
    let b = bottom.display && bottom.whole_panel;
    if t {
        Some(0)
    } else if b {
        Some(1)
    } else {
        None
    }
}

impl Rotation {
    /// Starts every quarter at its first app. Timers start at the first `step`.
    pub fn new(layout: Layout) -> Rotation {
        let index = QUADRANTS.iter().map(|q| (q.to_string(), 0)).collect();
        Rotation { layout, index, since: HashMap::new(), frozen: false }
    }

    /// A new layout (the window saved it): start every quarter again from its first app.
    pub fn set_layout(&mut self, layout: Layout) {
        // Keep the user's freeze across a layout change.
        let frozen = self.frozen;
        *self = Rotation::new(layout);
        self.frozen = frozen;
    }

    /// The app quarter `q` shows now.
    fn current(&self, q: &str) -> &App {
        &self.layout.apps(q)[self.index[q]]
    }

    /// When the app `name` in quarter `q` started showing; the first time it is asked, that is `now`.
    fn since(&mut self, q: &str, name: &str, now: f64) -> f64 {
        *self.since.entry((q.to_string(), name.to_string())).or_insert(now)
    }

    /// Moves on the quarters whose app has had its time (or all of them, for "next app"), and
    /// returns which panels changed. Hidden quarters (under a whole-panel app) move on too.
    pub fn step(&mut self, now: f64, next_app: bool) -> Changed {
        let mut changed = Changed::default();
        // Worked out before anything moves, so a change is judged by what was on screen.
        let hidden = self.hidden_quarters();
        for q in QUADRANTS {
            let app = self.current(q).clone();
            let since = self.since(q, &app.name, now);
            // While frozen the timers keep their start times, so when unfrozen every app that is past
            // its time moves on at the next step.
            let due = now - since >= app.duration as f64 || next_app;
            if due && !self.frozen {
                // A change under a whole-panel app is not visible, so it does not count as a change
                // for the panel (on a change the monitor re-sends the panel's scrolling on/off flag,
                // the `Option<bool>` of `queue::Item::Frame`).
                if !hidden.contains(&q) {
                    if q.contains("left") {
                        changed.left = true;
                    } else {
                        changed.right = true;
                    }
                }
                let n = self.layout.apps(q).len();
                let i = (self.index[q] + 1) % n;
                self.index.insert(q.to_string(), i);
                let name = self.layout.apps(q)[i].name.clone();
                self.since.insert((q.to_string(), name), now);
            }
        }
        changed
    }

    /// The quarters that are covered by a whole-panel app in the other half of their panel.
    fn hidden_quarters(&self) -> Vec<&'static str> {
        let mut hidden = Vec::new();
        for (top, bottom) in [("top-left", "bottom-left"), ("top-right", "bottom-right")] {
            match owner(self.current(top), self.current(bottom)) {
                Some(0) => hidden.push(bottom),
                Some(_) => hidden.push(top),
                None => {}
            }
        }
        hidden
    }

    /// What the left (`right == false`) or right panel shows now.
    ///
    /// A hidden app (`display: false`) still comes back here; `render.rs` draws nothing for it.
    pub fn panel(&self, right: bool) -> PanelApps {
        let (top, bottom) = if right { ("top-right", "bottom-right") } else { ("top-left", "bottom-left") };
        let (t, b) = (self.current(top), self.current(bottom));
        match owner(t, b) {
            Some(0) => PanelApps::Whole(t.clone()),
            Some(_) => PanelApps::Whole(b.clone()),
            None => PanelApps::Halves(t.clone(), b.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small layout covering the options: per-app duration, whole panel, args and a hidden app.
    const LAYOUT: &str = r#"
duration: 10
left-location: 1-4.2
quadrants:
  top-left:
  - app:
    name: time
    duration: 5
    scope: panel
  - app:
    name: cpu
  bottom-left:
  - app:
    name: mem-bat
    duration: 3
  - app:
    name: disk
    duration: 30
  top-right:
  - app:
    name: cpu-graph
    args:
      border: false
  bottom-right:
  - app:
    name: noop
    display: false
"#;

    /// Proves the file is read with defaults filled in (duration, border, display) and that a file without
    /// quadrants is rejected.
    #[test]
    fn reads_the_layout() {
        let l = Layout::parse(LAYOUT).unwrap();
        assert_eq!(l.duration, 10);
        assert_eq!(l.left_location.as_deref(), Some("1-4.2"));
        assert_eq!(l.apps("top-left")[0].duration, 5);
        assert_eq!(l.apps("top-left")[1].duration, 10);
        assert!(l.apps("top-left")[0].whole_panel);
        assert!(!l.apps("top-right")[0].border());
        assert!(!l.apps("bottom-right")[0].display);
        assert!(Layout::parse("duration: 5").is_err());
    }

    /// Proves apps take turns by their own durations, that a hidden half rotates without counting as a
    /// change, and that freezing stops even "next app".
    #[test]
    fn apps_take_turns_and_whole_panel_apps_hide_the_other_half() {
        let mut r = Rotation::new(Layout::parse(LAYOUT).unwrap());
        r.step(0.0, false);
        assert!(matches!(r.panel(false), PanelApps::Whole(ref a) if a.name == "time"));
        assert!(matches!(r.panel(true), PanelApps::Halves(ref t, _) if t.name == "cpu-graph"));
        // The hidden bottom-left still rotates, but doesn't count as a change on the panel
        let c = r.step(3.0, false);
        assert!(!c.left);
        let c = r.step(5.0, false);
        assert!(c.left);
        assert!(matches!(r.panel(false), PanelApps::Halves(ref t, ref b) if t.name == "cpu" && b.name == "disk"));
        // Frozen: nothing moves, not even "next app"
        r.frozen = true;
        assert_eq!(r.step(100.0, true), Changed::default());
        r.frozen = false;
        let c = r.step(100.5, true);
        assert!(c.left);
        assert!(matches!(r.panel(false), PanelApps::Whole(ref a) if a.name == "time"));
    }

    /// Proves the layout shipped with the app (`led_mon/config.yaml`) parses.
    #[test]
    fn the_bundled_layout_reads() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../led_mon/config.yaml")).unwrap();
        let l = Layout::parse(&text).unwrap();
        assert!(l.apps("top-left").len() >= 2);
    }
}
