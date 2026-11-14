//! Draws every monitor case from `scripts/golden_frames.py` (monitor_cases) with the Rust code and
//! compares it with the pictures Python saved in `tests/golden/monitor_apps.json`. Every pixel must match.

use std::collections::BTreeMap;
use std::path::PathBuf;

use wr_core::drawing::*;
use wr_core::patterns::ICONS;
use wr_core::{Grid, HEIGHT, WIDTH};

const FOREGROUND: i32 = 160;
const BACKGROUND: i32 = 35;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// A float written the way Python's f-strings write it: 1.0, 0.07, 199.6.
fn py(v: f64) -> String {
    if v == v.trunc() {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn encode(grid: &Grid) -> Vec<String> {
    let bytes = grid.to_bytes();
    (0..HEIGHT).map(|y| (0..WIDTH).map(|x| format!("{:02x}", bytes[x][y])).collect()).collect()
}

fn monitor_cases() -> BTreeMap<String, Grid> {
    let mut cases = BTreeMap::new();
    let mut case = |name: String, draw: &dyn Fn(&mut Grid)| {
        let mut g = Grid::new();
        draw(&mut g);
        cases.insert(name, g);
    };

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

    let halves = [("top", 0usize), ("bottom", 16usize)];
    for (half, y) in halves {
        for (label, values) in [("8", &cpu_8[..]), ("16", &cpu_16[..]), ("24", &cpu_24[..])] {
            case(format!("cpu-{label}-cores-{half}"), &|g| {
                draw_cpu_vals(g, values, FOREGROUND, y);
                draw_app_border("cpu", g, BACKGROUND, y, values.len());
            });
        }
        for app in ["cpu-graph", "gpu-graph"] {
            case(format!("{app}-{half}"), &|g| {
                draw_history_graph(g, &history, FOREGROUND, y);
                draw_app_border(app, g, BACKGROUND, y, 8);
            });
        }
        for ratio in [0.0, 0.07, 0.33, 0.5, 0.99, 1.0] {
            case(format!("mem-{}-{half}", py(ratio)), &|g| {
                draw_memory(g, ratio, FOREGROUND, y);
                draw_app_border("mem", g, BACKGROUND, y, 8);
            });
        }
        for app in ["disk", "net"] {
            for (a, b) in [(0.0, 0.0), (0.1, 0.9), (0.5, 0.25), (1.0, 0.66)] {
                case(format!("{app}-{}-{}-{half}", py(a), py(b)), &|g| {
                    draw_bar(g, a, FOREGROUND, 1, y);
                    draw_bar(g, b, FOREGROUND, 5, y);
                    draw_app_border(app, g, BACKGROUND, y, 8);
                });
            }
        }
        for (disk, net) in [((0.0, 0.0), (0.0, 0.0)), ((0.2, 0.8), (1.0, 0.4)), ((0.55, 0.05), (0.3, 0.95))] {
            let name = format!("disk-net-{}-{}-{}-{}-{half}", py(disk.0), py(disk.1), py(net.0), py(net.1));
            case(name, &|g| {
                draw_disk_net(g, disk, net, FOREGROUND, y);
                draw_app_border("disk-net", g, BACKGROUND, y, 8);
            });
        }
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

    for (panel, file) in [("left", "zigzag"), ("left", "the-way-left"), ("right", "the-way-right")] {
        let path = repo_root().join("led_mon").join("snapshot_files").join(panel).join(format!("{file}.json"));
        if let Ok(text) = std::fs::read_to_string(&path) {
            let rows: Vec<Vec<f64>> = serde_json::from_str(&text).unwrap();
            case(format!("snap-{panel}-{file}"), &|g| draw_snapshot(g, &rows, FOREGROUND));
        }
    }

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
    for (icon, _) in ICONS {
        if *icon != "pm_indicator" {
            case(format!("weather-icon-{icon}"), &|g| draw_chars_list(g, &["2", "0", "degC", icon], FOREGROUND, 1));
        }
    }

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

    let halves_ids = ["cpu", "cpu-graph", "disk", "disk-net", "fan", "gpu-graph", "mem-bat", "net", "temp"];
    for (i, top) in halves_ids.iter().enumerate() {
        let bottom = halves_ids[(i + 3) % halves_ids.len()];
        case(format!("ids-{top}-{bottom}"), &|g| draw_ids(g, top, bottom, FOREGROUND));
    }
    for name in ["fps", "snap"] {
        case(format!("id-{name}"), &|g| draw_id(g, name, FOREGROUND));
    }

    for app in ["cpu", "cpu-graph", "disk-net", "disk", "mem"] {
        for (half, y) in halves {
            case(format!("border-{app}-{half}"), &|g| draw_app_border(app, g, BACKGROUND, y, 8));
        }
    }
    case("border-outline".to_string(), &|g| draw_outline_border(g, BACKGROUND));
    cases
}

#[test]
fn monitor_apps_match_python() {
    let path = repo_root().join("tests").join("golden").join("monitor_apps.json");
    let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let saved: BTreeMap<String, Vec<String>> = serde_json::from_value(data["frames"].clone()).unwrap();
    let drawn = monitor_cases();

    let mut problems = Vec::new();
    for name in saved.keys().filter(|n| !drawn.contains_key(*n)) {
        problems.push(format!("not drawn in Rust: {name}"));
    }
    for (name, grid) in &drawn {
        match saved.get(name) {
            None => problems.push(format!("not in the saved frames: {name}")),
            Some(rows) => {
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
