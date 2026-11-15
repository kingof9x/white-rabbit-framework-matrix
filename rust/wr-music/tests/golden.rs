//! Plays every music case from `scripts/golden_frames.py` (music_cases) through the Rust looks and
//! compares the pictures with the ones Python saved in `tests/golden/music_looks.json`. Brightness is
//! stored as round(value * 255) and may differ by 1.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use wr_music::demo::demo_samples;
use wr_music::visualizer::{HEIGHT, WIDTH};
use wr_music::{from_json, to_leds, Frame, MusicPanel, Visualizer, STYLES};

const FRAMES: usize = 48;
const SAVE_EVERY: usize = 6;
const DT: f64 = 0.05;

fn golden(file: &str) -> BTreeMap<String, Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden").join(file);
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    serde_json::from_value(data["frames"].clone()).unwrap()
}

fn decode(rows: &[String]) -> [[i64; HEIGHT]; WIDTH] {
    let mut out = [[0; HEIGHT]; WIDTH];
    for (y, row) in rows.iter().enumerate() {
        for (x, column) in out.iter_mut().enumerate() {
            column[y] = i64::from_str_radix(&row[2 * x..2 * x + 2], 16).unwrap();
        }
    }
    out
}

fn to_255(grid: &Frame) -> [[i64; HEIGHT]; WIDTH] {
    let mut out = [[0; HEIGHT]; WIDTH];
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            out[x][y] = ((grid[x][y] * 255.0).round_ties_even() as i64).clamp(0, 255);
        }
    }
    out
}

/// "name: n LEDs differ (worst x,y: rust vs python)" for frames that differ by more than tolerance.
fn compare(name: &str, got: &[[i64; HEIGHT]; WIDTH], rows: &[String], tolerance: i64) -> Option<String> {
    let want = decode(rows);
    let mut bad = Vec::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            if (got[x][y] - want[x][y]).abs() > tolerance {
                bad.push(format!("({x},{y}) {} vs {}", got[x][y], want[x][y]));
            }
        }
    }
    (!bad.is_empty()).then(|| format!("{name}: {} differ, e.g. {}", bad.len(), bad[..bad.len().min(3)].join(", ")))
}

fn options(name: &str) -> Value {
    match name {
        "plain" => json!({}),
        _ => json!({"peak-dots": true, "shaded": true, "flip": true, "fall-speed": "fast", "sensitivity": 8}),
    }
}

#[test]
fn music_looks_draw_the_golden_frames() {
    let frames = golden("music_looks.json");
    let mut problems = Vec::new();
    let mut checked = 0;
    for (style, _, _) in STYLES {
        for span in [false, true] {
            for option_name in ["plain", "options"] {
                for side in ["left", "right"] {
                    let mut raw = options(option_name);
                    raw["style"] = json!(style);
                    raw["span-panels"] = json!(span);
                    let mut v = Visualizer::new(&from_json(&raw), side == "right", Some(1));
                    let base = format!("{style}-{}-{option_name}-{side}", if span { "span" } else { "single" });
                    let mut t = 1.0;
                    for frame in 1..=FRAMES {
                        let grid = v.frame(&demo_samples(t), DT);
                        t += DT;
                        if frame % SAVE_EVERY == 0 {
                            let name = format!("{base}-f{frame:02}");
                            checked += 1;
                            problems.extend(compare(&name, &to_255(&grid), &frames[&name], 1));
                        }
                    }
                    let silence = vec![[0.0; 2]; wr_music::analyzer::FFT_SIZE];
                    let mut grid = [[0.0; HEIGHT]; WIDTH];
                    for _ in 0..30 {
                        grid = v.frame(&silence, DT);
                    }
                    let name = format!("{base}-silence");
                    checked += 1;
                    problems.extend(compare(&name, &to_255(&grid), &frames[&name], 1));
                }
            }
        }
    }
    let mut ramp = [[0.0; HEIGHT]; WIDTH];
    for (x, column) in ramp.iter_mut().enumerate() {
        for (y, v) in column.iter_mut().enumerate() {
            *v = x as f64 / 8.0 * y as f64 / 33.0;
        }
    }
    for v in ramp[0][..3].iter_mut() {
        *v = 0.015;
    }
    for foreground in [1, 60, 255] {
        let leds = to_leds(&ramp, foreground);
        let got = leds.v.map(|c| c.map(|v| v as i64));
        let name = format!("to-leds-ramp-{foreground}");
        checked += 1;
        problems.extend(compare(&name, &got, &frames[&name], 0));
    }
    assert_eq!(checked, frames.len(), "every golden frame is checked");
    assert!(problems.is_empty(), "{} of {checked} frames differ:\n{}", problems.len(), problems.join("\n"));
}

/// The idle cases from `scripts/golden_frames.py` (music_idle_cases): music, then silence long
/// enough for the calm idle version, then music again.
#[test]
fn music_idle_draws_the_golden_frames() {
    let frames = golden("music_idle.json");
    let mut problems = Vec::new();
    let mut checked = 0;
    let silence = vec![[0.0; 2]; wr_music::analyzer::FFT_SIZE];
    for (style, _, _) in STYLES {
        for side in ["left", "right"] {
            let raw = json!({"style": style});
            let mut panel = MusicPanel::with_seed(&from_json(&raw), side == "right", 4.0, Some(1));
            let mut t = 1.0;
            for frame in 1..=IDLE_FRAMES {
                let quiet = (20..140).contains(&frame);
                let grid = if quiet { panel.frame(&silence, DT) } else { panel.frame(&demo_samples(t), DT) };
                t += DT;
                if frame % 10 == 0 {
                    let name = format!("idle-{style}-{side}-f{frame:03}");
                    checked += 1;
                    problems.extend(compare(&name, &to_255(&grid), &frames[&name], 1));
                }
            }
        }
    }
    assert_eq!(checked, frames.len(), "every golden frame is checked");
    assert!(problems.is_empty(), "{} of {checked} frames differ:\n{}", problems.len(), problems.join("\n"));
}

const IDLE_FRAMES: usize = 160;
