//! What the layout editor knows about each app: a friendly name, whether it fills half a panel or the
//! whole panel, and the settings it offers (`led_mon/editor/catalog.py`). Apps from plugins not listed
//! here still work, with no settings form.
//!
//! This file is mostly data: the `APPS` table, built at compile time with the small `const fn` helpers
//! below. The editor (`editor.rs`) reads it to fill the "add an app" list and to build each app's settings
//! form; `new_app` and `finalize` make the layout-file entry for a placed app. Nothing here touches the
//! window, threads or files; it is pure data and functions, used on the UI thread.
//!
//! How an app looks in the layout file the monitor reads: an object with "name" (the app key), sometimes
//! flags on the entry itself ("animate", "persistent-draw"...), and its settings under "args".

use serde_json::{json, Map, Value};

/// How much of a panel an app takes: the top or bottom half (17 rows), or all 34 rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Half,
    Full,
}

/// What kind of control a setting gets in the editor's form, and how its value is stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A switch; true or false.
    Bool,
    /// A drop-down of fixed options (`Setting::options`).
    Choice,
    /// A text box; a string.
    Text,
    /// A number box; a whole number.
    Int,
    /// Comma-separated, stored as a list
    List,
    /// Several of the options, stored as a list
    Multi,
    /// A drawing or built-in picture
    Snapshot,
    /// A drop-down of the graphics cards the monitor reports.
    Gpu,
    /// A drop-down of the hardware sensors the monitor reports (from Libre Hardware Monitor).
    Sensor,
    /// Where sensor readings come from (a setting for the whole monitor, see `Target::Global`).
    SensorSource,
    /// A drop-down of the computer's microphones.
    Microphone,
    /// How the weather app finds where the laptop is.
    LocationSource,
    /// A game for the FPS apps
    Process,
}

/// Where a setting's value is stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// In the app's args
    Args,
    /// On the app entry itself (e.g. animate)
    App,
    /// In settings.json, for the whole app
    Global,
}

/// One setting in an app's form. Values are kept as JSON text so the table can be a compile-time constant.
#[derive(Clone, Debug)]
pub struct Setting {
    /// The key in the layout file (under "args", on the entry, or in settings.json; see `target`).
    pub key: &'static str,
    /// The label in the form.
    pub label: &'static str,
    /// The kind of control.
    pub kind: Kind,
    /// JSON text of the default
    pub default: &'static str,
    /// JSON values of the options (a choice or multi)
    pub options: &'static [&'static str],
    /// What to show for each option, if not the option itself
    pub labels: &'static [&'static str],
    /// Where the value is stored.
    pub target: Target,
    /// A sentence under the control; empty for none.
    pub help: &'static str,
}

impl Setting {
    /// The default as a JSON value; null if the text isn't valid JSON (a test checks they all are).
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).unwrap_or(Value::Null)
    }

    /// The options as JSON values, in order.
    pub fn option_values(&self) -> Vec<Value> {
        self.options.iter().map(|o| serde_json::from_str(o).unwrap_or(Value::Null)).collect()
    }

    /// The text shown for option `i`: its label if there is one, otherwise the value itself (a string without
    /// quotes). Out of range gives an empty string.
    pub fn option_label(&self, i: usize) -> String {
        if let Some(l) = self.labels.get(i) {
            return l.to_string();
        }
        match self.option_values().get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        }
    }
}

/// A plain setting stored in "args", with no options and no help text.
const fn s(key: &'static str, label: &'static str, kind: Kind, default: &'static str) -> Setting {
    Setting { key, label, kind, default, options: &[], labels: &[], target: Target::Args, help: "" }
}

/// A drop-down setting stored in "args".
const fn choice(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    options: &'static [&'static str],
    labels: &'static [&'static str],
) -> Setting {
    Setting { key, label, kind: Kind::Choice, default, options, labels, target: Target::Args, help: "" }
}

/// The same setting with a help sentence added.
const fn help(mut setting: Setting, help: &'static str) -> Setting {
    setting.help = help;
    setting
}

/// The same setting stored somewhere other than "args".
const fn target(mut setting: Setting, target: Target) -> Setting {
    setting.target = target;
    setting
}

/// One app the editor knows.
pub struct AppInfo {
    /// The app's name in the layout file, as the monitor knows it ("cpu", "clock"...).
    pub key: &'static str,
    /// The friendly name shown in the editor.
    pub label: &'static str,
    /// What the app shows, for the editor's help text.
    pub help: &'static str,
    /// Whether it takes half a panel or the whole panel.
    pub size: Size,
    /// The settings in the form, in order.
    pub settings: &'static [Setting],
    /// Fixed parts of the config entry the app needs to run (JSON object text)
    pub fixed: &'static str,
    /// Fixed "args" the app needs (JSON object text), added whatever the user sets.
    pub fixed_args: &'static str,
}

/// The game an FPS app watches; empty means "automatic".
const GAME: Setting = help(
    s("process", "Game to watch", Kind::Process, "\"\""),
    "Automatic follows the full-screen game in front and keeps it while you switch to other windows. Or pick a game from the list; games drawing frames right now are at the top.",
);
/// Shared by the CPU and GPU graphs: which edge the bars grow from.
const GRAPH_BARS_FROM: Setting =
    choice("bars_from", "Bars grow from", "\"left\"", &["\"left\"", "\"right\""], &["The left edge", "The right edge"]);
/// Shared by the CPU and GPU graphs: which end the newest second is drawn at.
const GRAPH_NEWEST: Setting = choice(
    "newest",
    "Newest second",
    "\"top\"",
    &["\"top\"", "\"bottom\""],
    &["At the top (scrolls down)", "At the bottom (scrolls up)"],
);
/// Where sensor readings come from; stored in settings.json for every app at once (`Target::Global`).
const SOURCE: Setting = target(s("sensor_source", "Read from", Kind::SensorSource, "null"), Target::Global);

/// The audio visualizer's looks as JSON strings, in the order of `LOOK_LABELS` (and of wr-music's styles).
pub const LOOK_KEYS: [&str; 22] = [
    "\"classic\"",
    "\"bars\"",
    "\"centre\"",
    "\"dots\"",
    "\"waterfall\"",
    "\"meter\"",
    "\"wave\"",
    "\"mirror-wave\"",
    "\"double-wave\"",
    "\"radial\"",
    "\"radial-fan\"",
    "\"radial-facing\"",
    "\"rings\"",
    "\"pulse\"",
    "\"sparkle\"",
    "\"bonfire\"",
    "\"beat-tunnel\"",
    "\"fountain\"",
    "\"lava-lamp\"",
    "\"heartbeat\"",
    "\"dancing-rabbit\"",
    "\"water\"",
];
/// The looks' names as shown in the editor, one per entry of `LOOK_KEYS`.
pub const LOOK_LABELS: [&str; 22] = [
    "Classic",
    "Spectrum analyzer",
    "Centre out",
    "Dots",
    "Spectrogram",
    "Level meter",
    "Waveform",
    "Mirror waveform",
    "Double waveform",
    "Radial bars",
    "Radial fan",
    "Radial facing",
    "Rings",
    "Pulse",
    "Sparkle",
    "Bonfire",
    "Beat tunnel",
    "Fountain",
    "Lava lamp",
    "Heartbeat",
    "Dancing rabbit",
    "Water, glow and sparks",
];
/// The battery app's animations (JSON strings), one per entry of `BATTERY_LOOK_LABELS`.
const BATTERY_LOOKS: [&str; 6] = ["\"bolt\"", "\"sweep\"", "\"bubbles\"", "\"pour\"", "\"wave\"", "\"orbit\""];
const BATTERY_LOOK_LABELS: [&str; 6] = [
    "Bolt (pulsing bolt / down arrow)",
    "Sweep (light climbs / dark band falls)",
    "Bubbles (rise / sink)",
    "Pour (drops fall in / float out)",
    "Wave (rolls right / rolls left)",
    "Orbit (spark runs up / runs down)",
];
/// A speed setting's options and their labels.
const SPEEDS: [&str; 3] = ["\"slow\"", "\"normal\"", "\"fast\""];
const SPEED_LABELS: [&str; 3] = ["Slow", "Normal", "Fast"];

/// Every app the editor knows, in the order the "add an app" list shows them. The last two are special:
/// "none" is an empty half and "plugin" stands for any app the table doesn't know (`get` falls back to
/// it). Each row is one long line on purpose, so the table reads as a list of apps.
pub static APPS: [AppInfo; 22] = [
    AppInfo { key: "cpu", label: "CPU cores", help: "Load on each CPU core, drawn as small spirals (or a bar per core when there are more than eight).", size: Size::Half,
        settings: &[help(s("afterglow", "Afterglow", Kind::Bool, "true"), "A dim trail shows where each core's load just was, fading over a couple of seconds")], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "cpu-graph", label: "CPU graph", help: "Overall CPU use over the last seconds, with a memory (RAM) meter above it. Each row is one second; a longer row is a busier CPU.", size: Size::Half,
        settings: &[help(s("memory", "Memory meter on top", Kind::Bool, "true"), "How much memory (RAM) is in use, as two rows above the graph"), GRAPH_BARS_FROM, GRAPH_NEWEST], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "gpu-graph", label: "GPU graph", help: "Overall GPU use over the last seconds, like the CPU graph, with the GPU's memory meter above it.", size: Size::Half,
        settings: &[help(s("gpu", "GPU", Kind::Gpu, "null"), "Which graphics card to show, when the computer has more than one"), help(s("memory", "GPU memory meter on top", Kind::Bool, "true"), "How much of the graphics card's memory is in use, as two rows above the graph"), GRAPH_BARS_FROM, GRAPH_NEWEST], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "disk-net", label: "Network + disk", help: "Network upload and download bars on top, disk read and write bars under them (every disk in the computer), and how full a drive is at the bottom.", size: Size::Half,
        settings: &[help(s("drive", "Drive space meter", Kind::Text, "\"C:\""), "The drive whose space shows at the bottom, such as C: or D:. Blank hides the meter")], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "battery", label: "Battery", help: "The battery level in a battery shape, with a moving look that shows whether it is charging or running on battery. Flashes when nearly empty.", size: Size::Half,
        settings: &[help(choice("look", "Look", "\"bolt\"", &BATTERY_LOOKS, &BATTERY_LOOK_LABELS), "Each look moves one way while charging and the other way on battery")], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "mem-bat", label: "Memory + battery", help: "Memory use on top, battery level below (pulses when charging).", size: Size::Half, settings: &[], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "battery-sand", label: "Battery (sand)", help: "The battery level as a pile of sand. Charging rains sand in from the top; on battery the pile slowly drains away. Flashes when nearly empty.", size: Size::Half,
        settings: &[choice("sand_speed", "Sand speed", "\"normal\"", &SPEEDS, &SPEED_LABELS)], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "disk", label: "Disk", help: "Disk read (left bar) and write (right bar) activity.", size: Size::Half, settings: &[], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "net", label: "Network", help: "Upload (left bar) and download (right bar) activity.", size: Size::Half, settings: &[], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "temp", label: "Temperatures", help: "Three temperatures as numbers, one under the other (the middle one dimmer). Pick which sensor each one shows, and Celsius or Fahrenheit.", size: Size::Half,
        settings: &[
            help(s("sensor_1", "Top number", Kind::Sensor, "\"\""), "Blank picks the next sensor automatically (CPU first)."),
            s("sensor_2", "Middle number", Kind::Sensor, "\"\""),
            s("sensor_3", "Bottom number", Kind::Sensor, "\"\""),
            help(choice("unit", "Unit", "\"C\"", &["\"C\"", "\"F\""], &[]), "C: Celsius, F: Fahrenheit"),
            SOURCE,
        ], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "fan", label: "Fans", help: "Speed of the two fans as bars.", size: Size::Half, settings: &[SOURCE], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "time", label: "Clock", help: "The current time.", size: Size::Full,
        settings: &[
            s("fmt_24_hour", "24-hour clock", Kind::Bool, "false"),
            help(s("timezone", "Time zone", Kind::Text, "\"\""), "Blank for this computer's time zone, or an IANA name such as Europe/London"),
        ], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "weather", label: "Weather", help: "Current weather or a forecast, from your location.", size: Size::Full,
        settings: &[
            choice("units", "Units", "\"metric\"", &["\"metric\"", "\"imperial\"", "\"standard\""], &[]),
            Setting { key: "location_source", label: "Location", kind: Kind::LocationSource, default: "\"manual\"", options: &["\"manual\"", "\"windows\""], labels: &["Typed in below", "Windows Location"], target: Target::Args, help: "" },
            help(s("zip_info", "Zip / postcode, country", Kind::List, "[]"), "For example: 10001, US. If this and the coordinates are blank, your location is worked out from your internet address"),
            help(s("lat_lon", "Latitude, longitude", Kind::List, "[]"), "Used instead of the zip code if set"),
            Setting { key: "measures", label: "Show", kind: Kind::Multi, default: "[\"temp_condition\", \"wind_chill\", \"wind\"]", options: &["\"temp_condition\"", "\"wind_chill\"", "\"wind\""], labels: &[], target: Target::Args, help: "" },
            s("measures-duration", "Seconds per measure", Kind::Int, "20"),
            s("forecast", "Show a forecast", Kind::Bool, "false"),
            choice("forecast_day", "Forecast day (1-5)", "1", &["1", "2", "3", "4", "5"], &[]),
            choice("forecast_hour", "Forecast hour", "12", &["0", "3", "6", "9", "12", "15", "18", "21"], &[]),
        ], fixed: "{}", fixed_args: "{\"id_key_override\": [\"forecast\", \"weather_forecast\", \"weather_current\"]}" },
    // Still called "equalizer" in config files, so existing layouts keep working
    AppInfo { key: "equalizer", label: "Audio visualizer", help: "Moves with the music: what the laptop plays, or what a microphone hears. The Audio page shows a preview of each look.", size: Size::Full,
        settings: &[
            choice("input-mode", "Listen to", "\"playback\"", &["\"playback\"", "\"microphone\""], &["What the laptop is playing", "A microphone"]),
            help(s("input-device", "Microphone", Kind::Microphone, "\"\""), "Used when listening to a microphone"),
            choice("style", "Look", "\"classic\"", &LOOK_KEYS, &LOOK_LABELS),
            help(s("span-panels", "One spectrum across both panels (18 bars)", Kind::Bool, "false"), "Turn this on for the audio visualizer on both panels: bass on the left, treble on the right"),
            help(s("peak-dots", "Peak dots", Kind::Bool, "false"), "A dot above each bar that falls slowly"),
            help(s("shaded", "Shaded", Kind::Bool, "false"), "Bars get brighter towards their tips"),
            s("flip", "Upside down", Kind::Bool, "false"),
            choice("fall-speed", "Falling speed", "\"normal\"", &SPEEDS, &SPEED_LABELS),
            choice("sensitivity", "Sensitivity (1-10)", "5", &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], &[]),
            help(s("auto-sensitivity", "Adjust the sensitivity automatically", Kind::Bool, "true"), "Quiet and loud songs both fill the panel"),
        ], fixed: "{\"persistent-draw\": true, \"dispose-fn\": \"equalizer_dispose\"}", fixed_args: "{\"external-filter\": false, \"border\": false}" },
    AppInfo { key: "now-playing", label: "Now playing", help: "The cover of whatever is playing (Spotify, a browser, the Media Player app), with the song name scrolling along the top and a small visualizer underneath. Put it on both panels: each panel shows half of the cover. Windows only.", size: Size::Full,
        settings: &[
            s("title", "Scroll the song name along the top", Kind::Bool, "true"),
            s("visualizer", "Small visualizer under the cover", Kind::Bool, "true"),
            s("drift", "Let the cover drift slowly", Kind::Bool, "true"),
            help(choice("when_idle", "When nothing is playing", "\"skip\"", &["\"skip\"", "\"last\"", "\"clock\""], &["Show my other apps", "Keep the last cover", "Show the clock"]), "Skip goes straight on to the next page of the layout"),
        ], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "fps", label: "FPS counter", help: "Frames per second of a game on the top half, with a histogram below that scrolls left, one column per second, newest on the right. Windows only.", size: Size::Full, settings: &[GAME], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "fps-num", label: "FPS number", help: "Just the frames per second of a game, as numbers. Windows only.", size: Size::Half, settings: &[GAME], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "animation", label: "Animation", help: "A moving picture on the whole panel: fire, plasma, a DNA helix, snowfall, chevrons or a bouncing ball.", size: Size::Full,
        settings: &[help(choice("look", "Look", "\"fire\"", &["\"fire\"", "\"plasma\"", "\"dna\"", "\"snow\"", "\"chevrons\"", "\"ball\""], &["Fire", "Plasma", "DNA helix", "Snowfall", "Chevrons", "Bouncing ball"]),
            "Fire: Flames flickering up from the bottom of the panel. Plasma: Soft blobs of light drifting and merging. DNA helix: Two strands twisting around each other. Snowfall: Snowflakes drifting down and settling at the bottom. Chevrons: Arrow shapes marching down the panel. Bouncing ball: A ball bouncing around the panel with a short tail.")],
        fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "text", label: "Text", help: "Your own words on the whole panel. Type {time}, {battery} or {cpu} to show the clock, battery level or CPU use, kept up to date.", size: Size::Full,
        settings: &[
            help(s("text", "Text", Kind::Text, "\"{time} {battery} {cpu}\""), "For example: {time} {battery} {cpu}  or  Hello {time}"),
            choice("style", "Style", "\"ticker\"", &["\"ticker\"", "\"sideways\"", "\"stacked\""], &["Ticker (scrolls up)", "Sideways (read with your head tilted right)", "Stacked (two letters per row)"]),
            help(s("fmt_24_hour", "24-hour clock", Kind::Bool, "false"), "For {time}"),
        ], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "snap", label: "Picture", help: "One of your drawings from the Draw tab (animated ones play), or a built-in pattern.", size: Size::Full,
        settings: &[
            s("file", "Picture", Kind::Snapshot, "null"),
            help(target(s("animate", "Scroll", Kind::Bool, "false"), Target::App), "Scroll the picture up the panel"),
        ], fixed: "{}", fixed_args: "{\"path\": \"snapshot_files\"}" },
    AppInfo { key: "none", label: "(empty)", help: "Nothing: the panel half stays dark.", size: Size::Half, settings: &[], fixed: "{}", fixed_args: "{}" },
    AppInfo { key: "plugin", label: "App from a plugin", help: "App from a plugin.", size: Size::Half, settings: &[], fixed: "{}", fixed_args: "{}" },
];

/// The apps to offer in the editor, in order.
/// Leaves out "none" and "plugin", which aren't apps the user can pick.
pub fn offered() -> impl Iterator<Item = &'static AppInfo> {
    APPS.iter().filter(|a| a.key != "none" && a.key != "plugin")
}

/// The app with this key; an unknown key (a plugin's app) gives the "plugin" entry, with no settings.
pub fn get(key: &str) -> &'static AppInfo {
    APPS.iter().find(|a| a.key == key).unwrap_or(&APPS[APPS.len() - 1])
}

/// The name to show for a layout entry: the friendly name for a known app, the raw name for a plugin's,
/// "(empty)" when there is no entry or it has no name.
pub fn label(entry: Option<&Map<String, Value>>) -> String {
    match entry.and_then(|e| e.get("name")).and_then(Value::as_str) {
        None => "(empty)".into(),
        Some(name) if APPS.iter().any(|a| a.key == name) => get(name).label.into(),
        Some(name) => name.into(),
    }
}

/// JSON object text to a map; anything else gives an empty map.
fn object(text: &str) -> Map<String, Value> {
    serde_json::from_str(text).unwrap_or_default()
}

/// Adds what the app needs to run on this panel (fixed keys, the panel side).
/// Called on every entry before it is saved, so hand-edited or old entries get what they need too.
/// The fixed keys always win over what the entry had.
pub fn finalize(mut entry: Map<String, Value>, side: &str) -> Map<String, Value> {
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let info = get(&name);
    for (k, v) in object(info.fixed) {
        entry.insert(k, v);
    }
    let fixed_args = object(info.fixed_args);
    // These apps need to know which panel they are on: the equalizer and now-playing pick the left or
    // right sound channel ("side"), and the Picture app shows its half of a two-panel drawing ("panel").
    if !fixed_args.is_empty() || name == "equalizer" || name == "now-playing" || name == "snap" {
        let args = entry.entry("args").or_insert_with(|| json!({}));
        // A broken "args" (not an object) is replaced, so the inserts below can't fail.
        if !args.is_object() {
            *args = json!({});
        }
        let args = args.as_object_mut().unwrap();
        for (k, v) in fixed_args {
            args.insert(k, v);
        }
        if name == "equalizer" || name == "now-playing" {
            args.insert("side".into(), json!(side));
        }
        if name == "snap" {
            args.insert("panel".into(), json!(side));
        }
    }
    entry
}

/// A config entry for a newly placed app, with default settings. first_snapshot: the Picture app's
/// first choice.
/// Settings stored in settings.json (`Target::Global`) are skipped: they aren't part of the entry.
pub fn new_app(name: &str, side: &str, first_snapshot: Option<String>) -> Map<String, Value> {
    let info = get(name);
    let mut entry = Map::new();
    entry.insert("name".into(), json!(name));
    let mut args = Map::new();
    for st in info.settings {
        if st.target == Target::Global {
            continue;
        }
        let value = if st.kind == Kind::Snapshot {
            first_snapshot.clone().map(Value::String).unwrap_or(Value::Null)
        } else {
            st.default_value()
        };
        // Empty values of these kinds mean "automatic / not chosen", which is what a missing key means
        // too, so they are left out to keep the layout file as Python wrote it.
        let empty = matches!(&value, Value::Null) || value == json!("") || value == json!([]);
        if empty
            && matches!(
                st.kind,
                Kind::Text | Kind::Process | Kind::List | Kind::Snapshot | Kind::Gpu | Kind::Sensor | Kind::Microphone
            )
        {
            continue;
        }
        if st.target == Target::App {
            entry.insert(st.key.into(), value);
        } else {
            args.insert(st.key.into(), value);
        }
    }
    // An entry with nothing in "args" has no "args" key at all (as the Python version wrote it).
    if !args.is_empty() || info.fixed_args != "{}" {
        entry.insert("args".into(), Value::Object(args));
    }
    finalize(entry, side)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of JSON text in the table parses, so `default_value` and `option_values` never fall back
    /// to null by mistake.
    #[test]
    fn every_default_and_option_is_json() {
        for app in APPS.iter() {
            assert!(serde_json::from_str::<Value>(app.fixed).is_ok(), "{}", app.key);
            assert!(serde_json::from_str::<Value>(app.fixed_args).is_ok(), "{}", app.key);
            for st in app.settings {
                assert!(serde_json::from_str::<Value>(st.default).is_ok(), "{} {}", app.key, st.key);
                for o in st.options {
                    assert!(serde_json::from_str::<Value>(o).is_ok(), "{} {} {o}", app.key, st.key);
                }
            }
        }
    }

    /// New entries look exactly like the Python editor's: fixed keys, the panel side, default args, and
    /// empty settings left out.
    #[test]
    fn new_apps_match_python() {
        // catalog.new_app("equalizer", "left") in Python
        let e = new_app("equalizer", "left", None);
        assert_eq!(e["persistent-draw"], json!(true));
        assert_eq!(e["dispose-fn"], json!("equalizer_dispose"));
        assert_eq!(e["args"]["side"], json!("left"));
        assert_eq!(e["args"]["style"], json!("classic"));
        assert_eq!(e["args"]["border"], json!(false));
        assert!(e["args"].get("input-device").is_none());
        let w = new_app("weather", "right", None);
        assert_eq!(w["args"]["id_key_override"], json!(["forecast", "weather_forecast", "weather_current"]));
        assert!(w["args"].get("zip_info").is_none());
        let p = new_app("snap", "right", Some("drawing:Cat".into()));
        assert_eq!(p["animate"], json!(false));
        assert_eq!(p["args"]["file"], json!("drawing:Cat"));
        assert_eq!(p["args"]["panel"], json!("right"));
        // The CPU graph now has options (not in Python): bars from the left, newest at the top
        let c = new_app("cpu-graph", "left", None);
        assert_eq!(c["args"]["bars_from"], json!("left"));
        assert_eq!(c["args"]["newest"], json!("top"));
    }
}
