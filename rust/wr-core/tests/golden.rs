//! Draws every monitor case from `scripts/golden_frames.py` (monitor_cases) with the Rust code and
//! compares it with the pictures Python saved in `tests/golden/monitor_apps.json`. Every pixel must match.
//!
//! Why: the Rust monitor replaces the Python one, and users should see exactly the same pictures.
//! The Python script drew each app with many fixed inputs and saved every result as 34 rows of hex
//! (two hex digits per LED, 9 LEDs a row). This test draws the same inputs with the Rust functions,
//! encodes them the same way, and compares by case name. A case Python saved but Rust does not draw
//! (or the other way round) is a failure too, so the two lists cannot drift apart unnoticed.
//!
//! To add a case: add it to `golden_frames.py`, run it to regenerate the JSON, then add the same case
//! here with the same name. Names are built from the inputs, written as Python would print them.
//! The test reads files from the repository (the JSON and `led_mon/snapshot_files`), so it is run
//! from the workspace with `cargo test`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use wr_core::apps::now_playing::{demo_cover, draw_half, scene, Media, Options};
use wr_core::apps::panel_names::name_grid;
use wr_core::apps::picture::{draw_frame, Drawing};
use wr_core::apps::text;
use wr_core::apps::weather::{draw_forecast_period, weather_values, Weather};
use wr_core::drawing::*;
use wr_core::patterns::ICONS;
use wr_core::{Grid, HEIGHT, WIDTH};

/// The LED brightness for lit pixels used in every case (the default foreground brightness).
const FOREGROUND: i32 = 160;
/// The LED brightness for borders used in every case (the default background brightness).
const BACKGROUND: i32 = 35;

/// The repository's top folder: this crate is at `rust/wr-core`, so two folders up.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// A float written the way Python's f-strings write it: 1.0, 0.07, 199.6.
/// Used to build case names so they match the names Python wrote.
fn py(v: f64) -> String {
    if v == v.trunc() {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// The grid as Python saved it: one string per row, each LED as two lower-case hex digits,
/// values clamped to 0-255.
fn encode(grid: &Grid) -> Vec<String> {
    let bytes = grid.to_bytes();
    (0..HEIGHT).map(|y| (0..WIDTH).map(|x| format!("{:02x}", bytes[x][y])).collect()).collect()
}

/// Every case, by name. Each case starts from a dark grid and draws one app (and usually its border)
/// the way `golden_frames.py` does. A `BTreeMap` keeps the names sorted, so failures list in order.
fn monitor_cases() -> BTreeMap<String, Grid> {
    let mut cases = BTreeMap::new();
    // Helper: draw one case on a fresh grid and store it under its name.
    let mut case = |name: String, draw: &dyn Fn(&mut Grid)| {
        let mut g = Grid::new();
        draw(&mut g);
        cases.insert(name, g);
    };

    // Inputs shared by several apps: core loads for 8, 16 and 24 cores (0.0-1.0), a usage history
    // with a gap (None) in it, and an fps history.
    let cpu_8 = [0.0, 0.12, 0.25, 0.37, 0.5, 0.63, 0.88, 1.0];
    let cpu_16: Vec<f64> = (0..16).map(|i| i as f64 / 15.0).collect();
    let cpu_24: Vec<f64> = (0..24).map(|i| ((i * 7) % 24) as f64 / 23.0).collect();
    let history = [
        Some(0.9),
        Some(0.75),
        None,
        Some(0.6),
        Some(0.45),
        Some(0.3),
        Some(0.15),
        Some(0.05),
        Some(0.0),
        Some(1.0),
        Some(0.5),
        Some(0.33),
        Some(0.66),
        Some(0.2),
        Some(0.8),
        Some(0.1),
    ];
    let fps_history = [144.0, 140.5, 0.0, 120.0, 61.0, 59.9, 30.0, 75.0, 90.0];

    // The half-panel apps are drawn in both the top half (row 0) and the bottom half (row 16).
    let halves = [("top", 0usize), ("bottom", 16usize)];
    for (half, y) in halves {
        // The cpu app with 8, 16 and 24 cores (more cores means thinner columns per core).
        for (label, values) in [("8", &cpu_8[..]), ("16", &cpu_16[..]), ("24", &cpu_24[..])] {
            case(format!("cpu-{label}-cores-{half}"), &|g| {
                draw_cpu_vals(g, values, FOREGROUND, y);
                draw_app_border("cpu", g, BACKGROUND, y, values.len());
            });
        }
        // Both graphs draw the same history; only their borders differ.
        for app in ["cpu-graph", "gpu-graph"] {
            case(format!("{app}-{half}"), &|g| {
                draw_history_graph(g, &history, FOREGROUND, y);
                draw_app_border(app, g, BACKGROUND, y, 8);
            });
        }
        // The memory app at empty, nearly empty, in-between, nearly full and full.
        for ratio in [0.0, 0.07, 0.33, 0.5, 0.99, 1.0] {
            case(format!("mem-{}-{half}", py(ratio)), &|g| {
                draw_memory(g, ratio, FOREGROUND, y);
                draw_app_border("mem", g, BACKGROUND, y, 8);
            });
        }
        // Two bars each, at several pairs of rates.
        for app in ["disk", "net"] {
            for (a, b) in [(0.0, 0.0), (0.1, 0.9), (0.5, 0.25), (1.0, 0.66)] {
                case(format!("{app}-{}-{}-{half}", py(a), py(b)), &|g| {
                    draw_bar(g, a, FOREGROUND, 1, y);
                    draw_bar(g, b, FOREGROUND, 5, y);
                    draw_app_border(app, g, BACKGROUND, y, 8);
                });
            }
        }
        // The older combined disk and network app.
        for (disk, net) in [((0.0, 0.0), (0.0, 0.0)), ((0.2, 0.8), (1.0, 0.4)), ((0.55, 0.05), (0.3, 0.95))] {
            let name = format!("disk-net-{}-{}-{}-{}-{half}", py(disk.0), py(disk.1), py(net.0), py(net.1));
            case(name, &|g| {
                draw_disk_net(g, disk, net, FOREGROUND, y);
                draw_app_border("disk-net", g, BACKGROUND, y, 8);
            });
        }
        // The fps number on its own: one to four digits, a decimal, and "not measurable".
        for (label, fps, available) in [
            ("0", 0.0, true),
            ("7", 7.0, true),
            ("60", 60.0, true),
            ("144", 144.0, true),
            ("199.6", 199.6, true),
            ("240", 240.0, true),
            ("999", 999.0, true),
            ("1500", 1500.0, true),
            ("60", 60.0, false),
        ] {
            let state = if available { "ok" } else { "unavailable" };
            case(format!("fps-num-{label}-{state}-{half}"), &|g| draw_fps_half(g, fps, available, FOREGROUND, y));
        }
    }

    // Memory plus battery: memory in the half's first rows and the battery 3 rows lower, at
    // several charge levels and moments (the battery flashes and pulses with time).
    for (half, y) in [("top", 3usize), ("bottom", 19usize)] {
        for (ratio, plugged) in [(1.0, false), (0.66, false), (0.33, true), (0.05, false), (0.05, true)] {
            for now in [0.0, 1.0, 2.5, 4.7] {
                let state = if plugged { "plugged" } else { "battery" };
                case(format!("mem-bat-{}-{state}-t{}-{half}", py(ratio), py(now)), &|g| {
                    draw_memory(g, 0.4, FOREGROUND, y - 3);
                    draw_battery(g, ratio, plugged, FOREGROUND, y, now);
                    draw_app_border("mem", g, BACKGROUND, y - 3, 8);
                });
            }
        }
    }

    // The full-panel fps app: number plus histogram, with short, empty and unavailable histories.
    for (label, fps, history, available) in [
        ("144", 144.0, &fps_history[..], true),
        ("61", 61.0, &fps_history[..4], true),
        ("0", 0.0, &[][..], true),
        ("480", 480.0, &[480.0, 470.0, 300.0][..], true),
        ("0", 0.0, &fps_history[..], false),
    ] {
        let state = if available { "ok" } else { "unavailable" };
        case(format!("fps-{label}-{}-{state}", history.len()), &|g| draw_fps(g, fps, history, available, FOREGROUND));
    }

    // Built-in pictures, read from the repository. A missing file is skipped here, which then
    // shows up as "not drawn in Rust" for that name.
    for (panel, file) in [("left", "zigzag"), ("left", "the-way-left"), ("right", "the-way-right")] {
        let path = repo_root().join("led_mon").join("snapshot_files").join(panel).join(format!("{file}.json"));
        if let Ok(text) = std::fs::read_to_string(&path) {
            let rows: Vec<Vec<f64>> = serde_json::from_str(&text).unwrap();
            case(format!("snap-{panel}-{file}"), &|g| draw_snapshot(g, &rows, FOREGROUND));
        }
    }

    // Drawings from the Draw tab, made the same way as DRAWINGS in golden_frames.py
    // `count` frames of made-up values from a formula, including values below 0 and above 255
    // and halves, to check clamping and rounding down.
    let frames = |count: usize, offset: usize| {
        let frames: Vec<Vec<Vec<f64>>> = (0..count)
            .map(|k| {
                (0..HEIGHT)
                    .map(|y| {
                        (0..WIDTH)
                            .map(|x| {
                                let v = ((x * 29 + y * 7 + k * 50 + offset) % 300) as f64 - 20.0;
                                v + if x % 2 == 1 { 0.5 } else { 0.0 }
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        serde_json::json!(frames)
    };
    // Delays: normal, too fast (clamped to 20 ms), missing (default) and given as text.
    let drawings = [
        ("three-frames", serde_json::json!({"version": 1, "delay_ms": 150, "frames": frames(3, 0)})),
        ("fast", serde_json::json!({"version": 1, "delay_ms": 5, "frames": frames(2, 77)})),
        ("no-delay", serde_json::json!({"version": 1, "frames": frames(4, 11)})),
        ("text-delay", serde_json::json!({"version": 1, "delay_ms": "250", "frames": frames(2, 5)})),
        ("empty", serde_json::json!({"version": 1, "delay_ms": 100, "frames": []})),
    ];
    for (name, data) in &drawings {
        let drawing = Drawing::from_json(data).unwrap();
        // Moments just before and at a frame change (0.149 / 0.15 s for a 150 ms delay).
        for t in [0.0, 0.149, 0.15, 0.7, 12.34] {
            case(format!("drawing-{name}-t{}", py(t)), &|g| draw_frame(g, drawing.frame_at(t), FOREGROUND));
        }
    }

    // The Text app
    // Texts with times, percents, punctuation, letters outside the font (degree sign, sharp s)
    // and blank text, in every style at several moments.
    let texts = [
        ("short", "Hi"),
        ("time", "10:47 87% 12%"),
        ("words", "Hello, world! 42"),
        ("long", "The quick brown fox jumps over the lazy dog 0123456789"),
        ("symbols", "a+b=c? (x/y)*2 #1 <3> _ ' \" \u{b0} ~ \u{df}"),
        ("percent", "5% 100%"),
        ("blank", "   "),
    ];
    for style in text::STYLES {
        for (label, words) in texts {
            for t in [0.0, 1.0, 3.3, 7.25, 40.0] {
                case(format!("text-{style}-{label}-t{}", py(t)), &|g| text::draw_text(g, words, style, FOREGROUND, t));
            }
        }
    }
    // Live values filled into a text: 12- and 24-hour clocks, unknown values and rounding edges.
    for (label, cpu, battery, hour, fmt_24) in [
        ("pm", Some(0.123), Some(0.876), 13, false),
        ("24h", None, None, 9, true),
        ("midnight", Some(1.0), Some(0.004), 0, false),
    ] {
        let values = text::TextValues {
            time: text::time_text(hour, 5, fmt_24),
            battery: text::percent_text(battery),
            cpu: text::percent_text(cpu),
        };
        case(format!("text-values-{label}"), &|g| {
            text::draw_with_values(g, "{time} {Battery} {CPU} {other}", "stacked", FOREGROUND, &values, 2.0)
        });
    }

    // Panel names
    // Panel names, including too-long names, spaces, accented letters, symbols and an empty one.
    for name in ["Left", "Right", "Desk", "abcdefgh", "a b", "\u{e4}\u{f6}\u{fc}!", "?\u{e9}#", "", "10:47"] {
        case(format!("name-{name}"), &|g| *g = name_grid(name, FOREGROUND));
    }

    // Columns of characters as the clock and weather apps draw them.
    let char_cases: [(&str, &[&str]); 7] = [
        ("time-1234", &["1", "2", "horiz_colon", "3", "4"]),
        ("time-0905", &["0", "9", "horiz_colon", "0", "5"]),
        ("weather-21c-clouds", &["2", "1", "degC", "clouds"]),
        ("weather-minus-4f-snow", &["-", "4", "degF", "snow"]),
        ("weather-wind-chill", &["wc", " ", "1", "8", "degC"]),
        ("weather-wind", &["wd", "1", "2", "km", "wind-sw"]),
        ("weather-unknown", &["?", "?"]),
    ];
    for (name, chars) in char_cases {
        case(name.to_string(), &|g| draw_chars_list(g, chars, FOREGROUND, 1));
    }
    // Every icon under a temperature; the PM dot is not a stand-alone icon.
    for (icon, _) in ICONS {
        if *icon != "pm_indicator" {
            case(format!("weather-icon-{icon}"), &|g| draw_chars_list(g, &["2", "0", "degC", icon], FOREGROUND, 1));
        }
    }

    // The temp app: three numbers, including a missing one, 0, and three-digit values.
    for (half, y) in halves {
        for (label, values) in [
            ("45-62.4-38", [Some(45.0), Some(62.4), Some(38.0)]),
            ("100-7-none", [Some(100.0), Some(7.0), None]),
            ("199-0-150.6", [Some(199.0), Some(0.0), Some(150.6)]),
            ("none-none-none", [None, None, None]),
        ] {
            case(format!("temp-{label}-{half}"), &|g| draw_temp_numbers(g, &values, FOREGROUND, y));
        }
    }

    // The fan app: two bars at several speeds.
    for (half, y) in halves {
        for (a, b) in [(0.0, 0.0), (0.25, 1.0), (0.5, 0.1)] {
            case(format!("fan-{}-{}-{half}", py(a), py(b)), &|g| {
                draw_bar(g, a, FOREGROUND, 1, y);
                draw_bar(g, b, FOREGROUND, 5, y);
                draw_app_border("fan", g, BACKGROUND, y, 8);
            });
        }
    }

    // Builds a `Weather` reading from its parts.
    let weather = |temp, chill, speed, speed_symbol: &str, dir, temp_symbol: &str, condition: &str| Weather {
        temp,
        wind_chill: chill,
        wind_speed: speed,
        wind_speed_symbol: speed_symbol.into(),
        wind_dir: dir,
        temp_symbol: temp_symbol.into(),
        condition: condition.into(),
    };
    let readings = [
        ("mild", weather(21.4, 19.5, 12.5, "km", 225.0, "degC", "Clouds")),
        ("cold", weather(-4.5, -11.6, 30.2, "mi", 350.0, "degF", "snow")),
        ("hot", weather(101.0, 99.5, 3.0, "km", 92.0, "degK", "clear")),
    ];
    // Every measure (and an unknown one) for three readings, in both halves.
    for (label, w) in &readings {
        for measure in ["temp_condition", "wind_chill", "wind", "bogus"] {
            for (half, y) in halves {
                case(format!("weather-{label}-{measure}-{half}"), &|g| {
                    let values = weather_values(w, measure);
                    let chars: Vec<&str> = values.iter().map(String::as_str).collect();
                    draw_chars_list(g, &chars, FOREGROUND, y);
                });
            }
        }
    }
    // Forecast bars, including a day past 5 (clamped) and a negative hour.
    for (day, hour) in [(1, 12), (3, 0), (5, 23), (9, -1)] {
        case(format!("weather-forecast-d{day}-h{hour}"), &|g| {
            let values = weather_values(&readings[0].1, "temp_condition");
            let chars: Vec<&str> = values.iter().map(String::as_str).collect();
            draw_chars_list(g, &chars, FOREGROUND, 0);
            draw_forecast_period(g, FOREGROUND, day, hour);
        });
    }

    // Visualizer levels including over-tall (7.4) and negative ones, to check clamping.
    let levels = [7.4, 6.5, 5.5, 4.2, 3.5, 2.5, 1.0, 0.4, -1.0];
    // The now-playing scene with a full song name, a short one and none, with all options on and
    // all off, at several moments, as seen by each panel.
    let song =
        |title: &str, artist: &str| Media { title: title.into(), artist: artist.into(), playing: true, cover: None };
    let songs = [("full", song("Preview song", "White Rabbit")), ("title", song("ok?", "")), ("none", song("", ""))];
    let all = Options { title: true, visualizer: true, drift: true, when_idle: "skip".into() };
    let plain = Options { title: false, visualizer: false, drift: false, when_idle: "skip".into() };
    let cover = demo_cover();
    for (label, media) in &songs {
        for (opt, options) in [("all", &all), ("plain", &plain)] {
            for now in [0.0, 2.3, 7.9, 61.25] {
                for side in ["left", "right"] {
                    case(format!("now-playing-{label}-{opt}-t{}-{side}", py(now)), &|g| {
                        let canvas = scene(media, Some(&cover), now, options, Some(&levels));
                        draw_half(g, &canvas, side == "right", FOREGROUND);
                    });
                }
            }
        }
    }

    // The "Identify panels" pictures: every pair of half-panel app names, and the whole-panel ones.
    let halves_ids = ["cpu", "cpu-graph", "disk", "disk-net", "fan", "gpu-graph", "mem-bat", "net", "temp"];
    for (i, top) in halves_ids.iter().enumerate() {
        let bottom = halves_ids[(i + 3) % halves_ids.len()];
        case(format!("ids-{top}-{bottom}"), &|g| draw_ids(g, top, bottom, FOREGROUND));
    }
    for name in ["fps", "snap"] {
        case(format!("id-{name}"), &|g| draw_id(g, name, FOREGROUND));
    }

    // Borders on their own, and the outline used around whole-panel apps.
    for app in ["cpu", "cpu-graph", "disk-net", "disk", "mem"] {
        for (half, y) in halves {
            case(format!("border-{app}-{half}"), &|g| draw_app_border(app, g, BACKGROUND, y, 8));
        }
    }
    case("border-outline".to_string(), &|g| draw_outline_border(g, BACKGROUND));
    cases
}

/// Every case drawn in Rust matches Python's saved picture pixel for pixel, and both sides have the
/// same set of case names. All problems are collected and listed together, each with the first row
/// that differs, so one run shows everything that is wrong.
#[test]
fn monitor_apps_match_python() {
    let path = repo_root().join("tests").join("golden").join("monitor_apps.json");
    let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // "frames" maps each case name to its 34 hex rows.
    let saved: BTreeMap<String, Vec<String>> = serde_json::from_value(data["frames"].clone()).unwrap();
    let drawn = monitor_cases();

    let mut problems = Vec::new();
    // Names Python saved that Rust did not draw count as problems too.
    for name in saved.keys().filter(|n| !drawn.contains_key(*n)) {
        problems.push(format!("not drawn in Rust: {name}"));
    }
    for (name, grid) in &drawn {
        match saved.get(name) {
            None => problems.push(format!("not in the saved frames: {name}")),
            Some(rows) => {
                // Compare the hex rows; on a mismatch report the first row that differs.
                let ours = encode(grid);
                if &ours != rows {
                    let first = (0..HEIGHT).find(|&y| ours[y] != rows[y]).unwrap();
                    problems
                        .push(format!("differs: {name} (row {first}: python {} rust {})", rows[first], ours[first]));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}
