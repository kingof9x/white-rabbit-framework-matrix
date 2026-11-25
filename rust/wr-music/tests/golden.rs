//! Plays every music case from `scripts/golden_frames.py` (music_cases) through the Rust looks and
//! compares the pictures with the ones Python saved in `tests/golden/music_looks.json`. Brightness is
//! stored as round(value * 255) and may differ by 1.
//!
//! Both sides use the same made-up music (`demo_samples`), the same time step and the same random seed
//! (numpy's generator is reproduced in `wr_core::nprand`), so any difference means the Rust maths has
//! drifted from Python's. Each golden frame is stored as 34 hex strings (one per row), two hex digits
//! per LED, left to right.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use wr_music::demo::demo_samples;
use wr_music::visualizer::{HEIGHT, WIDTH};
use wr_music::{from_json, to_leds, Frame, MusicPanel, Visualizer, STYLES};

/// Frames drawn per case (2.4 s of simulated time at `DT`).
const FRAMES: usize = 48;
/// Every 6th frame is compared (Python saved the same ones).
const SAVE_EVERY: usize = 6;
/// Seconds per frame, as in the Python script (20 frames a second).
const DT: f64 = 0.05;

/// The saved frames of one golden file: frame name -> 34 rows of hex. Panics if the file is missing
/// or malformed, which fails the test.
fn golden(file: &str) -> BTreeMap<String, Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden").join(file);
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    serde_json::from_value(data["frames"].clone()).unwrap()
}

/// A saved frame back to 0..255 values, indexed [x][y].
fn decode(rows: &[String]) -> [[i64; HEIGHT]; WIDTH] {
    let mut out = [[0; HEIGHT]; WIDTH];
    for (y, row) in rows.iter().enumerate() {
        for (x, column) in out.iter_mut().enumerate() {
            column[y] = i64::from_str_radix(&row[2 * x..2 * x + 2], 16).unwrap();
        }
    }
    out
}

/// A Rust picture (0..1) in the same form as the saved ones: rounded half to even like Python's round().
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

/// The two option sets every look is tried with: all defaults, and every drawing option switched on.
fn options(name: &str) -> Value {
    match name {
        "plain" => json!({}),
        _ => json!({"peak-dots": true, "shaded": true, "flip": true, "fall-speed": "fast", "sensitivity": 8}),
    }
}

/// Proves that all 22 looks, single and spanning, plain and with options, left and right, draw the
/// same pictures as Python (within 1 of 255) while music plays and after a stretch of silence, and that
/// `to_leds` gives exactly Python's LED values for a brightness ramp.
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
                    // Seed 1, as in the Python script, so the random looks (sparkle, fire, ...) draw the same.
                    let mut v = Visualizer::new(&from_json(&raw), side == "right", Some(1));
                    let base = format!("{style}-{}-{option_name}-{side}", if span { "span" } else { "single" });
                    // The demo music starts at t = 1 s; the Python script does the same, or nothing would match.
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
                    // 1.5 s of silence: the bars and effects have time to fall and fade.
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
            // A ramp from 0 (left or top) to 1 (bottom-right corner).
            *v = x as f64 / 8.0 * y as f64 / 33.0;
        }
    }
    // A few values just under the 0.02 cut-off of `to_leds`, which must come out as 0.
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
/// Proves the switch to and from the idle version: music for 1 s, silence for 6 s (the idle version
/// starts after 4 s), then music again, matching Python frame for frame (every 10th frame compared).
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
                // Frames 20..139 are silent (6 s), so the idle version starts after 4 s of it.
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

/// Frames drawn per idle case (8 s at `DT`).
const IDLE_FRAMES: usize = 160;
