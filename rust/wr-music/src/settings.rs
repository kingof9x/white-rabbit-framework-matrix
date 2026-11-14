//! The music visualizer's settings ("music" in settings.json, and the audio visualizer app's args in
//! config.yaml use the same keys), and the list of looks. Mirrors the top of
//! `led_mon/equalizer_files/looks.py`.

use serde_json::{Map, Value};

/// (key, label, description) of every look, in the order the window lists them.
pub const STYLES: &[(&str, &str, &str)] = &[
    ("classic", "Classic", "Bars grow up and down from the middle row, like the original equalizer."),
    ("bars", "Spectrum analyzer", "Bars rise from the bottom of the panel: bass on the left, treble on the right."),
    (
        "centre",
        "Centre out",
        "Bass in the middle, treble at the edges, like a mirror. Across both panels the bass is where the panels meet.",
    ),
    ("dots", "Dots", "One dot where the top of each bar would be."),
    ("waterfall", "Spectrogram", "The spectrum scrolls down the panel like a waterfall; brighter means louder."),
    (
        "meter",
        "Level meter",
        "Loudness meters for the left and right channels, with peak marks. Across both panels each panel is one channel.",
    ),
    ("wave", "Waveform", "The sound wave itself, like an oscilloscope."),
    ("mirror-wave", "Mirror waveform", "The sound wave mirrored around the middle column."),
    ("double-wave", "Double waveform", "Two waves side by side: the left and the right channel."),
    (
        "radial",
        "Radial bars",
        "Bars that point out from a circle in the middle, one direction per band. The left panel is a mirror image of the right one.",
    ),
    (
        "radial-fan",
        "Radial fan",
        "Half a circle on each panel that fans out from the keyboard side: bass points up, treble points down. Together the panels look like one circle split by the keyboard.",
    ),
    (
        "radial-facing",
        "Radial facing",
        "A circle on each panel with the bass bars pointing towards the keyboard and the treble bars pointing outwards, so the two panels face each other.",
    ),
    ("rings", "Rings", "Circles around the middle, bass inside and treble outside; brighter means louder."),
    ("pulse", "Pulse", "A glow in the middle that swells with the beat."),
    ("sparkle", "Sparkle", "LEDs twinkle all over; the louder it gets, the more of them."),
    ("bonfire", "Bonfire", "Flames that grow with the music, bass by the keyboard; sparks fly up on the beat."),
    ("beat-tunnel", "Beat tunnel", "Each beat sends a frame out from the keyboard, like flying down a tunnel."),
    ("fountain", "Fountain", "Every column throws up drops; the louder its band, the higher they go."),
    ("lava-lamp", "Lava lamp", "Soft blobs float up and down, swell with the bass and merge when they meet."),
    (
        "heartbeat",
        "Heartbeat",
        "A heart monitor line running down the panel. The left panel beats with the bass, the right panel with the treble.",
    ),
    ("dancing-rabbit", "Dancing rabbit", "The White Rabbit jumps on the beat, over a small spectrum at the bottom."),
    (
        "water",
        "Water, glow and sparks",
        "The bass pushes ripples through water, the middle notes add a glow that rises and spreads, and the treble throws sparks. The left panel is a mirror image of the right one.",
    ),
];

/// Every look's key.
pub fn style_keys() -> impl Iterator<Item = &'static str> {
    STYLES.iter().map(|(key, _, _)| *key)
}

/// Looks that have no bars, so peak dots and shading don't change them (and both panels hear both channels).
pub const NO_BAR_OPTIONS: &[&str] = &["water"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputMode {
    Playback,
    Microphone,
}

impl InputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            InputMode::Playback => "playback",
            InputMode::Microphone => "microphone",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallSpeed {
    Slow,
    Normal,
    Fast,
}

impl FallSpeed {
    /// Levels (out of 34) the bars drop per second.
    pub fn levels_per_sec(self) -> f64 {
        match self {
            FallSpeed::Slow => 30.0,
            FallSpeed::Normal => 70.0,
            FallSpeed::Fast => 160.0,
        }
    }
}

/// The settings with every key filled in and every value valid (Python's `looks.normalize`).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub input_mode: InputMode,
    /// Microphone name, blank for the default microphone.
    pub input_device: String,
    pub style: String,
    pub span_panels: bool,
    pub peak_dots: bool,
    pub shaded: bool,
    pub flip: bool,
    pub fall_speed: FallSpeed,
    /// 1-10.
    pub sensitivity: i64,
    pub auto_sensitivity: bool,
    /// Older settings from the equalizer app, kept as given when present.
    pub level_gain: Option<Value>,
    pub noise_gate_level: Option<Value>,
    pub silence_level_sum_threshold: Option<Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            input_mode: InputMode::Playback,
            input_device: String::new(),
            style: "classic".into(),
            span_panels: false,
            peak_dots: false,
            shaded: false,
            flip: false,
            fall_speed: FallSpeed::Normal,
            sensitivity: 5,
            auto_sensitivity: true,
            level_gain: None,
            noise_gate_level: None,
            silence_level_sum_threshold: None,
        }
    }
}

/// Python's `str(value)` for the JSON values settings can hold.
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

/// Python's truthiness.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn to_bool(value: Option<&Value>, default: bool) -> bool {
    match value {
        None | Some(Value::Null) => default,
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" | "" => false,
            _ => default,
        },
        Some(v) => truthy(v),
    }
}

/// Python's `float(value)`.
pub(crate) fn to_float(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim().to_lowercase();
            match t.as_str() {
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                _ => t.replace('_', "").parse::<f64>().ok(),
            }
        }
        _ => None,
    }
}

/// Python's `int(value)` (numbers are cut towards zero; strings must be whole numbers).
pub(crate) fn to_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|v| v.is_finite()).map(|v| v.trunc() as i64)),
        Value::String(s) => s.trim().replace('_', "").parse::<i64>().ok(),
        _ => None,
    }
}

pub fn normalize_input_mode(value: Option<&Value>) -> InputMode {
    let mode = match value {
        Some(v) if truthy(v) => text(v).trim().to_lowercase(),
        _ => "playback".into(),
    };
    match mode.as_str() {
        "microphone" | "mic" | "input" | "capture" => InputMode::Microphone,
        _ => InputMode::Playback,
    }
}

/// Settings with every key filled in and every value made valid.
pub fn normalize(raw: &Map<String, Value>) -> Settings {
    let mut s = Settings { input_mode: normalize_input_mode(raw.get("input-mode")), ..Settings::default() };
    if let Some(device) = raw.get("input-device").filter(|v| truthy(v)) {
        s.input_device = text(device).trim().to_string();
    }
    let style = match raw.get("style") {
        Some(v) if truthy(v) => text(v).trim().to_lowercase(),
        _ => "classic".into(),
    };
    if style_keys().any(|k| k == style) {
        s.style = style;
    }
    s.span_panels = to_bool(raw.get("span-panels"), false);
    s.peak_dots = to_bool(raw.get("peak-dots"), false);
    s.shaded = to_bool(raw.get("shaded"), false);
    s.flip = to_bool(raw.get("flip"), false);
    s.auto_sensitivity = to_bool(raw.get("auto-sensitivity"), true);
    let fall = match raw.get("fall-speed") {
        Some(v) if truthy(v) => text(v).trim().to_lowercase(),
        _ => "normal".into(),
    };
    s.fall_speed = match fall.as_str() {
        "slow" => FallSpeed::Slow,
        "fast" => FallSpeed::Fast,
        _ => FallSpeed::Normal,
    };
    s.sensitivity = match raw.get("sensitivity") {
        None => 5,
        Some(v) => {
            to_float(v).filter(|f| f.is_finite()).map(|f| (f.round_ties_even() as i64).clamp(1, 10)).unwrap_or(5)
        }
    };
    let keep = |key: &str| raw.get(key).filter(|v| !v.is_null()).cloned();
    s.level_gain = keep("level-gain");
    s.noise_gate_level = keep("noise-gate-level");
    s.silence_level_sum_threshold = keep("silence-level-sum-threshold");
    s
}

/// Settings from a JSON object (anything else gives the defaults).
pub fn from_json(raw: &Value) -> Settings {
    match raw {
        Value::Object(map) => normalize(map),
        _ => Settings::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fills_in_and_cleans_up() {
        let s = from_json(&json!({}));
        assert_eq!(s, Settings::default());
        let s = from_json(&json!({"style": " Water ", "input-mode": "mic", "sensitivity": "12", "flip": "yes",
                                  "fall-speed": "FAST", "auto-sensitivity": 0, "peak-dots": "maybe"}));
        assert_eq!(s.style, "water");
        assert_eq!(s.input_mode, InputMode::Microphone);
        assert_eq!(s.sensitivity, 10);
        assert!(s.flip && !s.auto_sensitivity && !s.peak_dots);
        assert_eq!(s.fall_speed, FallSpeed::Fast);
        let s = from_json(&json!({"style": "disco", "sensitivity": 2.5, "level-gain": 2}));
        assert_eq!(s.style, "classic");
        assert_eq!(s.sensitivity, 2);
        assert_eq!(s.level_gain, Some(json!(2)));
        assert_eq!(STYLES.len(), 22);
    }
}
