//! Fuzz tests: the files and data the monitor reads from outside (the layout file, settings.json,
//! weather replies, Framework Control and framework_tool output, GPU counter names) are fed thousands
//! of broken, odd and random versions. None may crash the monitor.
//!
//! Most important is the layout: the window writes it, a person can edit it by hand, and every app in
//! it is drawn ten times a second on the monitor's main-loop thread. A panic there would stop that
//! thread and freeze the panels, so every layout that parses must also draw without panicking, with
//! any app and any setting values.
//!
//! The readings given to the apps are odd too: empty lists, values out of range, missing values.
//!
//! The generator is a small seeded xorshift, so a failure prints a seed and the input, and repeats.
//! `WR_FUZZ_CASES=200000 cargo test -p wr-core --test fuzz` runs longer than the CI default.

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use wr_core::apps::{CONTENT_APPS, DATA_APPS};
use wr_core::effects::{parse_position, EffectsSettings};
use wr_core::layout::{App, Layout, Rotation};
use wr_core::render::{brightness_values, render_panel_on, Readings, SUPPORTED_APPS};
use wr_core::settings::Settings;
use wr_core::Grid;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn cases(default: u64) -> u64 {
    std::env::var("WR_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// Odd numbers and strings an app setting might hold.
fn odd_scalar(rng: &mut Rng) -> String {
    rng.pick(&[
        "0",
        "-1",
        "1",
        "2",
        "-999999999",
        "99999999999999999999",
        "1e308",
        "-1e308",
        ".nan",
        ".inf",
        "-.inf",
        "0.5",
        "''",
        "'x'",
        "'?'",
        "'🐇 é'",
        "true",
        "false",
        "null",
        "[]",
        "{}",
        "[1, 2, 3]",
        "'C:'",
        "'Z:'",
        "'left'",
        "'right'",
        "'both'",
        "'not a look'",
        "'bars'",
        "'radial'",
        "'auto'",
        "'metric'",
        "'imperial'",
        "'00:00'",
        "'25:61'",
        "'%s%s%n'",
        "'../../../../etc/passwd'",
        "'\\\\?\\C:\\nul'",
    ])
    .to_string()
}

/// The keys apps read from their settings (grep of `arg*("...")` in `src/apps` and `render.rs`), plus
/// some they don't.
const ARG_KEYS: [&str; 30] = [
    "bars_from",
    "border",
    "drift",
    "drive",
    "file",
    "forecast",
    "forecast_day",
    "forecast_hour",
    "gpu",
    "look",
    "measures",
    "measures-duration",
    "memory",
    "newest",
    "panel",
    "path",
    "process",
    "sand_speed",
    "side",
    "style",
    "text",
    "title",
    "visualizer",
    "when_idle",
    "units",
    "speed",
    "zip_info",
    "lat",
    "lon",
    "unknown-key",
];

fn random_app(rng: &mut Rng) -> String {
    let names: Vec<&str> = SUPPORTED_APPS
        .iter()
        .chain(CONTENT_APPS.iter())
        .chain(DATA_APPS.iter())
        .copied()
        .chain(["equalizer", "no-such-app", ""])
        .collect();
    let mut app = format!("  - app:\n    name: '{}'\n", rng.pick(&names));
    if rng.below(3) == 0 {
        app += &format!("    duration: {}\n", odd_scalar(rng));
    }
    for (key, value) in [("scope", "panel"), ("display", "false"), ("animate", "true"), ("persistent-draw", "true")] {
        if rng.below(4) == 0 {
            app += &format!("    {key}: {value}\n");
        }
    }
    if rng.below(4) != 0 {
        app += "    args:\n";
        for _ in 0..rng.below(6) {
            app += &format!("      {}: {}\n", rng.pick(&ARG_KEYS), odd_scalar(rng));
        }
    }
    app
}

fn random_layout(rng: &mut Rng) -> String {
    let mut text = format!("duration: {}\n", rng.pick(&["10", "0", "-5", "1", "3600", "0.1"]));
    if rng.below(3) == 0 {
        text += "left-location: 1-3.3\nright-port: COM9\n";
    }
    text += "quadrants:\n";
    for q in ["top-left", "bottom-left", "top-right", "bottom-right"] {
        text += &format!("  {q}:\n");
        for _ in 0..1 + rng.below(3) {
            text += &random_app(rng);
        }
    }
    text
}

/// Breaks a text: flips, drops, repeats or cuts bytes, keeping it valid UTF-8.
fn mutate(rng: &mut Rng, text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for _ in 0..1 + rng.below(8) {
        if chars.is_empty() {
            break;
        }
        let at = rng.below(chars.len() as u64) as usize;
        match rng.below(5) {
            0 => {
                chars.remove(at);
            }
            1 => chars.insert(at, *rng.pick(&[':', '-', ' ', '\n', '{', '[', '\'', '"', '\u{0}', 'é', '9'])),
            2 => chars[at] = *rng.pick(&['x', '0', ':', '\t', '#', '&', '*', '!']),
            3 => chars.truncate(at),
            _ => {
                let end = (at + rng.below(40) as usize).min(chars.len());
                let piece: Vec<char> = chars[at..end].to_vec();
                for (i, c) in piece.into_iter().enumerate() {
                    chars.insert(end + i, c);
                }
            }
        }
    }
    chars.into_iter().collect()
}

/// Readings with odd values: empty lists, NaN, values out of range.
struct OddReadings(Rng);
impl Readings for OddReadings {
    fn cpu_cores(&mut self) -> Vec<f64> {
        let n = self.0.below(40) as usize;
        (0..n).map(|_| odd_f64(&mut self.0)).collect()
    }
    fn cpu_history(&mut self) -> Vec<Option<f64>> {
        let n = self.0.below(100) as usize;
        (0..n).map(|_| (self.0.below(4) != 0).then(|| odd_f64(&mut self.0))).collect()
    }
    fn memory(&mut self) -> f64 {
        odd_f64(&mut self.0)
    }
    fn battery(&mut self) -> Option<(f64, bool)> {
        (self.0.below(3) != 0).then(|| (odd_f64(&mut self.0), self.0.below(2) == 0))
    }
    fn clock(&mut self, _: &App) -> (String, bool) {
        (self.0.pick(&["0905", "", "12345", "ab:c", "9"]).to_string(), self.0.below(2) == 0)
    }
    fn now(&mut self) -> f64 {
        odd_f64(&mut self.0).abs() * 1000.0
    }
    fn disk(&mut self) -> (f64, f64) {
        (odd_f64(&mut self.0), odd_f64(&mut self.0))
    }
    fn net(&mut self) -> (f64, f64) {
        (odd_f64(&mut self.0), odd_f64(&mut self.0))
    }
    fn gpu_history(&mut self, _: &str) -> Vec<Option<f64>> {
        self.cpu_history()
    }
    fn fans(&mut self) -> Vec<f64> {
        let n = self.0.below(5) as usize;
        (0..n).map(|_| odd_f64(&mut self.0) * 10_000.0).collect()
    }
    fn temperatures(&mut self, _: &App) -> [Option<f64>; 3] {
        [0, 1, 2].map(|_| (self.0.below(3) != 0).then(|| odd_f64(&mut self.0) * 200.0))
    }
    fn gpu_memory(&mut self, _: &str) -> Option<f64> {
        (self.0.below(2) == 0).then(|| odd_f64(&mut self.0))
    }
    fn drive_space(&mut self, _: &str) -> Option<f64> {
        (self.0.below(2) == 0).then(|| odd_f64(&mut self.0))
    }
}

/// Readings are finite (the Windows readers never give NaN or infinity), but may be out of range:
/// a little below 0 or well above 1 (a counter that wrapped, a rate above the highest seen).
fn odd_f64(rng: &mut Rng) -> f64 {
    match rng.below(10) {
        0 => -0.001,
        1 => 1.2,
        2 => -1.0,
        3 => 7.5,
        4 => 0.0,
        5 => 1.0,
        _ => rng.below(1001) as f64 / 1000.0,
    }
}

/// Parses `text` and, if it is a layout, runs the main loop's drawing over it for a while.
fn parse_and_draw(text: &str, seed: u64) {
    let Ok(layout) = Layout::parse(text) else { return };
    let mut rotation = Rotation::new(layout);
    let mut readings = OddReadings(Rng(seed | 1));
    for pass in 0..12u64 {
        let changed = rotation.step(pass as f64 * 0.7, pass % 5 == 4);
        let _ = changed;
        let (fg, bg) = brightness_values((pass % 11) as f64 / 10.0);
        for side in 0..2 {
            let apps = rotation.panel(side == 1);
            let mut grid = Grid::new();
            readings.set_panel(side == 1);
            render_panel_on(&mut grid, &apps, &mut readings, fg, bg, ["left", "right"][side]);
        }
    }
}

fn check(what: &str, seed: u64, input: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        panic!("{what} panicked (seed {seed}) on this input:\n{input}");
    }
}

#[test]
fn random_layouts_parse_and_draw_without_panicking() {
    let bundled = include_str!("../../../led_mon/config.yaml");
    check("the bundled layout", 0, bundled, || parse_and_draw(bundled, 1));
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for case in 0..cases(1500) {
        let seed = rng.next();
        let mut r = Rng(seed | 1);
        let text = if case % 2 == 0 { random_layout(&mut r) } else { mutate(&mut r, bundled) };
        check("the layout", seed, &text, || parse_and_draw(&text, seed));
    }
}

/// A random JSON value, up to `depth` levels deep.
fn random_json(rng: &mut Rng, depth: u32) -> Value {
    match rng.below(if depth == 0 { 6 } else { 8 }) {
        0 => Value::Null,
        1 => json!(rng.below(2) == 0),
        2 => json!(rng.next() as i64),
        3 => json!(*rng.pick(&[0.0, -1.0, 1e308, -1e-308, 0.5, 100.0, 255.0, 256.0])),
        4 => json!(*rng.pick(&["", "x", "left-back", "right-front", "0", "1e3", "🐇", "true"])),
        5 => json!(rng.below(1000) as f64 / 7.0),
        6 => Value::Array((0..rng.below(5)).map(|_| random_json(rng, depth - 1)).collect()),
        _ => {
            let keys = [
                "follow_screen_brightness",
                "brightness_level",
                "turn_off_with_lid",
                "turn_off_when_screen_unused",
                "tv_effect",
                "shortcuts",
                "effects",
                "learned_ports",
                "music",
                "style",
                "i",
                "n",
                "m",
                "f",
                "u",
                "plug_usb",
                "text_battery",
                "full_shades",
                "current",
                "daily",
                "hourly",
                "weather",
                "main",
                "temp",
                "list",
                "time",
                "temperature_2m",
                "Temperatures",
                "Fans",
                "Value",
                "Name",
                "Children",
                "Text",
            ];
            Value::Object(
                (0..rng.below(8)).map(|_| (rng.pick(&keys).to_string(), random_json(rng, depth - 1))).collect(),
            )
        }
    }
}

#[test]
fn random_settings_and_replies_never_panic() {
    let mut rng = Rng(0x0F0F_1234_ABCD_0001);
    let sample = r#"{"follow_screen_brightness": false, "brightness_level": 42, "shortcuts": {"m": false},
        "effects": {"plug_usb": true, "learned_ports": {"K": "left-back"}}, "music": {"style": "bars"}}"#;
    for _ in 0..cases(3000) {
        let seed = rng.next();
        let mut r = Rng(seed | 1);
        let value = random_json(&mut r, 4);
        let text = if r.below(2) == 0 { value.to_string() } else { mutate(&mut r, sample) };
        check("settings.json", seed, &text, || {
            let _ = Settings::from_json(&text);
        });
        check("the effects settings", seed, &value.to_string(), || {
            let _ = EffectsSettings::from_json(&value);
            let _ = EffectsSettings::from_json(&json!({ "learned_ports": value.clone() }));
        });
        check("the weather reply", seed, &value.to_string(), || {
            for units in ["metric", "imperial", ""] {
                for forecast in [None, Some(("daily", 1)), Some(("hourly", 3)), Some(("daily", -1)), Some(("x", 999))] {
                    let _ = wr_core::apps::weather::parse_open_meteo(&value, units, forecast);
                    let _ = wr_core::apps::weather::parse_openweather(&value, units, forecast);
                }
            }
        });
        check("the Framework Control reply", seed, &value.to_string(), || {
            let _ = wr_core::apps::temp_fan::parse_framework_control(&value);
        });
        let line = mutate(&mut r, "CPU temp: 45 C\nFan Speed:  2000 RPM\nGPU0 instance pid_1_luid_0x0_0x1A_phys_0");
        check("framework_tool / GPU counter text", seed, &line, || {
            let _ = wr_core::apps::temp_fan::parse_framework_tool(&line);
            let _ = wr_core::apps::gpu::parse_instance(&line);
            let _ = wr_core::apps::gpu::parse_adapter_luid(&line);
            let _ = parse_position(&line);
        });
    }
}
