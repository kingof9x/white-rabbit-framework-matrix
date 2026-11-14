//! The layout file (`config-local.yaml`, or `config.yaml`) and which app each quarter of the panels
//! shows when: a port of the rotation in `led_mon/led_system_monitor.py`.
//!
//! Each quarter (top-left, bottom-left, top-right, bottom-right) has a list of apps that take turns,
//! each for its own `duration` in seconds. An app with `scope: panel` takes the whole panel, and the
//! other half of that panel keeps rotating unseen so the two stay in step.

use serde_yaml::{Mapping, Value};
use std::collections::HashMap;

pub const QUADRANTS: [&str; 4] = ["top-left", "bottom-left", "top-right", "bottom-right"];

#[derive(Clone, Debug, PartialEq)]
pub struct App {
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
    pub args: Mapping,
}

impl App {
    pub fn arg(&self, key: &str) -> Option<&Value> {
        self.args.get(Value::String(key.to_string()))
    }

    pub fn arg_bool(&self, key: &str) -> bool {
        self.arg(key).map(yaml_truthy).unwrap_or(false)
    }

    pub fn arg_str(&self, key: &str) -> Option<String> {
        match self.arg(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// `border: false` turns the app's border off.
    pub fn border(&self) -> bool {
        self.arg("border").map(yaml_truthy).unwrap_or(true)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub duration: i64,
    pub quadrants: HashMap<String, Vec<App>>,
    /// Optional `left-port` / `right-port` (e.g. COM4) and `left-location` / `right-location` (USB position).
    pub left_port: Option<String>,
    pub right_port: Option<String>,
    pub left_location: Option<String>,
    pub right_location: Option<String>,
}

#[derive(Debug)]
pub struct LayoutError(pub String);

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LayoutError {}

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

fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn opt_string(map: &Mapping, key: &str) -> Option<String> {
    match map.get(Value::String(key.to_string()))? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Layout {
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

    pub fn apps(&self, quadrant: &str) -> &[App] {
        &self.quadrants[quadrant]
    }
}

/// What one panel shows: one app on the whole panel, or a top and a bottom app.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelApps {
    Whole(App),
    Halves(App, App),
}

impl PanelApps {
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
pub struct Rotation {
    pub layout: Layout,
    index: HashMap<String, usize>,
    /// When each (quarter, app name) was last switched to. Missing = now, as Python's defaultdict.
    since: HashMap<(String, String), f64>,
    pub frozen: bool,
}

/// What changed in one step: whether the left / right panel moved to another app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Changed {
    pub left: bool,
    pub right: bool,
}

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
    pub fn new(layout: Layout) -> Rotation {
        let index = QUADRANTS.iter().map(|q| (q.to_string(), 0)).collect();
        Rotation { layout, index, since: HashMap::new(), frozen: false }
    }

    /// A new layout (the window saved it): start every quarter again from its first app.
    pub fn set_layout(&mut self, layout: Layout) {
        let frozen = self.frozen;
        *self = Rotation::new(layout);
        self.frozen = frozen;
    }

    fn current(&self, q: &str) -> &App {
        &self.layout.apps(q)[self.index[q]]
    }

    fn since(&mut self, q: &str, name: &str, now: f64) -> f64 {
        *self.since.entry((q.to_string(), name.to_string())).or_insert(now)
    }

    /// Moves on the quarters whose app has had its time (or all of them, for "next app"), and
    /// returns which panels changed. Hidden quarters (under a whole-panel app) move on too.
    pub fn step(&mut self, now: f64, next_app: bool) -> Changed {
        let mut changed = Changed::default();
        let hidden = self.hidden_quarters();
        for q in QUADRANTS {
            let app = self.current(q).clone();
            let since = self.since(q, &app.name, now);
            let due = now - since >= app.duration as f64 || next_app;
            if due && !self.frozen {
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

    #[test]
    fn the_bundled_layout_reads() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../led_mon/config.yaml")).unwrap();
        let l = Layout::parse(&text).unwrap();
        assert!(l.apps("top-left").len() >= 2);
    }
}
