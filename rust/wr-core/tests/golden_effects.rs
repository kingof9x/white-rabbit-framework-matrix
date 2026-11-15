//! Plays every effects scenario saved in `tests/golden/effects.json` (by `scripts/golden_effects.py`)
//! with the Rust effects and compares what comes out with what Python drew: every pixel of every frame,
//! every `interval()`, the settings normalized and the port names parsed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use wr_core::effects::*;
use wr_core::{Grid, HEIGHT, WIDTH};

fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/effects.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn encode(grid: &Grid) -> Vec<String> {
    let bytes = grid.to_bytes();
    (0..HEIGHT).map(|y| (0..WIDTH).map(|x| format!("{:02x}", bytes[x][y])).collect()).collect()
}

fn decode(rows: &Value) -> Grid {
    let mut g = Grid::new();
    for (y, row) in rows.as_array().unwrap().iter().enumerate() {
        let row = row.as_str().unwrap();
        for x in 0..WIDTH {
            g.v[x][y] = i32::from_str_radix(&row[2 * x..2 * x + 2], 16).unwrap();
        }
    }
    g
}

fn side(step: &Value) -> Side {
    Side::from_name(step["side"].as_str().unwrap()).unwrap()
}

/// One scenario, step by step as `play()` in the Python script: the frames it keeps and what
/// `interval()` said, named by step number and side.
fn play(steps: &[Value], pictures: &BTreeMap<String, Grid>) -> (BTreeMap<String, Grid>, BTreeMap<String, Option<f64>>) {
    let mut m = Effects::new();
    let mut now = 0.0;
    let (mut frames, mut values) = (BTreeMap::new(), BTreeMap::new());
    for (i, step) in steps.iter().enumerate() {
        // Times are strings: serde_json can read a float one bit off, which changes frames
        if let Some(t) = step.get("t") {
            now = t.as_str().unwrap().parse().unwrap();
        }
        match step["op"].as_str().unwrap() {
            "configure" => m.configure(EffectsSettings::from_json(&step["settings"])),
            "present" => m.panel_present(side(step), step.get("present").and_then(Value::as_bool).unwrap_or(true)),
            "plug" => m.plug(step["position"].as_str(), step["in"].as_bool().unwrap(), now),
            "say" => m.say(step["text"].as_str().unwrap()),
            "slide" => m.start_slide(side(step), now),
            "forget" => m.forget(side(step)),
            "interval" => {
                values.insert(format!("{i:03}-{}", side(step).name()), m.interval(side(step)));
            }
            "frame" => {
                let grid = &pictures[step["picture"].as_str().unwrap()];
                let out = m.apply(side(step), grid, step["new"].as_bool().unwrap(), now);
                if step["keep"].as_bool().unwrap() {
                    frames.insert(format!("{i:03}-{}", side(step).name()), out);
                }
            }
            op => panic!("unknown step {op}"),
        }
    }
    (frames, values)
}

#[test]
fn effects_play_the_golden_frames() {
    let data = golden();
    let pictures: BTreeMap<String, Grid> =
        data["inputs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), decode(v))).collect();
    let saved = data["frames"].as_object().unwrap();
    let saved_values = data["values"].as_object().unwrap();

    let mut problems = Vec::new();
    let (mut drawn, mut checked_values) = (0, 0);
    for (name, steps) in data["scenarios"].as_object().unwrap() {
        let (frames, values) = play(steps.as_array().unwrap(), &pictures);
        for (key, grid) in &frames {
            let full = format!("{name}-{key}");
            drawn += 1;
            match saved.get(&full) {
                None => problems.push(format!("not in the saved frames: {full}")),
                Some(rows) => {
                    let ours = encode(grid);
                    let theirs: Vec<String> = serde_json::from_value(rows.clone()).unwrap();
                    if ours != theirs {
                        let y = (0..HEIGHT).find(|&y| ours[y] != theirs[y]).unwrap();
                        problems.push(format!("{full}: row {y} is {} in Rust, {} in Python", ours[y], theirs[y]));
                    }
                }
            }
        }
        for (key, value) in &values {
            let full = format!("{name}-{key}");
            checked_values += 1;
            let theirs = saved_values.get(&full).unwrap_or_else(|| panic!("no saved value {full}")).as_f64();
            let same = match (value, theirs) {
                (None, None) => true,
                (Some(a), Some(b)) => (a - b).abs() < 1e-12,
                _ => false,
            };
            if !same {
                problems.push(format!("{full}: interval {value:?} in Rust, {theirs:?} in Python"));
            }
        }
    }
    assert_eq!(drawn, saved.len(), "Rust drew a different number of frames");
    assert_eq!(checked_values, saved_values.len());
    assert!(problems.is_empty(), "{} of {drawn} frames differ:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn settings_are_normalized_like_python() {
    for case in golden()["settings"].as_array().unwrap() {
        let ours = serde_json::to_value(EffectsSettings::from_json(&case[0])).unwrap();
        assert_eq!(ours, case[1], "settings from {}", case[0]);
    }
}

#[test]
fn positions_are_read_like_python() {
    for case in golden()["positions"].as_array().unwrap() {
        let text = case[0].as_str();
        let (side, spot) = text.map(parse_position).unwrap_or((None, None));
        assert_eq!(side.map(Side::name), case[1].as_str(), "side of {}", case[0]);
        assert_eq!(spot.map(Spot::name), case[2].as_str(), "spot of {}", case[0]);
        assert_eq!(position_label(text), case[3].as_str().unwrap(), "label of {}", case[0]);
        if let Some(side) = side {
            let back = position_text(Some(side), spot).unwrap();
            assert_eq!(parse_position(&back), (Some(side), spot));
        }
    }
}
